// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA Spike Readback — UART Bridge to Basys3 Hardware
//!
//! Handles UART communication with Basys3 FPGA to send stimuli
//! and read back spike states using the SiliconBridge v3.0 protocol.
//!
//! Extracted from Eagle-Lander's Ship of Theseus neuromorphic core.
//!
//! ## Connecting
//!
//! Prefer an explicit port and [`SerialConfig`]. [`FpgaBridge::open_with_config`]
//! and [`FpgaBridge::builder`] accept any caller-selected path — Linux
//! `ttyUSB`/`ttyACM`, macOS `cu.*`, Windows `COM*`, `/dev/serial/by-id/...`,
//! or another device node — and do not filter that path.
//!
//! [`FpgaBridge::new`] remains as a **legacy** probe helper. It is not the
//! recommended public path: it guesses among USB-serial-looking names and, when
//! enumeration is empty or fails, the historical `/dev/ttyUSB0..2` list. A
//! port that opens is transport-open only; nothing here authenticates an FPGA.
//!
//! Opening a named port does not require serial enumeration. On Linux,
//! `serialport::available_ports` typically needs `libudev`; macOS and Windows
//! do not. Default-feature builds of this crate do not link `serialport` or
//! those system libraries.
//!
//! ## Timeouts
//!
//! [`SerialConfig::timeout`] is the `serialport` **per-I/O** timeout (each
//! `read` / `write`), not a whole-transaction deadline.
//! [`FpgaBridge::process_stimuli`] uses `read_exact`, which may issue several
//! reads to fill the reply; the call can therefore outlast the configured
//! timeout. A timeout after the request has been written does **not** rewind
//! FPGA state. This module does not silently retry or resend stimulus frames
//! — a retry would apply the stimulus again.
//!
//! ## Q8.8 convention used here: signed
//!
//! Host stimuli and membrane potentials on the wire are **signed** Q8.8
//! (`i16`, two's complement, big-endian) encoded with
//! [`crate::encode_q88_signed`] — the **legacy UART clamp** of
//! [`crate::STIMULUS_Q88_MIN`]`..=`[`crate::STIMULUS_Q88_MAX`] (`±127.99`).
//! That helper is not the parameter-export contract: `.mem` images use
//! [`crate::encode_q88_signed_full`] over `[-128, 127.99609375]`, so `-128.0`
//! is `8003` on this wire and `8000` in a parameter file.
//!
//! | Aspect | This module (UART TX/RX) | `fpga_export` (`.mem`) |
//! |---|---|---|
//! | Encode with | [`crate::encode_q88_signed`] | [`crate::encode_q88_signed_full`] |
//! | Decode with | [`crate::q88_signed_to_f32`] | [`crate::q88_signed_to_f32`] |
//! | Raw type | `i16` (two's complement) | `i16` (two's complement) |
//! | Encoder input clamp | [`crate::STIMULUS_Q88_MIN`]`..=`[`crate::STIMULUS_Q88_MAX`] | [`crate::Q88_SIGNED_MIN`]`..=`[`crate::Q88_SIGNED_MAX`] |
//! | Byte order | raw binary, big-endian (MSB first) | ASCII hex, one `{:04X}` word per line |
//! | Use it for | host stimuli, RX membrane potentials | weights, thresholds, decay rates |
//!
//! [`crate::encode_q88_unsigned`] / [`crate::q88_to_f32`] are an
//! unsigned-magnitude pair and are **not** the hardware convention on either
//! path: a stimulus encoded with them turns every inhibitory input into `0`.

use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

/// Baud rate the SiliconBridge v3.0 firmware runs at.
pub const DEFAULT_BAUD_RATE: u32 = 115_200;

/// Default per-I/O timeout applied to the serial port.
///
/// This bounds a single `read` or `write`, not a whole
/// [`FpgaBridge::process_stimuli`] call — `read_exact` may issue several
/// reads, so the call can outlast this value.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(100);

/// Linux device nodes probed when port enumeration yields nothing.
///
/// `serialport::available_ports` needs `libudev` on Linux and can come back
/// empty on a stripped-down host, so the original hard-coded probe list is
/// kept as a fallback for the legacy [`FpgaBridge::new`] helper rather than
/// removed.
const LEGACY_LINUX_PORTS: [&str; 3] = ["/dev/ttyUSB0", "/dev/ttyUSB1", "/dev/ttyUSB2"];

/// Validated baud rate and per-I/O timeout for opening a serial transport.
///
/// Construct with [`SerialConfig::new`] or [`SerialConfig::default`]. Fields
/// are private so a zero baud rate or a zero timeout cannot bypass validation.
/// A zero timeout is how `serialport` waits forever; this crate requires a
/// finite nonzero timeout.
///
/// ```rust
/// # #[cfg(feature = "uart")] {
/// use std::time::Duration;
/// use silicon_bridge::SerialConfig;
///
/// let cfg = SerialConfig::default();
/// assert_eq!(cfg.baud_rate(), silicon_bridge::DEFAULT_BAUD_RATE);
/// assert_eq!(cfg.timeout(), silicon_bridge::DEFAULT_IO_TIMEOUT);
/// assert!(SerialConfig::new(0, Duration::from_millis(100)).is_err());
/// assert!(SerialConfig::new(115_200, Duration::ZERO).is_err());
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialConfig {
    baud_rate: u32,
    timeout: Duration,
}

/// Why a [`SerialConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SerialConfigError {
    /// Baud rate was `0`. `serialport` treats that as invalid input.
    ZeroBaudRate,
    /// Timeout was [`Duration::ZERO`]. In `serialport` that means "block
    /// forever", which is not a finite I/O timeout.
    ZeroTimeout,
}

impl fmt::Display for SerialConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBaudRate => {
                write!(f, "serial baud rate must be nonzero")
            }
            Self::ZeroTimeout => write!(
                f,
                "serial I/O timeout must be nonzero; Duration::ZERO means wait forever"
            ),
        }
    }
}

impl std::error::Error for SerialConfigError {}

impl SerialConfig {
    /// Validate `baud_rate > 0` and a nonzero (hence finite) I/O timeout.
    pub fn new(baud_rate: u32, timeout: Duration) -> Result<Self, SerialConfigError> {
        if baud_rate == 0 {
            return Err(SerialConfigError::ZeroBaudRate);
        }
        if timeout.is_zero() {
            return Err(SerialConfigError::ZeroTimeout);
        }
        Ok(Self { baud_rate, timeout })
    }

    /// SiliconBridge v3.0 defaults: [`DEFAULT_BAUD_RATE`] and
    /// [`DEFAULT_IO_TIMEOUT`].
    pub fn defaults() -> Self {
        Self {
            baud_rate: DEFAULT_BAUD_RATE,
            timeout: DEFAULT_IO_TIMEOUT,
        }
    }

    /// Configured baud rate in bits per second.
    pub fn baud_rate(&self) -> u32 {
        self.baud_rate
    }

    /// Per-I/O timeout. Not a whole-transaction deadline.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Replace the baud rate, re-validating the pair.
    pub fn with_baud_rate(self, baud_rate: u32) -> Result<Self, SerialConfigError> {
        Self::new(baud_rate, self.timeout)
    }

    /// Replace the per-I/O timeout, re-validating the pair.
    pub fn with_timeout(self, timeout: Duration) -> Result<Self, SerialConfigError> {
        Self::new(self.baud_rate, timeout)
    }
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Serial-port setting that failed after the descriptor was already open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SerialConfigureOp {
    /// `SerialPort::set_baud_rate`.
    BaudRate,
    /// `SerialPort::set_timeout`.
    Timeout,
}

impl fmt::Display for SerialConfigureOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaudRate => write!(f, "baud rate"),
            Self::Timeout => write!(f, "I/O timeout"),
        }
    }
}

/// Transport-layer failure when enumerating, opening, or configuring a port.
///
/// Framing and stimulus I/O stay on [`FpgaBridge::process_stimuli`] (owned by
/// the protocol-codec work). These variants carry the port name and operation
/// so a caller can act without scraping stdout — this crate does not print
/// connection summaries.
#[derive(Debug)]
#[non_exhaustive]
pub enum SerialError {
    /// [`SerialConfig`] validation failed before any OS call.
    InvalidConfig(SerialConfigError),
    /// `serialport::available_ports` failed. Common on Linux without
    /// `libudev`. Opening a named path does not need this call.
    Enumerate {
        /// Underlying `serialport` error.
        source: serialport::Error,
    },
    /// The OS refused to open `port`.
    Open {
        /// Path or COM name that was requested.
        port: String,
        /// Underlying `serialport` error.
        source: serialport::Error,
    },
    /// An already-open port rejected a baud-rate or timeout change.
    Configure {
        /// Port name reported by the transport, or `"<unnamed>"`.
        port: String,
        /// Which setting was being applied.
        operation: SerialConfigureOp,
        /// Underlying `serialport` error.
        source: serialport::Error,
    },
    /// Legacy [`FpgaBridge::new`] tried every candidate and none opened.
    ProbeExhausted {
        /// Names that were attempted, in probe order.
        candidates: Vec<String>,
        /// Failure from the last candidate, when at least one open was tried.
        last_error: Option<Box<SerialError>>,
    },
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(err) => write!(f, "{err}"),
            Self::Enumerate { source } => {
                write!(f, "failed to enumerate serial ports: {source}")
            }
            Self::Open { port, source } => {
                write!(f, "failed to open serial port {port}: {source}")
            }
            Self::Configure {
                port,
                operation,
                source,
            } => write!(
                f,
                "failed to set {operation} on serial port {port}: {source}"
            ),
            Self::ProbeExhausted {
                candidates,
                last_error,
            } => {
                write!(
                    f,
                    "no serial port opened among {} candidate(s): {}",
                    candidates.len(),
                    candidates.join(", ")
                )?;
                if let Some(err) = last_error {
                    write!(f, " (last error: {err})")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SerialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(err) => Some(err),
            Self::Enumerate { source } => Some(source),
            Self::Open { source, .. } => Some(source),
            Self::Configure { source, .. } => Some(source),
            Self::ProbeExhausted { last_error, .. } => last_error
                .as_deref()
                .map(|err| err as &(dyn std::error::Error + 'static)),
        }
    }
}

impl From<SerialConfigError> for SerialError {
    fn from(err: SerialConfigError) -> Self {
        Self::InvalidConfig(err)
    }
}

/// Map `serialport::available_ports` into [`SerialError::Enumerate`].
///
/// Extracted so tests can cover the error path without depending on `libudev`.
fn ports_from_available(
    result: Result<Vec<SerialPortInfo>, serialport::Error>,
) -> Result<Vec<SerialPortInfo>, SerialError> {
    result.map_err(|source| SerialError::Enumerate { source })
}

/// Filter already-enumerated ports to USB-serial-looking names.
///
/// This is a **selection heuristic**, not FPGA authentication. It does not
/// open ports or send stimuli.
fn select_usb_serial_ports(ports: Vec<SerialPortInfo>) -> Vec<SerialPortInfo> {
    ports
        .into_iter()
        .filter(|p| is_fpga_port_name(&p.port_name))
        .collect()
}

/// Fluent constructor for a named port plus baud rate and timeout.
///
/// The path is not filtered: any platform-native or custom identifier is
/// passed to `serialport`. Validation of baud rate and timeout happens in
/// [`FpgaBridgeBuilder::open`].
///
/// ```no_run
/// # #[cfg(feature = "uart")]
/// # fn example() -> Result<(), silicon_bridge::SerialError> {
/// use std::time::Duration;
/// use silicon_bridge::FpgaBridge;
///
/// let _bridge = FpgaBridge::builder("/dev/ttyACM0")
///     .baud_rate(115_200)
///     .timeout(Duration::from_millis(100))
///     .open()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct FpgaBridgeBuilder {
    port_name: String,
    baud_rate: u32,
    timeout: Duration,
}

impl FpgaBridgeBuilder {
    /// Start a builder for `port_name` with SiliconBridge v3.0 defaults.
    pub fn new(port_name: impl Into<String>) -> Self {
        Self {
            port_name: port_name.into(),
            baud_rate: DEFAULT_BAUD_RATE,
            timeout: DEFAULT_IO_TIMEOUT,
        }
    }

    /// Set the requested baud rate. Validated when [`Self::open`] is called.
    #[must_use]
    pub fn baud_rate(mut self, baud_rate: u32) -> Self {
        self.baud_rate = baud_rate;
        self
    }

    /// Set the per-I/O timeout. Validated when [`Self::open`] is called.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Replace baud rate and timeout from an already-validated config.
    #[must_use]
    pub fn config(mut self, config: SerialConfig) -> Self {
        self.baud_rate = config.baud_rate();
        self.timeout = config.timeout();
        self
    }

    /// Open the port. Does not send stimulus frames or print to stdout.
    pub fn open(self) -> Result<FpgaBridge, SerialError> {
        let config = SerialConfig::new(self.baud_rate, self.timeout)?;
        FpgaBridge::open_with_config(&self.port_name, config)
    }
}

/// A blocking UART connection to a Basys3 board speaking SiliconBridge v3.0.
///
/// Construction opens a serial transport only. [`Self::is_transport_open`]
/// reports that the OS accepted the port. Nothing in this crate verifies that
/// the peer is FPGA firmware or that it speaks SiliconBridge.
pub struct FpgaBridge {
    port: Box<dyn SerialPort>,
    /// `true` after a successful open or [`Self::from_port`]. Not protocol
    /// identity.
    transport_open: bool,
    /// Latch used by [`Self::ping`]. Starts `true` (transport open, not
    /// verified). A failed ping sets it `false`; this is not device identity.
    active: bool,
}

/// Report whether `port_name` looks like a USB serial adapter an FPGA dev
/// board would appear as.
///
/// The Basys3 exposes an FTDI bridge, but the device name depends entirely on
/// the host OS, so matching only `ttyUSB` made [`find_fpga_ports`] return an
/// empty list on every non-Linux machine:
///
/// | OS | Example name | Matched by |
/// |---|---|---|
/// | Linux (FTDI, CP210x) | `/dev/ttyUSB0` | `ttyUSB` prefix |
/// | Linux (CDC-ACM) | `/dev/ttyACM0` | `ttyACM` prefix |
/// | macOS | `/dev/cu.usbserial-210319B` | `cu.usb` / `tty.usb` prefix |
/// | Windows | `COM3` | `COM` + digits |
///
/// Any directory prefix is stripped first, so a bare name and a full device
/// path classify identically.
///
/// This is a **name heuristic for optional filtering**, not authentication.
/// [`FpgaBridge::open_with_config`] does not consult this predicate, so a
/// caller may open `cu.*` (including non-`usb` call-out nodes), `COM*`, or a
/// custom path such as `/dev/serial/by-id/...`.
///
/// ```rust
/// # #[cfg(feature = "uart")] {
/// use silicon_bridge::is_fpga_port_name;
///
/// assert!(is_fpga_port_name("/dev/ttyUSB0"));
/// assert!(is_fpga_port_name("/dev/cu.usbserial-210319B"));
/// assert!(is_fpga_port_name("COM3"));
/// assert!(!is_fpga_port_name("/dev/ttyS0")); // on-board 16550, not USB
/// # }
/// ```
pub fn is_fpga_port_name(port_name: &str) -> bool {
    // `available_ports` reports "/dev/ttyUSB0" on Unix but a bare "COM3" on
    // Windows, so compare on the final path component either way.
    let name = port_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(port_name)
        .trim();

    // Linux: FTDI / CP210x bridges, then CDC-ACM boards.
    if name.starts_with("ttyUSB") || name.starts_with("ttyACM") {
        return true;
    }

    // macOS: call-out and dial-in nodes for the same USB device.
    if name.starts_with("cu.usb") || name.starts_with("tty.usb") {
        return true;
    }

    // Windows: COM followed by at least one digit, so a stray "COMPUTER"
    // style name is not treated as a port.
    match name.strip_prefix("COM") {
        Some(index) => !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// Build the ordered probe list [`FpgaBridge::new`] walks.
///
/// Enumerated ports come first, in the order the OS reported them. The
/// [`LEGACY_LINUX_PORTS`] fallback is appended only when enumeration produced
/// nothing, so a host that can enumerate is never slowed down by opening
/// Linux device nodes that cannot exist on it.
fn candidate_ports(discovered: Vec<String>) -> Vec<String> {
    if discovered.is_empty() {
        return LEGACY_LINUX_PORTS
            .iter()
            .map(|p| (*p).to_string())
            .collect();
    }
    discovered
}

/// USB vendor id of FTDI, whose FT2232H is the Basys3's USB-UART bridge.
const VID_FTDI: u16 = 0x0403;

/// Digilent's own USB vendor id, used by some of their boards.
const VID_DIGILENT: u16 = 0x1443;

/// Probe-order preference for a serial port, lowest first.
///
/// This is a *preference*, not identification — a serial port cannot be
/// identified as an FPGA without writing to it. It only means that when several
/// adapters are attached, the one whose vendor id matches the Basys3's bridge
/// is tried before a generic USB-serial cable or an unclassified device.
fn usb_rank(vid: Option<u16>) -> u8 {
    match vid {
        Some(VID_FTDI | VID_DIGILENT) => 0,
        Some(_) => 1,
        None => 2,
    }
}

/// Extract the USB vendor id from a port, when the OS reported one.
fn port_vid(info: &SerialPortInfo) -> Option<u16> {
    match &info.port_type {
        SerialPortType::UsbPort(usb) => Some(usb.vid),
        _ => None,
    }
}

impl FpgaBridge {
    /// Open the first USB-serial-looking port that accepts a connection.
    ///
    /// **Legacy convenience.** Prefer [`Self::open_with_config`] or
    /// [`Self::builder`] when the device path is known. This method is kept so
    /// existing callers do not have to migrate immediately; it is not the
    /// recommended public path.
    ///
    /// Ports are discovered with [`find_fpga_ports`]. When enumeration returns
    /// nothing or fails (for example `libudev` is unavailable), the historical
    /// Linux probe list (`/dev/ttyUSB0`, `/dev/ttyUSB1`, `/dev/ttyUSB2`) is
    /// tried instead. Defaults are [`DEFAULT_BAUD_RATE`] and
    /// [`DEFAULT_IO_TIMEOUT`].
    ///
    /// This method does not print to stdout. Connection summaries are the
    /// caller's responsibility (`port_name` is in [`SerialError`] on failure).
    ///
    /// # This does not verify the peer
    ///
    /// A port that opens is assumed to be the board. Nothing handshakes
    /// first, and not for lack of trying: an earlier revision of this method
    /// called [`ping`](Self::ping) on every candidate before accepting it,
    /// but review caught two problems the SiliconBridge v3.0 wire protocol
    /// makes unavoidable without a firmware change. `ping` calls
    /// [`process_stimuli`](Self::process_stimuli), which is a real stimulus
    /// frame — the "probe" advances the FPGA's membrane/spike state exactly
    /// as a genuine experiment step would, so construction itself would have
    /// a side effect. And the protocol carries no marker that identifies the
    /// far end: a 36-byte reply from *any* serial device reads as a valid
    /// response, so the "handshake" would accept a coincidentally
    /// same-sized-reply peer just as readily as it now accepts a
    /// same-opens-fine one. Confirming the peer for real needs a dedicated,
    /// side-effect-free handshake command in the firmware protocol, which is
    /// out of scope for this crate.
    ///
    /// Two things narrow the risk today. Candidates are ordered by USB
    /// vendor id, so a port from FTDI or Digilent — the Basys3's bridge — is
    /// tried before a generic adapter or an unclassified device. And
    /// [`FpgaBridge::open_with_config`] takes a port name, so a caller that
    /// knows which device it wants never has to guess — [`list_serial_ports`]
    /// returns the full [`SerialPortInfo`], serial number included, to pick
    /// from.
    pub fn new() -> Result<Self, SerialError> {
        let discovered = match find_fpga_ports() {
            Ok(mut found) => {
                // Stable sort: within one rank the OS's own ordering is preserved.
                found.sort_by_key(|info| usb_rank(port_vid(info)));
                found.into_iter().map(|info| info.port_name).collect()
            }
            // Enumeration failure is not fatal for this legacy helper: fall
            // back to the historical Linux probe list, same as an empty result.
            Err(_) => Vec::new(),
        };

        let candidates = candidate_ports(discovered);
        let mut last_error = None;

        for port_name in &candidates {
            match Self::open(port_name) {
                Ok(bridge) => return Ok(bridge),
                Err(err) => last_error = Some(err),
            }
        }

        Err(SerialError::ProbeExhausted {
            candidates,
            last_error: last_error.map(Box::new),
        })
    }

    /// Start a builder for an explicit port identifier.
    ///
    /// The name is not required to pass [`is_fpga_port_name`].
    pub fn builder(port_name: impl Into<String>) -> FpgaBridgeBuilder {
        FpgaBridgeBuilder::new(port_name)
    }

    /// Open a named serial port at the SiliconBridge v3.0 defaults.
    ///
    /// Prefer [`Self::open_with_config`] when baud rate or timeout must differ
    /// from [`SerialConfig::default`]. Prefer this over [`FpgaBridge::new`]
    /// whenever the board's port is known — `COM4` on Windows,
    /// `/dev/cu.usbserial-210319B` on macOS, or a stable
    /// `/dev/serial/by-id/...` symlink on Linux. Discovery cannot tell two
    /// identical FTDI adapters apart; a name can.
    ///
    /// Does not send stimulus frames.
    pub fn open(port_name: &str) -> Result<Self, SerialError> {
        Self::open_with_config(port_name, SerialConfig::default())
    }

    /// Open `port_name` with an explicit baud rate and per-I/O timeout.
    ///
    /// `port_name` is passed to `serialport` as-is. It is **not** filtered
    /// through [`is_fpga_port_name`], so Linux `ttyUSB`/`ttyACM`, macOS `cu.*`,
    /// Windows `COM*`, and caller-selected custom paths are all accepted.
    ///
    /// Does not send stimulus frames. A successful return means the OS opened
    /// the descriptor ([`Self::is_transport_open`]); it does not mean the peer
    /// answered SiliconBridge.
    pub fn open_with_config(port_name: &str, config: SerialConfig) -> Result<Self, SerialError> {
        let port = serialport::new(port_name, config.baud_rate())
            .timeout(config.timeout())
            .open()
            .map_err(|source| SerialError::Open {
                port: port_name.to_string(),
                source,
            })?;
        Ok(Self {
            port,
            transport_open: true,
            active: true,
        })
    }

    /// Wrap an already-open serial transport.
    ///
    /// Use this in tests (and hosts that opened the port themselves) so
    /// configuration and error paths can run without a physical board.
    /// Scripted Read/Write exchange tests belong with the protocol-codec
    /// split, not here.
    ///
    /// The handle is marked transport-open only. This does not send frames or
    /// verify SiliconBridge identity.
    pub fn from_port(port: Box<dyn SerialPort>) -> Self {
        Self {
            port,
            transport_open: true,
            active: true,
        }
    }

    /// Apply `config` to an already-open port, then wrap it.
    ///
    /// Fails with [`SerialError::Configure`] if the transport rejects the baud
    /// rate or timeout. Does not send stimulus frames.
    pub fn from_port_with_config(
        mut port: Box<dyn SerialPort>,
        config: SerialConfig,
    ) -> Result<Self, SerialError> {
        let port_name = port.name().unwrap_or_else(|| "<unnamed>".to_string());
        port.set_baud_rate(config.baud_rate())
            .map_err(|source| SerialError::Configure {
                port: port_name.clone(),
                operation: SerialConfigureOp::BaudRate,
                source,
            })?;
        port.set_timeout(config.timeout())
            .map_err(|source| SerialError::Configure {
                port: port_name,
                operation: SerialConfigureOp::Timeout,
                source,
            })?;
        Ok(Self {
            port,
            transport_open: true,
            active: true,
        })
    }

    /// Send neural stimuli to FPGA and read back spike states.
    ///
    /// Protocol (16-neuron SiliconBridge v3.0):
    ///   TX: 0xAA + 32 bytes (16 × signed Q8.8 stimuli, big-endian)
    ///   RX: 32 bytes (16 × signed Q8.8 potentials) + 2 bytes (spike flags) + 2 bytes (switches)
    ///
    /// Stimuli are encoded with [`crate::encode_q88_signed`] (`i16`) — the
    /// UART clamp of ±127.99, **not** [`crate::encode_q88_signed_full`] and
    /// **not** the unsigned-magnitude [`crate::encode_q88_unsigned`], which
    /// would flatten every inhibitory input to `0`. RX potentials use
    /// [`crate::q88_signed_to_f32`].
    ///
    /// Input is accepted as a dynamic slice; if fewer than 16 values are provided,
    /// remaining channels are zero-padded. If more are provided, only the first 16 are sent.
    ///
    /// # Timeouts and retries
    ///
    /// The port's timeout is per `read`/`write`, not a deadline for this
    /// whole call. `read_exact` may perform several reads to fill 36 bytes;
    /// those extra reads are **not** a retransmission of the stimulus frame.
    /// This method does not resend `tx_data` on a short or timed-out reply.
    /// If the write succeeded and the read then fails, the FPGA has already
    /// consumed the stimulus — retrying this method applies it again.
    pub fn process_stimuli(
        &mut self,
        stimuli: &[f32],
    ) -> Result<(Vec<f32>, Vec<bool>), Box<dyn std::error::Error>> {
        if !self.active {
            return Err("FPGA bridge not active".into());
        }

        // ENCODE SITE (signed Q8.8) — UART TX with the legacy ±127.99 clamp.
        // Parameter `.mem` export uses `encode_q88_signed_full` instead.
        let mut tx_data = vec![0xAAu8]; // Sync byte
        for i in 0..16 {
            let s = stimuli.get(i).copied().unwrap_or(0.0);
            let q8_8 = crate::encode_q88_signed(s);
            tx_data.extend_from_slice(&q8_8.to_be_bytes());
        }

        // Send to FPGA
        self.port.write_all(&tx_data)?;
        self.port.flush()?;

        // Read response: 32 bytes potentials + 2 bytes spike flags + 2 bytes switches
        let mut rx_data = vec![0u8; 36];
        self.port.read_exact(&mut rx_data)?;

        // DECODE SITE (signed Q8.8, big-endian) — membrane potentials can be negative.
        let mut potentials = Vec::with_capacity(16);
        for i in 0..16 {
            let raw = i16::from_be_bytes([rx_data[i * 2], rx_data[i * 2 + 1]]);
            potentials.push(crate::q88_signed_to_f32(raw));
        }

        // Parse spike flags (16-bit, 1 per neuron)
        let spike_word = u16::from_be_bytes([rx_data[32], rx_data[33]]);
        let spikes = (0..16).map(|i| (spike_word & (1 << i)) != 0).collect();
        // rx_data[34..36] = switch state (available but unused here)

        Ok((potentials, spikes))
    }

    /// Send a real 16-channel stimulus of `0.1` and treat any reply as success.
    ///
    /// **This is not a passive discovery ping.** It calls
    /// [`process_stimuli`](Self::process_stimuli) with nonzero values, so a
    /// successful call advances membrane/spike state on the far end exactly
    /// as an experiment step would. It does not identify the peer as FPGA
    /// firmware: any 36-byte reply parses as valid. On error the handle is
    /// latched inactive ([`Self::is_active`] becomes `false`) even though the
    /// serial descriptor remains open ([`Self::is_transport_open`]).
    ///
    /// Do not use this to decide which enumerated port is "the board".
    pub fn ping(&mut self) -> bool {
        let test_stimuli = [0.1; 16];
        match self.process_stimuli(&test_stimuli) {
            Ok(_) => true,
            Err(_) => {
                self.active = false;
                false
            }
        }
    }

    /// Whether the last [`Self::ping`] succeeded, or no ping has been issued.
    ///
    /// Starts `true` after open. This is **not** verified protocol
    /// responsiveness and is **not** FPGA identity. Prefer
    /// [`Self::is_transport_open`] for "did the OS accept the port?".
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Whether this handle holds an opened serial transport.
    ///
    /// Distinct from [`Self::is_active`] (the ping latch) and from any claim
    /// that the peer speaks SiliconBridge.
    pub fn is_transport_open(&self) -> bool {
        self.transport_open
    }
}

/// Enumerate every serial port the OS reports.
///
/// This does **not** open ports, send stimuli, or claim that any device is an
/// FPGA. USB metadata on [`SerialPortInfo`] can help a caller choose a port;
/// that is selection assistance, not authentication.
///
/// On Linux this typically requires `libudev`. A missing library or other
/// enumerator failure is [`SerialError::Enumerate`], not an empty `Vec`.
/// Opening a caller-selected path via [`FpgaBridge::open_with_config`] does
/// not call this function.
pub fn list_serial_ports() -> Result<Vec<SerialPortInfo>, SerialError> {
    ports_from_available(serialport::available_ports())
}

/// Enumerate serial ports whose names look like a USB-serial FPGA adapter.
///
/// Matching is delegated to [`is_fpga_port_name`], so macOS `cu.usb*` nodes
/// and Windows `COM*` ports are returned alongside Linux `ttyUSB` / `ttyACM`
/// devices. This is a name heuristic, not a claim that every returned port is
/// an FPGA. Enumeration errors are returned rather than converted into an
/// empty list — use [`list_serial_ports`] when you need every OS-reported
/// device, including names this filter would drop.
///
/// Does not open ports or send stimuli.
pub fn find_fpga_ports() -> Result<Vec<SerialPortInfo>, SerialError> {
    Ok(select_usb_serial_ports(list_serial_ports()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::{ClearBuffer, DataBits, FlowControl, Parity, StopBits, UsbPortInfo};
    use std::io;

    #[test]
    fn linux_usb_serial_nodes_are_fpga_ports() {
        assert!(is_fpga_port_name("/dev/ttyUSB0"));
        assert!(is_fpga_port_name("/dev/ttyUSB11"));
        assert!(is_fpga_port_name("ttyUSB0"));
        assert!(is_fpga_port_name("/dev/ttyACM0"));
    }

    #[test]
    fn macos_usb_serial_nodes_are_fpga_ports() {
        assert!(is_fpga_port_name("/dev/cu.usbserial-210319B"));
        assert!(is_fpga_port_name("/dev/tty.usbserial-210319B"));
        assert!(is_fpga_port_name("/dev/cu.usbmodem14201"));
    }

    #[test]
    fn windows_com_ports_are_fpga_ports() {
        assert!(is_fpga_port_name("COM1"));
        assert!(is_fpga_port_name("COM12"));
        assert!(is_fpga_port_name(r"\\.\COM3"));
    }

    /// The old `contains("ttyUSB")` filter returned nothing on macOS and
    /// Windows, which is the bug this predicate exists to fix.
    #[test]
    fn non_linux_names_were_missed_by_a_ttyusb_substring_filter() {
        for name in ["/dev/cu.usbserial-210319B", "COM3", "/dev/ttyACM0"] {
            assert!(
                !name.contains("ttyUSB"),
                "{name} would need the new matcher"
            );
            assert!(is_fpga_port_name(name), "{name} should be an FPGA port");
        }
    }

    #[test]
    fn non_usb_serial_devices_are_rejected() {
        assert!(!is_fpga_port_name("/dev/ttyS0")); // on-board 16550 UART
        assert!(!is_fpga_port_name("/dev/ttyprintk"));
        assert!(!is_fpga_port_name("/dev/null"));
        assert!(!is_fpga_port_name("COM")); // no index
        assert!(!is_fpga_port_name("COMPUTER")); // not a port index
        assert!(!is_fpga_port_name(""));
    }

    /// A `ttyUSB` substring anywhere in a path used to be enough; matching now
    /// happens on the final path component only.
    #[test]
    fn matching_uses_the_final_path_component() {
        assert!(!is_fpga_port_name("/dev/ttyUSB-shaped-dir/ttyS0"));
        assert!(is_fpga_port_name("/some/odd/dir/ttyUSB0"));
    }

    #[test]
    fn discovered_ports_are_probed_in_order_without_the_legacy_list() {
        let discovered = vec!["COM7".to_string(), "COM3".to_string()];
        assert_eq!(candidate_ports(discovered.clone()), discovered);
    }

    /// `new` cannot confirm an FPGA without writing protocol bytes to whatever
    /// answered, so it prefers the Basys3's own bridge vendor instead.
    #[test]
    fn basys3_bridge_vendors_are_probed_before_generic_adapters() {
        assert_eq!(usb_rank(Some(VID_FTDI)), 0);
        assert_eq!(usb_rank(Some(VID_DIGILENT)), 0);
        assert_eq!(
            usb_rank(Some(0x10C4)),
            1,
            "CP210x: plausible, not preferred"
        );
        assert_eq!(usb_rank(None), 2, "no USB descriptor at all");

        assert!(usb_rank(Some(VID_FTDI)) < usb_rank(Some(0x10C4)));
        assert!(usb_rank(Some(0x10C4)) < usb_rank(None));
    }

    /// Ranking must be a total order over the three tiers, so `sort_by_key`
    /// gives a deterministic probe order.
    #[test]
    fn ranking_orders_a_mixed_bus_deterministically() {
        let mut ranks: Vec<u8> = [None, Some(0x10C4), Some(VID_FTDI), Some(0x1A86)]
            .into_iter()
            .map(usb_rank)
            .collect();
        ranks.sort_unstable();
        assert_eq!(ranks, [0, 1, 1, 2]);
    }

    /// When enumeration comes back empty the historical Linux probe list is
    /// still tried, so hosts without `libudev` behave as they did before.
    #[test]
    fn empty_discovery_falls_back_to_the_legacy_linux_ports() {
        assert_eq!(
            candidate_ports(Vec::new()),
            vec!["/dev/ttyUSB0", "/dev/ttyUSB1", "/dev/ttyUSB2"]
        );
    }

    #[test]
    fn serial_config_preserves_v3_defaults() {
        let cfg = SerialConfig::default();
        assert_eq!(cfg, SerialConfig::defaults());
        assert_eq!(cfg.baud_rate(), DEFAULT_BAUD_RATE);
        assert_eq!(cfg.timeout(), DEFAULT_IO_TIMEOUT);
        assert_eq!(cfg.baud_rate(), 115_200);
        assert_eq!(cfg.timeout(), Duration::from_millis(100));
    }

    #[test]
    fn serial_config_rejects_zero_baud_and_zero_timeout() {
        assert_eq!(
            SerialConfig::new(0, Duration::from_millis(100)),
            Err(SerialConfigError::ZeroBaudRate)
        );
        assert_eq!(
            SerialConfig::new(115_200, Duration::ZERO),
            Err(SerialConfigError::ZeroTimeout)
        );
        assert_eq!(
            SerialConfig::defaults().with_baud_rate(0),
            Err(SerialConfigError::ZeroBaudRate)
        );
        assert_eq!(
            SerialConfig::defaults().with_timeout(Duration::ZERO),
            Err(SerialConfigError::ZeroTimeout)
        );
    }

    #[test]
    fn serial_config_accepts_nonzero_custom_values() {
        let cfg = SerialConfig::new(9600, Duration::from_millis(50)).expect("valid config");
        assert_eq!(cfg.baud_rate(), 9600);
        assert_eq!(cfg.timeout(), Duration::from_millis(50));
        let cfg = cfg
            .with_baud_rate(57_600)
            .and_then(|c| c.with_timeout(Duration::from_secs(1)))
            .expect("still valid");
        assert_eq!(cfg.baud_rate(), 57_600);
        assert_eq!(cfg.timeout(), Duration::from_secs(1));
    }

    #[test]
    fn config_errors_carry_actionable_context() {
        let err = SerialError::from(SerialConfigError::ZeroBaudRate);
        assert!(err.to_string().contains("baud rate"));
        assert!(std::error::Error::source(&err).is_some());
        let err = SerialError::from(SerialConfigError::ZeroTimeout);
        assert!(err.to_string().contains("timeout"));
    }

    fn missing_port_name() -> &'static str {
        "/dev/silicon-bridge-no-such-uart-51"
    }

    #[test]
    fn open_missing_port_is_an_open_error_with_the_requested_name() {
        let err = FpgaBridge::open(missing_port_name()).expect_err("path must not exist");
        match err {
            SerialError::Open { port, .. } => assert_eq!(port, missing_port_name()),
            other => panic!("expected Open, got {other}"),
        }
        assert!(err.to_string().contains(missing_port_name()));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn open_with_config_does_not_reject_non_usb_or_custom_names() {
        let cfg = SerialConfig::new(9600, Duration::from_millis(20)).expect("valid");
        // Paths that must not exist on CI, including names the USB-serial
        // heuristic rejects. A real `/dev/ttyS0` is avoided so the test
        // cannot accidentally open on-board UART.
        for name in [
            "/dev/cu.Bluetooth-Incoming-Port",
            "/dev/serial/by-id/usb-silicon-bridge-missing",
            missing_port_name(),
        ] {
            assert!(
                !is_fpga_port_name(name),
                "{name} should be an explicit custom path, not a heuristic match"
            );
            let err =
                FpgaBridge::open_with_config(name, cfg).expect_err("CI has no such serial device");
            match err {
                SerialError::Open { port, .. } => assert_eq!(port, name),
                other => panic!("explicit API must not reject {name} before open; got {other}"),
            }
        }
    }

    #[test]
    fn builder_validates_config_before_opening() {
        let err = FpgaBridge::builder("/dev/ttyUSB0")
            .baud_rate(0)
            .open()
            .expect_err("zero baud");
        assert!(matches!(
            err,
            SerialError::InvalidConfig(SerialConfigError::ZeroBaudRate)
        ));

        let err = FpgaBridgeBuilder::new("COM3")
            .timeout(Duration::ZERO)
            .open()
            .expect_err("zero timeout");
        assert!(matches!(
            err,
            SerialError::InvalidConfig(SerialConfigError::ZeroTimeout)
        ));
    }

    #[test]
    fn builder_defaults_match_serial_config_defaults() {
        let builder = FpgaBridge::builder("COM4");
        assert_eq!(builder.baud_rate, DEFAULT_BAUD_RATE);
        assert_eq!(builder.timeout, DEFAULT_IO_TIMEOUT);
        let custom = SerialConfig::new(38400, Duration::from_millis(250)).expect("valid");
        let builder = FpgaBridge::builder("COM4").config(custom);
        assert_eq!(builder.baud_rate, 38400);
        assert_eq!(builder.timeout, Duration::from_millis(250));
    }

    #[test]
    fn builder_open_failure_preserves_the_requested_port() {
        let err = FpgaBridge::builder(missing_port_name())
            .baud_rate(57_600)
            .timeout(Duration::from_millis(10))
            .open()
            .expect_err("missing port");
        match err {
            SerialError::Open { port, .. } => assert_eq!(port, missing_port_name()),
            other => panic!("expected Open, got {other}"),
        }
    }

    #[test]
    fn enumerate_errors_are_distinct_from_an_empty_port_list() {
        let empty = ports_from_available(Ok(Vec::new())).expect("empty is success");
        assert!(empty.is_empty());

        let source = serialport::Error::new(serialport::ErrorKind::Unknown, "libudev missing");
        let err = ports_from_available(Err(source)).expect_err("enumerator failed");
        match err {
            SerialError::Enumerate { ref source } => {
                assert!(source.to_string().contains("libudev"));
            }
            other => panic!("expected Enumerate, got {other}"),
        }
        assert!(err.to_string().contains("enumerate"));
        assert!(std::error::Error::source(&err).is_some());
    }

    fn port_info(name: &str, vid: Option<u16>) -> SerialPortInfo {
        let port_type = match vid {
            Some(vid) => SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid: 0x6010,
                serial_number: Some("test".into()),
                manufacturer: None,
                product: None,
            }),
            None => SerialPortType::Unknown,
        };
        SerialPortInfo {
            port_name: name.to_string(),
            port_type,
        }
    }

    #[test]
    fn selection_keeps_usb_serial_names_and_drops_others() {
        let mixed = vec![
            port_info("/dev/ttyUSB0", Some(VID_FTDI)),
            port_info("/dev/ttyS0", None),
            port_info("COM12", Some(0x10C4)),
            port_info("/dev/cu.usbserial-210319B", Some(VID_DIGILENT)),
            port_info("/dev/null", None),
        ];
        let selected: Vec<String> = select_usb_serial_ports(mixed)
            .into_iter()
            .map(|p| p.port_name)
            .collect();
        assert_eq!(
            selected,
            vec!["/dev/ttyUSB0", "COM12", "/dev/cu.usbserial-210319B",]
        );
    }

    #[test]
    fn list_serial_ports_does_not_require_a_board() {
        // Either the OS enumerator works (possibly an empty list) or it
        // returns a typed enumerate error. Neither path opens a port.
        match list_serial_ports() {
            Ok(ports) => {
                for info in &ports {
                    assert!(!info.port_name.is_empty());
                }
            }
            Err(SerialError::Enumerate { .. }) => {}
            Err(other) => panic!("list_serial_ports must not open devices: {other}"),
        }
    }

    #[test]
    fn find_fpga_ports_is_a_filter_over_enumeration_not_identity() {
        match find_fpga_ports() {
            Ok(ports) => {
                for info in ports {
                    assert!(
                        is_fpga_port_name(&info.port_name),
                        "{} passed the name heuristic only",
                        info.port_name
                    );
                }
            }
            Err(SerialError::Enumerate { .. }) => {}
            Err(other) => panic!("find_fpga_ports must not open devices: {other}"),
        }
    }

    #[test]
    fn probe_exhausted_error_lists_candidates_and_last_cause() {
        let last = SerialError::Open {
            port: "/dev/ttyUSB2".into(),
            source: serialport::Error::new(serialport::ErrorKind::NoDevice, "no device"),
        };
        let err = SerialError::ProbeExhausted {
            candidates: vec![
                "/dev/ttyUSB0".into(),
                "/dev/ttyUSB1".into(),
                "/dev/ttyUSB2".into(),
            ],
            last_error: Some(Box::new(last)),
        };
        let text = err.to_string();
        assert!(text.contains("/dev/ttyUSB0"));
        assert!(text.contains("last error"));
        assert!(text.contains("/dev/ttyUSB2"));
    }

    /// In-memory `SerialPort` so open/config failure can be scripted without
    /// hardware. Read/Write are unused here (protocol tests belong in #52).
    struct MockPort {
        name: Option<String>,
        baud_rate: u32,
        timeout: Duration,
        fail_baud: bool,
        fail_timeout: bool,
    }

    impl MockPort {
        fn ok(name: &str) -> Self {
            Self {
                name: Some(name.to_string()),
                baud_rate: DEFAULT_BAUD_RATE,
                timeout: DEFAULT_IO_TIMEOUT,
                fail_baud: false,
                fail_timeout: false,
            }
        }

        fn unsupported(_kind: &str) -> serialport::Error {
            serialport::Error::new(serialport::ErrorKind::Unknown, "mock port: unsupported")
        }
    }

    impl io::Read for MockPort {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("mock port is not a data path"))
        }
    }

    impl io::Write for MockPort {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("mock port is not a data path"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SerialPort for MockPort {
        fn name(&self) -> Option<String> {
            self.name.clone()
        }

        fn baud_rate(&self) -> serialport::Result<u32> {
            Ok(self.baud_rate)
        }

        fn data_bits(&self) -> serialport::Result<DataBits> {
            Ok(DataBits::Eight)
        }

        fn flow_control(&self) -> serialport::Result<FlowControl> {
            Ok(FlowControl::None)
        }

        fn parity(&self) -> serialport::Result<Parity> {
            Ok(Parity::None)
        }

        fn stop_bits(&self) -> serialport::Result<StopBits> {
            Ok(StopBits::One)
        }

        fn timeout(&self) -> Duration {
            self.timeout
        }

        fn set_baud_rate(&mut self, baud_rate: u32) -> serialport::Result<()> {
            if self.fail_baud {
                return Err(serialport::Error::new(
                    serialport::ErrorKind::InvalidInput,
                    "mock refused baud rate",
                ));
            }
            self.baud_rate = baud_rate;
            Ok(())
        }

        fn set_data_bits(&mut self, _data_bits: DataBits) -> serialport::Result<()> {
            Err(Self::unsupported("data bits"))
        }

        fn set_flow_control(&mut self, _flow_control: FlowControl) -> serialport::Result<()> {
            Err(Self::unsupported("flow control"))
        }

        fn set_parity(&mut self, _parity: Parity) -> serialport::Result<()> {
            Err(Self::unsupported("parity"))
        }

        fn set_stop_bits(&mut self, _stop_bits: StopBits) -> serialport::Result<()> {
            Err(Self::unsupported("stop bits"))
        }

        fn set_timeout(&mut self, timeout: Duration) -> serialport::Result<()> {
            if self.fail_timeout {
                return Err(serialport::Error::new(
                    serialport::ErrorKind::InvalidInput,
                    "mock refused timeout",
                ));
            }
            self.timeout = timeout;
            Ok(())
        }

        fn write_request_to_send(&mut self, _level: bool) -> serialport::Result<()> {
            Ok(())
        }

        fn write_data_terminal_ready(&mut self, _level: bool) -> serialport::Result<()> {
            Ok(())
        }

        fn read_clear_to_send(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }

        fn read_data_set_ready(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }

        fn read_ring_indicator(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }

        fn read_carrier_detect(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }

        fn bytes_to_read(&self) -> serialport::Result<u32> {
            Ok(0)
        }

        fn bytes_to_write(&self) -> serialport::Result<u32> {
            Ok(0)
        }

        fn clear(&self, _buffer_to_clear: ClearBuffer) -> serialport::Result<()> {
            Ok(())
        }

        fn try_clone(&self) -> serialport::Result<Box<dyn SerialPort>> {
            Err(Self::unsupported("clone"))
        }

        fn set_break(&self) -> serialport::Result<()> {
            Ok(())
        }

        fn clear_break(&self) -> serialport::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn from_port_marks_transport_open_without_protocol_verification() {
        let bridge = FpgaBridge::from_port(Box::new(MockPort::ok("mock0")));
        assert!(bridge.is_transport_open());
        assert!(
            bridge.is_active(),
            "open is not a ping; the latch starts true"
        );
    }

    #[test]
    fn from_port_with_config_applies_baud_and_timeout() {
        let cfg = SerialConfig::new(9600, Duration::from_millis(40)).expect("valid");
        let bridge =
            FpgaBridge::from_port_with_config(Box::new(MockPort::ok("mock0")), cfg).expect("apply");
        assert!(bridge.is_transport_open());
        assert_eq!(bridge.port.baud_rate().expect("baud"), 9600);
        assert_eq!(bridge.port.timeout(), Duration::from_millis(40));
    }

    #[test]
    fn from_port_with_config_reports_baud_failure() {
        let mut mock = MockPort::ok("mock-baud");
        mock.fail_baud = true;
        let err = FpgaBridge::from_port_with_config(Box::new(mock), SerialConfig::default())
            .expect_err("baud refused");
        match err {
            SerialError::Configure {
                port,
                operation,
                ref source,
            } => {
                assert_eq!(port, "mock-baud");
                assert_eq!(operation, SerialConfigureOp::BaudRate);
                assert!(source.to_string().contains("baud"));
            }
            other => panic!("expected Configure baud, got {other}"),
        }
        assert!(err.to_string().contains("mock-baud"));
        assert!(err.to_string().contains("baud"));
    }

    #[test]
    fn from_port_with_config_reports_timeout_failure() {
        let mut mock = MockPort::ok("mock-timeout");
        mock.fail_timeout = true;
        let err = FpgaBridge::from_port_with_config(Box::new(mock), SerialConfig::default())
            .expect_err("timeout refused");
        match err {
            SerialError::Configure {
                port,
                operation,
                ref source,
            } => {
                assert_eq!(port, "mock-timeout");
                assert_eq!(operation, SerialConfigureOp::Timeout);
                assert!(source.to_string().contains("timeout"));
            }
            other => panic!("expected Configure timeout, got {other}"),
        }
    }

    #[test]
    fn ping_does_not_run_during_from_port_construction() {
        // A mock that cannot read would make ping fail and latch inactive.
        let bridge = FpgaBridge::from_port(Box::new(MockPort::ok("mock0")));
        assert!(bridge.is_active());
        assert!(bridge.is_transport_open());
    }
}
