// SPDX-License-Identifier: MIT OR Apache-2.0
//! Replay a recorded live-telemetry JSONL log back through the flashed
//! `spikenaut_soc_basys3_top` array over SiliconBridge v3.0 UART.
//!
//! Each log line produced by `live_telemetry_loop` already stores the
//! gain-scaled, lane-rotated stimulus vector for channels 0-4 (channels
//! 5-15 were always zero), so replay is an exact reconstruction: the logged
//! `stim` array is re-sent verbatim through `FpgaBridge::exchange`. This
//! lets on-chip STDP train on a previously captured gameplay session
//! without the game (or any GPU load) running.
//!
//! Requires the `uart` feature, an explicit serial port, and an input log:
//!
//! ```text
//! SILICON_BRIDGE_PORT=COM3 REPLAY_LOG=live-fpga-log-kcd2.jsonl \
//!   cargo run --release --features uart --example replay_log
//! ```
//!
//! Environment: `SILICON_BRIDGE_PORT`, `SILICON_BRIDGE_BAUD`, `TICK_MS`
//! (inter-tick delay, default 100), `REPLAY_LOG` (input JSONL, required),
//! `LIVE_LOG` (output log of replay responses, default
//! `replay-fpga-log.jsonl`), `REPLAY_LOOPS` (passes over the input,
//! default 1). Event lines (`poll_fail`, `exchange_error`) in the input are
//! skipped. The replay output log uses the same schema as the live loop so
//! downstream analysis stays compatible.

#[cfg(not(feature = "uart"))]
fn main() {
    eprintln!(
        "replay_log requires the `uart` feature: \
         cargo run --release --features uart --example replay_log"
    );
    std::process::exit(2);
}

#[cfg(feature = "uart")]
fn main() {
    std::process::exit(run());
}

/// Extract the five recorded stimulus values from one JSONL sample line.
/// Accepts lines shaped like `{"tick":N,"stim":[v0..v4],...}` and returns
/// `None` for event lines or anything unparseable — no serde dependency.
#[cfg(feature = "uart")]
fn parse_stim_line(line: &str) -> Option<[f32; 5]> {
    let key = "\"stim\":[";
    let start = line.find(key)? + key.len();
    let end = line[start..].find(']')? + start;
    let mut out = [0.0_f32; 5];
    let mut count = 0;
    for part in line[start..end].split(',') {
        if count == 5 {
            return None;
        }
        out[count] = part.trim().parse().ok()?;
        count += 1;
    }
    (count == 5).then_some(out)
}

#[cfg(feature = "uart")]
fn run() -> i32 {
    use silicon_bridge::{DenseQ88Layout, FpgaBridge};
    use std::io::{BufRead, Write};
    use std::time::{Duration, Instant};

    let port = std::env::args()
        .nth(1)
        .filter(|p| !p.trim().is_empty())
        .or_else(|| {
            std::env::var("SILICON_BRIDGE_PORT")
                .ok()
                .filter(|p| !p.trim().is_empty())
        });
    let port = match port {
        Some(p) => p,
        None => {
            eprintln!("==> no serial port provided (pass an argument or set SILICON_BRIDGE_PORT)");
            return 2;
        }
    };
    let baud = match std::env::var("SILICON_BRIDGE_BAUD") {
        Ok(b) => match b.trim().parse::<u32>() {
            Ok(v) if v > 0 => v,
            _ => {
                eprintln!("==> invalid SILICON_BRIDGE_BAUD={b:?}");
                return 2;
            }
        },
        Err(_) => silicon_bridge::DEFAULT_BAUD_RATE,
    };
    let tick_ms: u64 = std::env::var("TICK_MS")
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(100)
        .max(20);
    let loops: u64 = std::env::var("REPLAY_LOOPS")
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(1)
        .max(1);
    let input_path = match std::env::var("REPLAY_LOG") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => {
            eprintln!("==> REPLAY_LOG (input JSONL) is required");
            return 2;
        }
    };
    let log_path = std::env::var("LIVE_LOG").unwrap_or_else(|_| "replay-fpga-log.jsonl".into());

    // Load the recorded stimuli up front so a truncated/unreadable input
    // fails before we touch the serial port.
    let input = match std::fs::File::open(&input_path) {
        Ok(f) => std::io::BufReader::new(f),
        Err(e) => {
            eprintln!("==> cannot open {input_path}: {e}");
            return 1;
        }
    };
    let mut samples: Vec<[f32; 5]> = Vec::new();
    for line in input.lines() {
        match line {
            Ok(l) => {
                if let Some(stim) = parse_stim_line(&l) {
                    samples.push(stim);
                }
            }
            Err(e) => {
                eprintln!("==> read error in {input_path}: {e}");
                return 1;
            }
        }
    }
    if samples.is_empty() {
        eprintln!("==> no stimulus samples found in {input_path}");
        return 1;
    }
    eprintln!(
        "==> replay log: port={port} baud={baud} tick={tick_ms}ms \
         input={input_path} samples={} loops={loops} out={log_path}",
        samples.len()
    );

    let mut bridge = match FpgaBridge::builder(&port).baud_rate(baud).open() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("==> open failed: {e}");
            return 1;
        }
    };
    if let Err(e) = bridge.recover() {
        eprintln!("==> initial RX drain failed: {e}");
        return 1;
    }

    let mut log = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("==> cannot open {log_path}: {e}");
            return 1;
        }
    };

    let layout = DenseQ88Layout::silicon_bridge_v3();
    let mut tick: u64 = 0;
    let mut errors: u64 = 0;
    for pass in 1..=loops {
        for stim in &samples {
            let start = Instant::now();
            tick += 1;
            let mut stimuli = vec![0.0_f32; layout.input_channels()];
            stimuli[..5].copy_from_slice(stim);

            match bridge.exchange(&layout, &stimuli) {
                Ok(resp) => {
                    errors = 0;
                    let n_spikes = resp.spikes.iter().filter(|b| **b).count();
                    let pot_min = resp
                        .potentials
                        .iter()
                        .cloned()
                        .reduce(f32::min)
                        .unwrap_or(0.0);
                    let pot_max = resp
                        .potentials
                        .iter()
                        .cloned()
                        .reduce(f32::max)
                        .unwrap_or(0.0);
                    let line = format!(
                        "{{\"tick\":{tick},\"pass\":{pass},\"stim\":[{:.4},{:.4},{:.4},{:.4},{:.4}],\"spikes\":{n_spikes},\"pot_min\":{pot_min:.4},\"pot_max\":{pot_max:.4}}}",
                        stimuli[0], stimuli[1], stimuli[2], stimuli[3], stimuli[4],
                    );
                    if let Err(e) = writeln!(log, "{line}") {
                        eprintln!("==> log write failed at tick {tick}: {e}");
                        return 1;
                    }
                    if tick.is_multiple_of(500) {
                        eprintln!("==> tick {tick} (pass {pass}/{loops}): spikes={n_spikes}");
                    }
                }
                Err(e) => {
                    errors += 1;
                    eprintln!("==> exchange failed at tick {tick}: {e} (errors={errors})");
                    let _ = writeln!(log, "{{\"tick\":{tick},\"pass\":{pass},\"event\":\"exchange_error\"}}");
                    if bridge.needs_recovery()
                        && let Err(re) = bridge.recover()
                    {
                        eprintln!("==> recover failed: {re}");
                        return 1;
                    }
                    if errors > 50 {
                        eprintln!("==> too many exchange errors, giving up");
                        return 1;
                    }
                }
            }
            if let Err(e) = log.flush() {
                eprintln!("==> log flush failed at tick {tick}: {e}");
                return 1;
            }

            let elapsed = start.elapsed();
            if elapsed < Duration::from_millis(tick_ms) {
                std::thread::sleep(Duration::from_millis(tick_ms) - elapsed);
            }
        }
    }
    eprintln!("==> replay complete: {tick} ticks over {loops} pass(es)");
    0
}

#[cfg(all(test, feature = "uart"))]
mod tests {
    use super::*;

    #[test]
    fn parses_live_loop_sample_line() {
        let line = "{\"tick\":82310,\"stim\":[0.6946,0.0000,0.0000,0.0000,0.0000],\"gain\":2.50,\"spikes\":0,\"pot_min\":-1.0000,\"pot_max\":1.2344}";
        let stim = parse_stim_line(line).unwrap();
        assert!((stim[0] - 0.6946).abs() < 1e-6);
        assert_eq!(stim[1..], [0.0; 4]);
    }

    #[test]
    fn parses_rotated_lane() {
        let line = "{\"tick\":82312,\"stim\":[0.0000,0.0000,1.4467,0.0000,0.0000],\"gain\":2.50,\"spikes\":2}";
        let stim = parse_stim_line(line).unwrap();
        assert!((stim[2] - 1.4467).abs() < 1e-6);
    }

    #[test]
    fn rejects_event_and_malformed_lines() {
        assert!(parse_stim_line("{\"tick\":5,\"event\":\"poll_fail\"}").is_none());
        assert!(parse_stim_line("{\"stim\":[1.0,2.0]}").is_none());
        assert!(parse_stim_line("{\"stim\":[a,0,0,0,0]}").is_none());
        assert!(parse_stim_line("").is_none());
    }
}
