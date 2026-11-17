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
//! exchange is logged as one JSONL line appended to the path in `LIVE_LOG`
//! (default `live-fpga-log.jsonl` in the current directory). Ticks where the
//! GPU poll or the UART exchange fails are logged as `poll_fail` /
//! `exchange_error` events instead of being reported as valid samples.
//!
//! Requires the `uart` feature and an explicit serial port (positional
//! argument wins over `SILICON_BRIDGE_PORT`):
//!
//! ```text
//! SILICON_BRIDGE_PORT=COM3 cargo run --release --features uart --example live_telemetry_loop
//! SILICON_BRIDGE_PORT=COM3 TICK_MS=500 LIVE_LOG=C:\path\live.jsonl \
//!   cargo run --release --features uart --example live_telemetry_loop
//! ```
//!
//! Environment: `SILICON_BRIDGE_PORT`, `SILICON_BRIDGE_BAUD`, `TICK_MS`,
//! `LIVE_LOG`, `GPU_INDEX` (nvidia-smi `--id`, default 0), `NVIDIA_SMI`
//! (nvidia-smi binary override).

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
    ("power_w", 8.527, 302.845),
    ("gpu_temp_c", 0.0, 69.0),
    ("sm_clock_mhz", 180.0, 2910.0),
    ("mem_clock_mhz", 405.0, 14801.0),
];

/// Parse one `nvidia-smi --format=csv,noheader,nounits` row with columns
/// `memory.used,memory.total,power.draw,temperature.gpu,clocks.sm,clocks.mem`.
/// Returns [mem_util_pct, power_w, gpu_temp_c, sm_clock_mhz, mem_clock_mhz],
/// or `None` if any field is missing or unparseable.
#[cfg(feature = "uart")]
fn parse_gpu_row(line: &str) -> Option<[f32; 5]> {
    let cols: Vec<&str> = line.split(',').map(str::trim).collect();
    if cols.len() != 6 {
        return None;
    }
    let mut f = [0.0_f32; 6];
    for (dst, src) in f.iter_mut().zip(cols) {
        *dst = src.parse().ok()?;
    }
    if f[1] <= 0.0 {
        return None;
    }
    Some([100.0 * f[0] / f[1], f[2], f[3], f[4], f[5]])
}

/// Minmax-normalize the five live fields onto 0..1 using `LIVE_RANGES`.
#[cfg(feature = "uart")]
fn normalize(fields: [f32; 5]) -> [f32; 5] {
    let mut out = [0.0_f32; 5];
    for ((o, v), (_, lo, hi)) in out.iter_mut().zip(fields).zip(LIVE_RANGES) {
        *o = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    }
    out
}

/// Poll the live GPU fields via `nvidia-smi` for `GPU_INDEX` (default 0).
/// Returns [mem_util_pct, power_w, gpu_temp_c, sm_clock_mhz, mem_clock_mhz].
#[cfg(feature = "uart")]
fn poll_gpu() -> Option<[f32; 5]> {
    use std::process::Command;
    let bin = std::env::var("NVIDIA_SMI").unwrap_or_else(|_| "nvidia-smi".into());
    let gpu = std::env::var("GPU_INDEX").unwrap_or_else(|_| "0".into());
    let out = Command::new(bin)
        .args([
            &format!("--id={gpu}"),
            "--query-gpu=memory.used,memory.total,power.draw,temperature.gpu,clocks.sm,clocks.mem",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_gpu_row(text.trim().lines().next()?)
}

#[cfg(feature = "uart")]
fn run() -> i32 {
    use silicon_bridge::{DenseQ88Layout, FpgaBridge};
    use std::io::Write;
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
    let mut poll_errors: u64 = 0;
    loop {
        let start = Instant::now();
        tick += 1;

        let Some(fields) = poll_gpu() else {
            poll_errors += 1;
            eprintln!("==> GPU poll failed at tick {tick} (poll_errors={poll_errors})");
            let _ = writeln!(log, "{{\"tick\":{tick},\"event\":\"poll_fail\"}}");
            let _ = log.flush();
            if poll_errors > 50 {
                eprintln!("==> too many poll failures, giving up");
                return 1;
            }
            let elapsed = start.elapsed();
            if elapsed < Duration::from_millis(tick_ms) {
                std::thread::sleep(Duration::from_millis(tick_ms) - elapsed);
            }
            continue;
        };
        let norm = normalize(fields);
        let mut stimuli = vec![0.0_f32; layout.input_channels()];
        stimuli[..5].copy_from_slice(&norm);

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
                    "{{\"tick\":{tick},\"stim\":[{:.4},{:.4},{:.4},{:.4},{:.4}],\"spikes\":{n_spikes},\"pot_min\":{pot_min:.4},\"pot_max\":{pot_max:.4}}}",
                    norm[0], norm[1], norm[2], norm[3], norm[4],
                );
                if let Err(e) = writeln!(log, "{line}") {
                    eprintln!("==> log write failed at tick {tick}: {e}");
                    return 1;
                }
                if tick.is_multiple_of(120) {
                    eprintln!("==> tick {tick}: spikes={n_spikes} stim={norm:?}");
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!("==> exchange failed at tick {tick}: {e} (errors={errors})");
                let _ = writeln!(log, "{{\"tick\":{tick},\"event\":\"exchange_error\"}}");
                if bridge.needs_recovery() {
                    match bridge.recover() {
                        Ok(()) => errors = 0,
                        Err(re) => {
                            eprintln!("==> recover failed: {re}");
                            return 1;
                        }
                    }
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

#[cfg(all(test, feature = "uart"))]
mod tests {
    use super::*;

    #[test]
    fn parse_gpu_row_happy_path() {
        let row = parse_gpu_row("12000, 24576, 185.5, 62, 1980, 9501").unwrap();
        assert!((row[0] - 100.0 * 12000.0 / 24576.0).abs() < 1e-3);
        assert_eq!(row[1], 185.5);
        assert_eq!(row[2], 62.0);
        assert_eq!(row[3], 1980.0);
        assert_eq!(row[4], 9501.0);
    }

    #[test]
    fn parse_gpu_row_rejects_bad_fields() {
        assert!(parse_gpu_row("12000, 24576, N/A, 62, 1980, 9501").is_none());
        assert!(parse_gpu_row("12000, 0, 185.5, 62, 1980, 9501").is_none());
        assert!(parse_gpu_row("12000, 24576, 185.5, 62, 1980").is_none());
        assert!(parse_gpu_row("12000, 24576, 185.5, 62, 1980, 9501, extra").is_none());
        assert!(parse_gpu_row("").is_none());
    }

    #[test]
    fn normalize_clamps_to_unit() {
        let n = normalize([75.0, 302.845, 69.0, 2910.0, 14801.0]);
        assert!(n.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!((n[0] - 1.0).abs() < 1e-6);
        let n = normalize([0.0, 8.527, 0.0, 180.0, 405.0]);
        assert!(n.iter().all(|v| *v == 0.0));
        let n = normalize([200.0, 500.0, 90.0, 4000.0, 20000.0]);
        assert!(n.iter().all(|v| *v == 1.0));
    }
}
