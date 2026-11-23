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
//! exchange is logged as one JSONL line appended to the path in `LIVE_LOG`,
//! carrying the raw poll in physical units (`raw`, in `LIVE_RANGES` order)
//! alongside the encoded `stim` lanes.
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
//! (nvidia-smi binary override), `STIMULUS_GAIN` (default 2.5; non-finite
//! or unparseable values fall back to the default, negatives clamp to 0).
//!
//! The flashed SoC decodes only the lowest nonzero stimulus lane of each
//! `0xAA` frame as one input event and ignores its amplitude (silicon-hdl#112).
//! The loop therefore rotates one live lane per sent sample so every
//! telemetry axon fires in turn. `STIMULUS_GAIN` scales the value carried on
//! the active lane; it does not change on-chip membrane drive, it only
//! affects the logged value. The active lane is floored at one Q8.8 step, so
//! any positive gain still produces an event.

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
    if f[1] <= 0.0 || f.iter().any(|v| !v.is_finite()) {
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

/// Default `STIMULUS_GAIN` when the env var is unset or invalid.
#[cfg(feature = "uart")]
const DEFAULT_STIMULUS_GAIN: f32 = 2.5;

/// Smallest positive Q8.8 value; keeps the active lane nonzero after encoding.
#[cfg(feature = "uart")]
const Q88_STEP: f32 = 1.0 / 256.0;

/// Parse a `STIMULUS_GAIN` value. Rejects non-finite input; clamps negatives to 0.
#[cfg(feature = "uart")]
fn parse_gain(raw: &str) -> Option<f32> {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|g| g.is_finite())
        .map(|g| g.max(0.0))
}

/// Build one frame's stimulus vector for the `seq`-th sent sample: only lane
/// `seq % LIVE_RANGES.len()` is nonzero, because the SoC decodes just the
/// lowest nonzero lane per frame. The active lane carries the gain-scaled
/// normalized value, floored at one Q8.8 step so it always encodes as an
/// event (a gain of 0 disables stimulation).
#[cfg(feature = "uart")]
fn live_stimuli(seq: u64, norm: &[f32; 5], gain: f32, channels: usize) -> Vec<f32> {
    let mut stimuli = vec![0.0_f32; channels];
    let lane = (seq % LIVE_RANGES.len() as u64) as usize;
    if let Some(slot) = stimuli.get_mut(lane)
        && gain > 0.0
    {
        *slot = (norm[lane] * gain).max(Q88_STEP);
    }
    stimuli
}

/// Value the FPGA actually receives for `v` after signed Q8.8 encoding.
#[cfg(feature = "uart")]
fn as_sent(v: f32) -> f32 {
    silicon_bridge::q88_signed_to_f32(silicon_bridge::encode_q88_signed(v))
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
    let stimulus_gain: f32 = std::env::var("STIMULUS_GAIN")
        .ok()
        .and_then(|g| parse_gain(&g))
        .unwrap_or(DEFAULT_STIMULUS_GAIN);
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
    let mut sent: u64 = 0;
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
        poll_errors = 0;
        let norm = normalize(fields);
        // Rotate on delivered samples, not ticks, so failed polls or exchanges
        // never skip a lane.
        let stimuli = live_stimuli(sent, &norm, stimulus_gain, layout.input_channels());

        match bridge.exchange(&layout, &stimuli) {
            Ok(resp) => {
                sent += 1;
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
                    "{{\"tick\":{tick},\"raw\":[{:.4},{:.4},{:.4},{:.4},{:.4}],\"stim\":[{},{},{},{},{}],\"gain\":{stimulus_gain},\"spikes\":{n_spikes},\"pot_min\":{pot_min:.4},\"pot_max\":{pot_max:.4}}}",
                    fields[0],
                    fields[1],
                    fields[2],
                    fields[3],
                    fields[4],
                    as_sent(stimuli.first().copied().unwrap_or(0.0)),
                    as_sent(stimuli.get(1).copied().unwrap_or(0.0)),
                    as_sent(stimuli.get(2).copied().unwrap_or(0.0)),
                    as_sent(stimuli.get(3).copied().unwrap_or(0.0)),
                    as_sent(stimuli.get(4).copied().unwrap_or(0.0)),
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
        assert!(parse_gpu_row("12000, 24576, inf, 62, 1980, 9501").is_none());
        assert!(parse_gpu_row("12000, 24576, NaN, 62, 1980, 9501").is_none());
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

    #[test]
    fn live_stimuli_rotates_one_lane_per_tick() {
        let norm = [0.1, 0.2, 0.3, 0.4, 0.5];
        let mut hits = [0_u32; 5];
        for seq in 0..10_u64 {
            let s = live_stimuli(seq, &norm, 2.0, 16);
            assert_eq!(s.len(), 16);
            let active: Vec<usize> = (0..16).filter(|&i| s[i] != 0.0).collect();
            assert_eq!(active, vec![(seq % 5) as usize]);
            let lane = active[0];
            assert!((s[lane] - norm[lane] * 2.0).abs() < 1e-6);
            hits[lane] += 1;
        }
        assert_eq!(hits, [2; 5]);
    }

    #[test]
    fn live_stimuli_floors_zero_lane_to_one_q88_step() {
        let s = live_stimuli(0, &[0.0; 5], 1.0, 16);
        assert_eq!(s[0], Q88_STEP);
        assert!(s[1..].iter().all(|v| *v == 0.0));
        let s = live_stimuli(1, &[0.001; 5], 0.5, 16);
        assert_eq!(s[1], Q88_STEP);
        assert_eq!(as_sent(s[1]), Q88_STEP);
        assert!(
            live_stimuli(0, &[0.0; 5], 0.0, 16)
                .iter()
                .all(|v| *v == 0.0)
        );
    }

    #[test]
    fn live_stimuli_ignores_lanes_beyond_layout() {
        assert_eq!(live_stimuli(4, &[1.0; 5], 1.0, 3), vec![0.0; 3]);
    }

    #[test]
    fn as_sent_matches_q88_truncation_and_saturation() {
        assert_eq!(as_sent(0.0025), 0.0);
        assert_eq!(as_sent(1.0), 1.0);
        assert_eq!(as_sent(1000.0), 32765.0 / 256.0);
    }

    #[test]
    fn parse_gain_rejects_non_finite_and_clamps_negative() {
        assert_eq!(parse_gain(" 3.5 "), Some(3.5));
        assert_eq!(parse_gain("-1"), Some(0.0));
        assert_eq!(parse_gain("inf"), None);
        assert_eq!(parse_gain("-inf"), None);
        assert_eq!(parse_gain("NaN"), None);
        assert_eq!(parse_gain("abc"), None);
    }
}
