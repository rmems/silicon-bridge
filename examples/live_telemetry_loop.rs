// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live telemetry host loop: streams normalized GPU telemetry down the
//! SiliconBridge v3.0 UART as 16-channel signed-Q8.8 stimuli so the flashed
//! `spikenaut_soc_basys3_top` StdpController (SW14) learns on-chip while the
//! host plays.
//!
//! Channels 0-4 carry the five live contract columns in `Spikenaut-SNN`'s
//! live map order (mem_util_pct, power_w, gpu_temp_c, sm_clock_mhz,
//! mem_clock_mhz), normalized to 0..1 by the frozen v3 min/max ranges.
//! Channels 5-15 stay zero, matching the shipped bank's axon usage.
//!
//! Telemetry comes from `nvidia-smi` (one subprocess poll per tick). Each
//! exchange is logged as one JSONL line to the path in `LIVE_LOG` (default
//! `live-fpga-log.jsonl` in the current directory).
//!
//! Requires the `uart` feature and an explicit serial port:
//!
//! ```text
//! SILICON_BRIDGE_PORT=COM3 cargo run --release --features uart --example live_telemetry_loop
//! SILICON_BRIDGE_PORT=COM3 TICK_MS=500 LIVE_LOG=C:\path\live.jsonl \
//!   cargo run --release --features uart --example live_telemetry_loop
//! ```

#[cfg(not(feature = "uart"))]
fn main() {
    eprintln!(
        "live_telemetry_loop requires the `uart` feature: \
         cargo run --release --features uart --example live_telemetry_loop"
    );
    std::process::exit(2);
}

#[cfg(feature = "uart")]
fn main() {
    std::process::exit(run());
}

/// Frozen v3 normalization ranges from the Spikenaut telemetry contract.
/// (column name, min, max) in live-map order.
#[cfg(feature = "uart")]
const LIVE_RANGES: [(&str, f32, f32); 5] = [
    ("mem_util_pct", 0.0, 75.0),
    ("power_w", 8.5270004, 302.845),
    ("gpu_temp_c", 0.0, 69.0),
    ("sm_clock_mhz", 180.0, 2910.0),
    ("mem_clock_mhz", 405.0, 14801.0),
];

#[cfg(feature = "uart")]
fn run() -> i32 {
    use silicon_bridge::{DenseQ88Layout, FpgaBridge};
    use std::io::Write;
    use std::time::{Duration, Instant};

    let port = std::env::var("SILICON_BRIDGE_PORT")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .or_else(|| std::env::args().nth(1).filter(|p| !p.trim().is_empty()));
    let port = match port {
        Some(p) => p,
        None => {
            eprintln!("==> no serial port provided (set SILICON_BRIDGE_PORT or pass an argument)");
            return 2;
        }
    };
    let baud = std::env::var("SILICON_BRIDGE_BAUD")
        .ok()
        .and_then(|b| b.trim().parse::<u32>().ok())
        .filter(|b| *b > 0)
        .unwrap_or(silicon_bridge::DEFAULT_BAUD_RATE);
    let tick_ms: u64 = std::env::var("TICK_MS")
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(500)
        .max(50);
    let log_path = std::env::var("LIVE_LOG").unwrap_or_else(|_| "live-fpga-log.jsonl".into());

    eprintln!("==> live telemetry loop: port={port} baud={baud} tick={tick_ms}ms log={log_path}");

    let mut bridge = match FpgaBridge::builder(&port).baud_rate(baud).open() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("==> open failed: {e}");
            return 1;
        }
    };
    if let Err(e) = bridge.recover() {
        eprintln!("==> initial RX drain failed: {e}");
    }

    let mut log = match std::fs::File::create(&log_path) {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("==> cannot open {log_path}: {e}");
            return 1;
        }
    };

    let layout = DenseQ88Layout::silicon_bridge_v3();
    let mut tick: u64 = 0;
    let mut errors: u64 = 0;
    loop {
        let start = Instant::now();
        tick += 1;

        let stimuli = match poll_gpu() {
            Some(v) => {
                let mut s = vec![0.0_f32; layout.input_channels()];
                for (i, val) in v.iter().enumerate() {
                    let (_, lo, hi) = LIVE_RANGES[i];
                    s[i] = ((val - lo) / (hi - lo)).clamp(0.0, 1.0);
                }
                s
            }
            None => vec![0.0_f32; layout.input_channels()],
        };

        match bridge.exchange(&layout, &stimuli) {
            Ok(resp) => {
                let n_spikes = resp.spikes.iter().filter(|b| **b).count();
                let line = format!(
                    "{{\"tick\":{tick},\"stim\":[{:.4},{:.4},{:.4},{:.4},{:.4}],\"spikes\":{n_spikes},\"pot_min\":{:.4},\"pot_max\":{:.4}}}",
                    stimuli[0],
                    stimuli[1],
                    stimuli[2],
                    stimuli[3],
                    stimuli[4],
                    resp.potentials
                        .iter()
                        .cloned()
                        .fold(f32::INFINITY, f32::min),
                    resp.potentials
                        .iter()
                        .cloned()
                        .fold(f32::NEG_INFINITY, f32::max),
                );
                if writeln!(log, "{line}").is_err() {
                    eprintln!("==> log write failed at tick {tick}");
                }
                if tick.is_multiple_of(120) {
                    eprintln!(
                        "==> tick {tick}: spikes={n_spikes} stim={:?}",
                        &stimuli[..5]
                    );
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!("==> exchange failed at tick {tick}: {e} (errors={errors})");
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
        let _ = log.flush();

        let elapsed = start.elapsed();
        if elapsed < Duration::from_millis(tick_ms) {
            std::thread::sleep(Duration::from_millis(tick_ms) - elapsed);
        }
    }
}

/// Poll the five live GPU fields via `nvidia-smi`.
/// Returns [mem_util_pct, power_w, gpu_temp_c, sm_clock_mhz, mem_clock_mhz].
#[cfg(feature = "uart")]
fn poll_gpu() -> Option<[f32; 5]> {
    use std::process::Command;
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total,power.draw,temperature.gpu,clocks.sm,clocks.mem",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<f32> = text
        .trim()
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if parts.len() < 6 || parts[1] <= 0.0 {
        return None;
    }
    Some([
        100.0 * parts[0] / parts[1], // mem.used/mem.total -> pct
        parts[2],
        parts[3],
        parts[4],
        parts[5],
    ])
}
