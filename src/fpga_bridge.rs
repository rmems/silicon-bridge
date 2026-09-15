// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA Spike Readback — UART Bridge to Basys3 Hardware
//!
//! Handles UART communication with Basys3 FPGA to send stimuli
//! and read back spike states using the SiliconBridge v3.0 protocol.
//!
//! Extracted from Eagle-Lander's Ship of Theseus neuromorphic core.
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
//!
//! ## Frames vs transport
//!
//! Request/response bytes are encoded by [`crate::DenseQ88Layout`] /
//! [`crate::encode_stimuli`] / [`crate::decode_response`] and do not depend
//! on this module. [`DenseQ88Layout::silicon_bridge_v3`] is the 16-channel
//! software profile that matches current Basys3 firmware. Other dense sizes
//! need matching FPGA firmware — changing the host layout is not enough.
//!
//! A failed or partial exchange sets [`FpgaBridge::needs_recovery`]. Further
//! stimuli are refused until [`FpgaBridge::recover`]. This crate does not
//! resend frames automatically. The unframed v3 reply cannot detect every
//! stale same-length buffer; there is no checksum.

use crate::{
    CodecError, DenseQ88Layout, SILICON_BRIDGE_V3_RX_LEN, StimulusResponse, decode_response,
    encode_stimuli, encode_stimuli_legacy_v3,
};
use serialport::{ClearBuffer, SerialPort, SerialPortInfo, SerialPortType};
use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

/// Baud rate the SiliconBridge v3.0 firmware runs at.
const BAUD_RATE: u32 = 115_200;

/// Per-read timeout applied to the serial port.
///
/// This bounds a single `read`, not a whole [`FpgaBridge::process_stimuli`]
/// call — a partial reply is retried, so the call can outlast this value.
const READ_TIMEOUT: Duration = Duration::from_millis(100);

/// Linux device nodes probed when port enumeration yields nothing.
///
/// `serialport::available_ports` needs `libudev` on Linux and can come back
/// empty on a stripped-down host, so the original hard-coded probe list is
/// kept as a fallback rather than removed.
const LEGACY_LINUX_PORTS: [&str; 3] = ["/dev/ttyUSB0", "/dev/ttyUSB1", "/dev/ttyUSB2"];

/// A blocking UART connection to a Basys3 board speaking SiliconBridge v3.0.
///
/// Framing lives in [`crate::DenseQ88Layout`]. This type owns the serial
/// descriptor, recovery latch, and I/O.
pub struct FpgaBridge {
    port: Box<dyn SerialPort>,
    active: bool,
    /// Set when a write/read fails mid-exchange. Further stimuli are refused
    /// until [`Self::recover`].
    needs_recovery: bool,
}

impl fmt::Debug for FpgaBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FpgaBridge")
            .field("port", &self.port.name())
            .field("active", &self.active)
            .field("needs_recovery", &self.needs_recovery)
            .finish()
    }
}

/// Failure during a stimulus exchange on an already-open port.
///
/// Codec failures happen before any write. A failed or partial I/O
/// transaction sets [`FpgaBridge::needs_recovery`]; the next exchange is
/// refused until [`FpgaBridge::recover`]. This crate does not resend the
/// stimulus frame automatically.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExchangeError {
    /// [`FpgaBridge::is_active`] is false (the `ping` latch).
    NotActive,
    /// A previous exchange failed part-way; call [`FpgaBridge::recover`].
    NeedsRecovery,
    /// Encode or decode failed. No bytes were written when this comes from
    /// [`encode_stimuli`].
    Codec(CodecError),
    /// Writing or flushing the request failed. The peer may have seen a
    /// prefix; the stimulus is not retried.
    Write {
        /// Underlying I/O error.
        source: io::Error,
    },
    /// Reading the reply failed after the request was written. The peer has
    /// already consumed the stimulus.
    Read {
        /// Underlying I/O error.
        source: io::Error,
    },
    /// [`FpgaBridge::recover`] could not clear the port buffers.
    Recover {
        /// Underlying I/O error.
        source: io::Error,
    },
}

impl fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotActive => write!(f, "FPGA bridge is not active"),
            Self::NeedsRecovery => write!(
                f,
                "previous UART exchange failed; call recover() before further stimuli"
            ),
            Self::Codec(err) => write!(f, "{err}"),
            Self::Write { source } => write!(f, "failed to write stimulus frame: {source}"),
            Self::Read { source } => write!(f, "failed to read stimulus reply: {source}"),
            Self::Recover { source } => write!(f, "failed to recover serial buffers: {source}"),
        }
    }
}

impl std::error::Error for ExchangeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(err) => Some(err),
            Self::Write { source } | Self::Read { source } | Self::Recover { source } => {
                Some(source)
            }
            Self::NotActive | Self::NeedsRecovery => None,
        }
    }
}

impl From<CodecError> for ExchangeError {
    fn from(err: CodecError) -> Self {
        Self::Codec(err)
    }
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
    /// Open the first FPGA-looking serial port that accepts a connection.
    ///
    /// Ports are discovered with [`find_fpga_ports`], so this works on Linux,
    /// macOS, and Windows. When enumeration returns nothing — `libudev` is
    /// unavailable, for instance — the historical Linux probe list
    /// (`/dev/ttyUSB0`, `/dev/ttyUSB1`, `/dev/ttyUSB2`) is tried instead.
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
    /// [`FpgaBridge::open`] takes a port name, so a caller that knows which
    /// device it wants never has to guess — [`find_fpga_ports`] returns the
    /// full [`SerialPortInfo`], serial number included, to pick from.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut found = find_fpga_ports();
        // Stable sort: within one rank the OS's own ordering is preserved.
        found.sort_by_key(|info| usb_rank(port_vid(info)));

        let discovered: Vec<String> = found.into_iter().map(|info| info.port_name).collect();

        let candidates = candidate_ports(discovered);

        for port_name in &candidates {
            if let Ok(bridge) = Self::open(port_name) {
                println!("[fpga] Connected to FPGA on {port_name}");
                return Ok(bridge);
            }
        }

        Err(format!(
            "FPGA not found on any of {} candidate serial port(s): {}",
            candidates.len(),
            candidates.join(", ")
        )
        .into())
    }

    /// Open a named serial port at 115200 baud.
    ///
    /// Prefer this over [`FpgaBridge::new`] whenever the board's port is known
    /// — `COM4` on Windows, `/dev/cu.usbserial-210319B` on macOS, or a stable
    /// `/dev/serial/by-id/...` symlink on Linux. Discovery cannot tell two
    /// identical FTDI adapters apart; a name can.
    pub fn open(port_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let port = serialport::new(port_name, BAUD_RATE)
            .timeout(READ_TIMEOUT)
            .open()?;
        Ok(FpgaBridge {
            port,
            active: true,
            needs_recovery: false,
        })
    }

    /// Wrap an already-open serial transport.
    ///
    /// Used to script Read/Write exchanges in tests without a physical board.
    /// Compatible with an injected port from the configurable-transport work
    /// (#51). Does not send frames or verify SiliconBridge identity.
    pub fn from_port(port: Box<dyn SerialPort>) -> Self {
        Self {
            port,
            active: true,
            needs_recovery: false,
        }
    }

    /// Legacy v3 exchange: pad/truncate to 16 channels and return potentials
    /// and spikes (switch state discarded).
    ///
    /// Encoding is [`encode_stimuli_legacy_v3`]. Prefer [`Self::exchange`]
    /// with [`DenseQ88Layout::silicon_bridge_v3`] for an exact finite vector
    /// and a structured [`StimulusResponse`] (including switches). A non-v3
    /// layout needs matching FPGA firmware.
    ///
    /// The port timeout is per `read`/`write`, not a whole-call deadline.
    /// Extra `read_exact` syscalls are not a retransmission. After I/O
    /// failure the handle requires [`Self::recover`]. The unframed v3 RX
    /// format cannot reliably detect a stale same-length reply.
    pub fn process_stimuli(
        &mut self,
        stimuli: &[f32],
    ) -> Result<(Vec<f32>, Vec<bool>), ExchangeError> {
        let response = self.exchange_legacy(stimuli)?;
        Ok((response.potentials, response.spikes))
    }

    /// Legacy v3 exchange returning the full structured reply.
    ///
    /// Pads/truncates like [`encode_stimuli_legacy_v3`].
    pub fn exchange_legacy(&mut self, stimuli: &[f32]) -> Result<StimulusResponse, ExchangeError> {
        self.ensure_ready()?;
        let tx = encode_stimuli_legacy_v3(stimuli);
        self.write_frame(&tx)?;
        let rx = self.read_frame(SILICON_BRIDGE_V3_RX_LEN)?;
        decode_response(&DenseQ88Layout::silicon_bridge_v3(), &rx).map_err(ExchangeError::from)
    }

    /// Checked exchange for an explicit layout.
    ///
    /// `stimuli` must have exactly `layout.input_channels()` finite values or
    /// this returns [`ExchangeError::Codec`] **before** any write.
    pub fn exchange(
        &mut self,
        layout: &DenseQ88Layout,
        stimuli: &[f32],
    ) -> Result<StimulusResponse, ExchangeError> {
        self.ensure_ready()?;
        let tx = encode_stimuli(layout, stimuli)?;
        let rx_len = layout.rx_len()?;
        self.write_frame(&tx)?;
        let rx = self.read_frame(rx_len)?;
        decode_response(layout, &rx).map_err(ExchangeError::from)
    }

    fn ensure_ready(&self) -> Result<(), ExchangeError> {
        if !self.active {
            return Err(ExchangeError::NotActive);
        }
        if self.needs_recovery {
            return Err(ExchangeError::NeedsRecovery);
        }
        Ok(())
    }

    fn write_frame(&mut self, tx: &[u8]) -> Result<(), ExchangeError> {
        if let Err(source) = self.port.write_all(tx).and_then(|()| self.port.flush()) {
            self.needs_recovery = true;
            return Err(ExchangeError::Write { source });
        }
        Ok(())
    }

    fn read_frame(&mut self, len: usize) -> Result<Vec<u8>, ExchangeError> {
        let mut rx = vec![0u8; len];
        if let Err(source) = self.port.read_exact(&mut rx) {
            self.needs_recovery = true;
            return Err(ExchangeError::Read { source });
        }
        Ok(rx)
    }

    /// Clear serial buffers after a failed exchange so the next call does
    /// not consume leftover bytes as a fresh reply.
    ///
    /// Required after [`ExchangeError::Write`] or [`ExchangeError::Read`].
    /// Clearing the host buffers cannot detect a stale same-length reply
    /// already sitting in the FPGA UART.
    pub fn recover(&mut self) -> Result<(), ExchangeError> {
        self.port
            .clear(ClearBuffer::All)
            .map_err(|source| ExchangeError::Recover {
                source: io::Error::from(source),
            })?;
        self.needs_recovery = false;
        Ok(())
    }

    /// Whether the next exchange is blocked pending [`Self::recover`].
    pub fn needs_recovery(&self) -> bool {
        self.needs_recovery
    }

    /// Send a real 16-channel stimulus of `0.1` and treat any reply as success.
    ///
    /// **This is not a passive discovery ping.** It calls
    /// [`process_stimuli`](Self::process_stimuli) with nonzero values.
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

    /// Get connection status
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Enumerate serial ports that look like an FPGA dev board.
///
/// Matching is delegated to [`is_fpga_port_name`], so macOS `cu.usb*` nodes
/// and Windows `COM*` ports are returned alongside Linux `ttyUSB` / `ttyACM`
/// devices. Returns an empty vector when the platform cannot enumerate ports.
pub fn find_fpga_ports() -> Vec<SerialPortInfo> {
    match serialport::available_ports() {
        Ok(ports) => ports
            .into_iter()
            .filter(|p| is_fpga_port_name(&p.port_name))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::{DataBits, FlowControl, Parity, StopBits};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

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

    struct MockPort {
        name: Option<String>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        read_chunks: VecDeque<io::Result<Vec<u8>>>,
        leftover: Vec<u8>,
        write_err: Option<io::ErrorKind>,
        timeout: Duration,
    }

    impl MockPort {
        fn scripted(writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
            Self {
                name: Some("mock".into()),
                writes,
                read_chunks: VecDeque::new(),
                leftover: Vec::new(),
                write_err: None,
                timeout: READ_TIMEOUT,
            }
        }

        fn unsupported() -> serialport::Error {
            serialport::Error::new(serialport::ErrorKind::Unknown, "mock: unsupported")
        }
    }

    impl io::Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.leftover.is_empty() {
                let n = buf.len().min(self.leftover.len());
                buf[..n].copy_from_slice(&self.leftover[..n]);
                self.leftover.drain(..n);
                return Ok(n);
            }
            match self.read_chunks.pop_front() {
                None => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "mock: no scripted read",
                )),
                Some(Err(err)) => Err(err),
                Some(Ok(data)) if data.is_empty() => Ok(0),
                Some(Ok(data)) => {
                    let n = buf.len().min(data.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    if n < data.len() {
                        self.leftover.extend_from_slice(&data[n..]);
                    }
                    Ok(n)
                }
            }
        }
    }

    impl io::Write for MockPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(kind) = self.write_err {
                return Err(io::Error::new(kind, "mock: scripted write failure"));
            }
            self.writes.lock().expect("write log").push(buf.to_vec());
            Ok(buf.len())
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
            Ok(BAUD_RATE)
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
        fn set_baud_rate(&mut self, _baud_rate: u32) -> serialport::Result<()> {
            Ok(())
        }
        fn set_data_bits(&mut self, _data_bits: DataBits) -> serialport::Result<()> {
            Err(Self::unsupported())
        }
        fn set_flow_control(&mut self, _flow_control: FlowControl) -> serialport::Result<()> {
            Err(Self::unsupported())
        }
        fn set_parity(&mut self, _parity: Parity) -> serialport::Result<()> {
            Err(Self::unsupported())
        }
        fn set_stop_bits(&mut self, _stop_bits: StopBits) -> serialport::Result<()> {
            Err(Self::unsupported())
        }
        fn set_timeout(&mut self, timeout: Duration) -> serialport::Result<()> {
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
            Err(Self::unsupported())
        }
        fn set_break(&self) -> serialport::Result<()> {
            Ok(())
        }
        fn clear_break(&self) -> serialport::Result<()> {
            Ok(())
        }
    }

    fn v3_zero_reply() -> Vec<u8> {
        vec![0u8; SILICON_BRIDGE_V3_RX_LEN]
    }

    #[test]
    fn scripted_legacy_exchange_writes_once_and_returns_switches() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockPort::scripted(Arc::clone(&writes));
        let mut rx = v3_zero_reply();
        rx[0] = 0xFF;
        rx[1] = 0x00;
        rx[34] = 0x00;
        rx[35] = 0x42;
        mock.read_chunks.push_back(Ok(rx));
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        let response = bridge
            .exchange_legacy(&[0.1; 16])
            .expect("scripted v3 reply");
        assert_eq!(response.potentials[0], -1.0);
        assert_eq!(response.switches, Some(0x0042));
        let log = writes.lock().expect("log");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0], encode_stimuli_legacy_v3(&[0.1; 16]));
    }

    #[test]
    fn scripted_short_reads_do_not_resend() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockPort::scripted(Arc::clone(&writes));
        mock.read_chunks.push_back(Ok(vec![0u8; 10]));
        mock.read_chunks.push_back(Ok(vec![0u8; 10]));
        mock.read_chunks.push_back(Ok(vec![0u8; 16]));
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        bridge.process_stimuli(&[0.0; 16]).expect("assembled");
        assert_eq!(writes.lock().expect("log").len(), 1);
    }

    #[test]
    fn scripted_eof_requires_recover_and_does_not_resend() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockPort::scripted(Arc::clone(&writes));
        mock.read_chunks.push_back(Ok(Vec::new()));
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        assert!(matches!(
            bridge.process_stimuli(&[0.1; 16]),
            Err(ExchangeError::Read { .. })
        ));
        assert!(bridge.needs_recovery());
        assert_eq!(writes.lock().expect("log").len(), 1);
        assert!(matches!(
            bridge.process_stimuli(&[0.1; 16]),
            Err(ExchangeError::NeedsRecovery)
        ));
        assert_eq!(writes.lock().expect("log").len(), 1);
        bridge.recover().expect("clear");
        assert!(!bridge.needs_recovery());
    }

    #[test]
    fn scripted_timeout_and_write_errors_propagate() {
        let mut mock = MockPort::scripted(Arc::new(Mutex::new(Vec::new())));
        mock.read_chunks
            .push_back(Err(io::Error::new(io::ErrorKind::TimedOut, "mock timeout")));
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        match bridge.process_stimuli(&[0.0; 16]) {
            Err(ExchangeError::Read { ref source }) => {
                assert_eq!(source.kind(), io::ErrorKind::TimedOut);
            }
            other => panic!("expected Read timeout, got {other:?}"),
        }

        let mut mock = MockPort::scripted(Arc::new(Mutex::new(Vec::new())));
        mock.write_err = Some(io::ErrorKind::BrokenPipe);
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        assert!(matches!(
            bridge.process_stimuli(&[0.0; 16]),
            Err(ExchangeError::Write { .. })
        ));
        assert!(bridge.needs_recovery());
    }

    #[test]
    fn checked_exchange_rejects_wrong_length_before_any_write() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockPort::scripted(Arc::clone(&writes));
        mock.read_chunks.push_back(Ok(v3_zero_reply()));
        let mut bridge = FpgaBridge::from_port(Box::new(mock));
        let err = bridge
            .exchange(&DenseQ88Layout::silicon_bridge_v3(), &[0.1; 15])
            .expect_err("wrong length");
        assert!(matches!(
            err,
            ExchangeError::Codec(CodecError::WrongInputLength {
                expected: 16,
                actual: 15
            })
        ));
        assert!(writes.lock().expect("log").is_empty());
        assert!(!bridge.needs_recovery());
    }
}
