// SPDX-License-Identifier: MIT OR Apache-2.0
//! Basys 3 UART host-session smoke harness (GitHub #84).
//!
//! Requires the `uart` feature and an **explicit** serial port. It does not
//! auto-probe or enumerate ports. It opens the caller-selected port, drains
//! any buffered RX bytes, runs one SiliconBridge v3.0 exchange, and prints a
//! single `PASS`/`FAIL` line to stdout for a maintainer to copy into
//! `docs/hardware-smoke-note.md`.
//!
//! Unlike the other crate examples, this binary opens real hardware when built
//! with `--features uart`. Do not run it on shared CI runners or without an
//! attached board and an explicit port.
//!
//! This proves a live silicon-bridge UART host session on one named board. It
//! is distinct from the silicon-hdl LED-heartbeat smoke (#68), which is not a
//! UART host session, and it is not a crates.io / docs.rs publication proof.
//!
//! Run (feature required):
//!
//! ```text
//! cargo run --features uart --example uart_host_smoke -- /dev/ttyUSB0
//! SILICON_BRIDGE_PORT=/dev/ttyUSB0 cargo run --features uart --example uart_host_smoke
//! SILICON_BRIDGE_BAUD=115200 cargo run --features uart --example uart_host_smoke -- /dev/ttyUSB0
//! ```

// Under default features (no `uart`) there is no serial transport. Keep the
// example compiling — and `cargo build --examples` green — with a no-op main
// that explains how to enable the feature and exits non-zero.
#[cfg(not(feature = "uart"))]
fn main() {
    eprintln!(
        "uart_host_smoke requires the `uart` feature: \
         cargo run --features uart --example uart_host_smoke -- <PORT>"
    );
    std::process::exit(2);
}

#[cfg(feature = "uart")]
fn main() {
    std::process::exit(run(std::env::args().nth(1)));
}

/// Default baud rate when `SILICON_BRIDGE_BAUD` is unset or unparseable.
#[cfg(feature = "uart")]
const DEFAULT_SMOKE_BAUD: u32 = 115_200;

/// Resolve the port, open it, run one exchange, and print the result line.
///
/// Returns the process exit code: `0` PASS, `1` open/exchange failure, `2`
/// usage error (no explicit port, or the missing-feature `main` path). The
/// no-port case is a usage error (exit `2`, no `FpgaBridge` constructed) and
/// also prints a `FAIL` line to stdout so a copy-paste record is unambiguous.
#[cfg(feature = "uart")]
fn run(cli_port: Option<String>) -> i32 {
    use silicon_bridge::FpgaBridge;

    // Explicit port only: CLI argument wins over the environment variable.
    // Neither present (or empty) => guidance + non-zero exit, no bridge built.
    let port = cli_port
        .filter(|p| !p.trim().is_empty())
        .or_else(|| std::env::var("SILICON_BRIDGE_PORT").ok())
        .filter(|p| !p.trim().is_empty());
    let port = match port {
        Some(p) => p,
        None => {
            eprintln!("==> no serial port provided (this harness does not auto-probe)");
            eprintln!("    pass a port as an argument or set SILICON_BRIDGE_PORT");
            eprintln!("    example: SILICON_BRIDGE_PORT=/dev/ttyUSB0");
            println!("FAIL: no serial port provided (set SILICON_BRIDGE_PORT or pass an argument)");
            return 2;
        }
    };

    // Optional baud override; a non-positive-integer value falls back cleanly.
    let baud = std::env::var("SILICON_BRIDGE_BAUD")
        .ok()
        .and_then(|b| b.trim().parse::<u32>().ok())
        .filter(|b| *b > 0)
        .unwrap_or(DEFAULT_SMOKE_BAUD);

    eprintln!("==> reference: Digilent Basys 3 / XC7A35T-1CPG236C / spikenaut_soc_basys3_top");
    eprintln!("==> opening {port} @ {baud} baud (explicit port, no auto-probe)");

    let mut bridge = match FpgaBridge::builder(&port).baud_rate(baud).open() {
        Ok(bridge) => bridge,
        Err(err) => {
            eprintln!("==> open failed: {err}");
            println!("FAIL: open port={port} baud={baud} error={err}");
            return 1;
        }
    };
    eprintln!("==> transport_open = {}", bridge.is_transport_open());

    match report_exchange(&mut bridge) {
        Ok(summary) => {
            eprintln!("==> exchange ok: {summary}");
            println!(
                "PASS: live silicon-bridge UART host session port={port} baud={baud} ({summary})"
            );
            0
        }
        Err(reason) => {
            println!("FAIL: exchange port={port} baud={baud} reason={reason}");
            1
        }
    }
}

/// Run one SiliconBridge v3.0 exchange and describe the outcome.
///
/// On I/O failure this inspects [`FpgaBridge::needs_recovery`] and, when set,
/// calls [`FpgaBridge::recover`] once, reporting its result to stderr. The
/// error is returned regardless of the recovery outcome. Factored out of
/// [`run`] so it can be driven offline with [`FpgaBridge::from_port`].
#[cfg(feature = "uart")]
fn report_exchange(bridge: &mut silicon_bridge::FpgaBridge) -> Result<String, String> {
    use silicon_bridge::DenseQ88Layout;

    // Drop any complete reply left in the host RX buffer from an aborted prior
    // run so this exchange cannot PASS on stale same-length bytes alone.
    bridge
        .recover()
        .map_err(|err| format!("pre-exchange RX drain failed: {err}"))?;

    let layout = DenseQ88Layout::silicon_bridge_v3();
    let stimuli = vec![0.0_f32; layout.input_channels()];

    match bridge.exchange(&layout, &stimuli) {
        Ok(response) => Ok(format!(
            "{} potentials, {} spikes",
            response.potentials.len(),
            response.spikes.len()
        )),
        Err(err) => {
            eprintln!("==> exchange failed: {err}");
            if bridge.needs_recovery() {
                eprintln!("==> needs_recovery = true; calling recover()");
                match bridge.recover() {
                    Ok(()) => eprintln!("==> recover succeeded"),
                    Err(rec) => eprintln!("==> recover failed: {rec}"),
                }
            }
            Err(err.to_string())
        }
    }
}

#[cfg(all(test, feature = "uart"))]
mod tests {
    use silicon_bridge::{DenseQ88Layout, FpgaBridge, SILICON_BRIDGE_V3_RX_LEN};
    use std::collections::VecDeque;
    use std::io;
    use std::time::Duration;

    use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, StopBits};

    /// Minimal in-memory `SerialPort` for offline exchange tests. Reads are
    /// served from a scripted queue; writes are discarded. No OS device is
    /// opened. This mirrors the `MockPort` pattern in `src/fpga_bridge.rs`
    /// (that one is private to the crate's tests).
    struct MockPort {
        reads: VecDeque<io::Result<Vec<u8>>>,
        leftover: Vec<u8>,
    }

    impl MockPort {
        fn boxed(reads: VecDeque<io::Result<Vec<u8>>>) -> Box<dyn SerialPort> {
            Box::new(Self {
                reads,
                leftover: Vec::new(),
            })
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
            match self.reads.pop_front() {
                None => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "mock: drained",
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
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SerialPort for MockPort {
        fn name(&self) -> Option<String> {
            Some("mock".into())
        }
        fn baud_rate(&self) -> serialport::Result<u32> {
            Ok(115_200)
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
            Duration::from_millis(100)
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
        fn set_timeout(&mut self, _timeout: Duration) -> serialport::Result<()> {
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

    #[test]
    fn exchange_succeeds_on_full_v3_reply() {
        // A full 36-byte v3 reply (all zeros decodes cleanly) => success.
        let mut reads = VecDeque::new();
        reads.push_back(Ok(vec![0u8; SILICON_BRIDGE_V3_RX_LEN]));
        let mut bridge = FpgaBridge::from_port(MockPort::boxed(reads));

        let summary = super::report_exchange(&mut bridge).expect("full reply should pass");
        let layout = DenseQ88Layout::silicon_bridge_v3();
        assert!(summary.contains(&format!("{} potentials", layout.output_neurons())));
        assert!(!bridge.needs_recovery(), "clean exchange needs no recovery");
    }

    #[test]
    fn exchange_fails_and_recovers_on_short_read() {
        // Fewer than 36 bytes, then EOF => read_exact fails, recovery latched.
        let mut reads = VecDeque::new();
        reads.push_back(Ok(vec![0u8; 10]));
        reads.push_back(Ok(Vec::new()));
        let mut bridge = FpgaBridge::from_port(MockPort::boxed(reads));

        let err = super::report_exchange(&mut bridge).expect_err("short read should fail");
        assert!(!err.is_empty());
        // report_exchange calls recover() when needs_recovery latched; a
        // successful recover() clears the latch.
        assert!(
            !bridge.needs_recovery(),
            "recover() should have cleared the latch after the failed read"
        );
    }

    #[test]
    fn exchange_fails_and_recovers_on_eof() {
        // Immediate EOF (0 bytes) => read_exact fails on the first read.
        let mut reads = VecDeque::new();
        reads.push_back(Ok(Vec::new()));
        let mut bridge = FpgaBridge::from_port(MockPort::boxed(reads));

        let err = super::report_exchange(&mut bridge).expect_err("eof should fail");
        assert!(!err.is_empty());
        assert!(
            !bridge.needs_recovery(),
            "recover() should have cleared the latch after the eof read"
        );
    }
}
