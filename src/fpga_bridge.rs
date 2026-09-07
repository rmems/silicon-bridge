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
//! (`i16`, two's complement, big-endian). Parameter export (`.mem`) uses
//! **unsigned** `u16` instead. Encoding a stimulus with the export encoder
//! turns every negative (inhibitory) input into `0`.
//!
//! | Aspect | This module (UART TX/RX) | `fpga_export` (`.mem`) |
//! |---|---|---|
//! | Encode with | [`crate::encode_q88_signed`] | [`crate::encode_q88_unsigned`] |
//! | Decode with | [`crate::q88_signed_to_f32`] | [`crate::q88_to_f32`] |
//! | Raw type | `i16` (two's complement) | `u16` (unsigned) |
//! | Encoder input clamp | [`crate::STIMULUS_Q88_MIN`]`..=`[`crate::STIMULUS_Q88_MAX`] | `0.0..=255.99609375` |
//! | Byte order | raw binary, big-endian (MSB first) | ASCII hex, one `{:04X}` word per line |
//! | Use it for | host stimuli, RX membrane potentials | weights, thresholds, decay rates |

use serialport::{SerialPort, SerialPortInfo};
use std::io::{Read, Write};
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
pub struct FpgaBridge {
    port: Box<dyn SerialPort>,
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

impl FpgaBridge {
    /// Open the first FPGA-looking serial port that accepts a connection.
    ///
    /// Ports are discovered with [`find_fpga_ports`], so this works on Linux,
    /// macOS, and Windows. When enumeration returns nothing — `libudev` is
    /// unavailable, for instance — the historical Linux probe list
    /// (`/dev/ttyUSB0`, `/dev/ttyUSB1`, `/dev/ttyUSB2`) is tried instead.
    ///
    /// Use [`FpgaBridge::open`] when the port is already known; on a host with
    /// several USB serial adapters, discovery order decides which one wins.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let discovered: Vec<String> = find_fpga_ports()
            .into_iter()
            .map(|info| info.port_name)
            .collect();

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
        Ok(FpgaBridge { port, active: true })
    }

    /// Send neural stimuli to FPGA and read back spike states.
    ///
    /// Protocol (16-neuron SiliconBridge v3.0):
    ///   TX: 0xAA + 32 bytes (16 × signed Q8.8 stimuli, big-endian)
    ///   RX: 32 bytes (16 × signed Q8.8 potentials) + 2 bytes (spike flags) + 2 bytes (switches)
    ///
    /// Stimuli are encoded with [`crate::encode_q88_signed`] (`i16`) — **not**
    /// the unsigned `.mem` export encoder. RX potentials use
    /// [`crate::q88_signed_to_f32`].
    ///
    /// Input is accepted as a dynamic slice; if fewer than 16 values are provided,
    /// remaining channels are zero-padded. If more are provided, only the first 16 are sent.
    pub fn process_stimuli(
        &mut self,
        stimuli: &[f32],
    ) -> Result<(Vec<f32>, Vec<bool>), Box<dyn std::error::Error>> {
        if !self.active {
            return Err("FPGA bridge not active".into());
        }

        // ENCODE SITE (signed Q8.8) — UART TX. Not the unsigned u16 export path.
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

    /// Check if FPGA is responsive
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

    /// When enumeration comes back empty the historical Linux probe list is
    /// still tried, so hosts without `libudev` behave as they did before.
    #[test]
    fn empty_discovery_falls_back_to_the_legacy_linux_ports() {
        assert_eq!(
            candidate_ports(Vec::new()),
            vec!["/dev/ttyUSB0", "/dev/ttyUSB1", "/dev/ttyUSB2"]
        );
    }
}
