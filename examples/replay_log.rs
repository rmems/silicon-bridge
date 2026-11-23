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
//! (inter-tick delay in ms, default 100), `REPLAY_LOG` (input JSONL,
//! required), `LIVE_LOG` (output log of replay responses, default
//! `replay-fpga-log.jsonl`), `REPLAY_LOOPS` (passes over the input,
//! default 1). An unparseable `SILICON_BRIDGE_BAUD`, `TICK_MS`, or
//! `REPLAY_LOOPS` is an error, not a silent fallback to the default.
//!
//! Input handling: blank lines and `poll_fail` / `exchange_error` event
//! lines are skipped; any other line that fails to parse as a sample
//! (a truncated or malformed record) aborts the run before the serial
//! port opens. Captures written before the live loop logged `gain` are
//! replayed with a warning — their `stim` values were rounded to four
//! decimals, so the recorded wire precision cannot be recovered and the
//! replay is approximate.
//!
//! The output log is truncated on each run (never appended), and the run
//! refuses to start when `LIVE_LOG` resolves to the same file as
//! `REPLAY_LOG`. Output records carry the same fields as the live loop
//! (`stim`, `gain`, `spikes`, `pot_min`, `pot_max`) plus `pass`; `stim`
//! holds the Q8.8 values actually sent, at full precision, so a replay
//! log round-trips the exact wire values when used as input again.
//! `gain` is `null` when replaying legacy captures that predate the
//! field.

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

/// One parsed input sample: the five recorded stimulus lanes plus the
/// `gain` field the live loop writes. `gain` is absent in captures from
/// before the field existed, which also logged `stim` rounded to four
/// decimals — those replay approximately, not exactly.
#[cfg(feature = "uart")]
struct ReplaySample {
    stim: [f32; 5],
    gain: Option<f32>,
}

/// Find `key` (a JSON field name like `"stim":[`) only at a real key
/// boundary — the preceding non-whitespace byte must be `{` or `,` — so
/// a same-named field nested elsewhere in the line cannot match.
#[cfg(feature = "uart")]
fn find_key(line: &str, key: &str) -> Option<usize> {
    let mut offset = 0;
    loop {
        let pos = line[offset..].find(key)? + offset;
        if line[..pos]
            .bytes()
            .rev()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| b == b'{' || b == b',')
        {
            return Some(pos);
        }
        offset = pos + key.len();
    }
}

/// Extract one recorded sample from a JSONL line. Accepts lines shaped
/// like `{"tick":N,"stim":[v0..v4],...}` and returns `None` for event
/// lines or anything unparseable — no serde dependency. Non-finite
/// stimulus values (`NaN`, `inf`) are rejected: `f32::parse` accepts
/// them but `encode_stimuli` would refuse them at exchange time.
#[cfg(feature = "uart")]
fn parse_stim_line(line: &str) -> Option<ReplaySample> {
    let key = "\"stim\":[";
    let start = find_key(line, key)? + key.len();
    let end = line[start..].find(']')? + start;
    let mut stim = [0.0_f32; 5];
    let mut count = 0;
    for part in line[start..end].split(',') {
        if count == 5 {
            return None;
        }
        let v: f32 = part.trim().parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        stim[count] = v;
        count += 1;
    }
    if count != 5 {
        return None;
    }
    Some(ReplaySample {
        stim,
        gain: parse_gain_field(line),
    })
}

/// Parse the `gain` field of a sample line, when present and finite.
/// `null` (emitted by this example for legacy inputs) parses as absent.
#[cfg(feature = "uart")]
fn parse_gain_field(line: &str) -> Option<f32> {
    let key = "\"gain\":";
    let start = find_key(line, key)? + key.len();
    let rest = &line[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end]
        .trim()
        .parse()
        .ok()
        .filter(|v: &f32| v.is_finite())
}

/// Event records (`poll_fail`, `exchange_error`) carry no stimulus and
/// are skipped during input loading.
#[cfg(feature = "uart")]
fn is_event_line(line: &str) -> bool {
    find_key(line, "\"event\":").is_some()
}

/// Value the FPGA actually receives for `v` after signed Q8.8 encoding.
#[cfg(feature = "uart")]
fn as_sent(v: f32) -> f32 {
    silicon_bridge::q88_signed_to_f32(silicon_bridge::encode_q88_signed(v))
}

/// Format one replay output record: the live-loop sample fields plus
/// `pass`, with `stim` written at full precision so a replay log
/// re-parses to the exact wire values. `gain` is `null` for legacy
/// captures that predate the field.
#[cfg(feature = "uart")]
fn replay_line(
    tick: u64,
    pass: u64,
    stimuli: &[f32],
    gain: Option<f32>,
    n_spikes: usize,
    pot_min: f32,
    pot_max: f32,
) -> String {
    let gain = gain.map_or_else(|| "null".to_string(), |g| format!("{g}"));
    format!(
        "{{\"tick\":{tick},\"pass\":{pass},\"stim\":[{},{},{},{},{}],\"gain\":{gain},\"spikes\":{n_spikes},\"pot_min\":{pot_min:.4},\"pot_max\":{pot_max:.4}}}",
        as_sent(stimuli.first().copied().unwrap_or(0.0)),
        as_sent(stimuli.get(1).copied().unwrap_or(0.0)),
        as_sent(stimuli.get(2).copied().unwrap_or(0.0)),
        as_sent(stimuli.get(3).copied().unwrap_or(0.0)),
        as_sent(stimuli.get(4).copied().unwrap_or(0.0)),
    )
}

/// Canonicalize `path`; when it does not exist yet, canonicalize the
/// parent and re-attach the file name so a not-yet-created output log
/// still compares correctly against the input path.
#[cfg(feature = "uart")]
fn resolved_path(path: &std::path::Path) -> std::path::PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(p) = std::fs::canonicalize(parent)
    {
        return p.join(name);
    }
    path.to_path_buf()
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
    let tick_ms: u64 = match std::env::var("TICK_MS") {
        Ok(t) => match t.trim().parse::<u64>() {
            Ok(v) if v > 0 => v.max(20),
            _ => {
                eprintln!("==> invalid TICK_MS={t:?}");
                return 2;
            }
        },
        Err(_) => 100,
    };
    let loops: u64 = match std::env::var("REPLAY_LOOPS") {
        Ok(t) => match t.trim().parse::<u64>() {
            Ok(v) if v > 0 => v,
            _ => {
                eprintln!("==> invalid REPLAY_LOOPS={t:?}");
                return 2;
            }
        },
        Err(_) => 1,
    };
    let input_path = match std::env::var("REPLAY_LOG") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => {
            eprintln!("==> REPLAY_LOG (input JSONL) is required");
            return 2;
        }
    };
    let log_path = std::env::var("LIVE_LOG").unwrap_or_else(|_| "replay-fpga-log.jsonl".into());

    if resolved_path(std::path::Path::new(&input_path))
        == resolved_path(std::path::Path::new(&log_path))
    {
        eprintln!(
            "==> LIVE_LOG={log_path} resolves to the same file as REPLAY_LOG={input_path}; \
             replay output would corrupt the input capture"
        );
        return 2;
    }

    // Load the recorded stimuli up front so a truncated/unreadable input
    // fails before we touch the serial port. Blank lines and known event
    // records are skipped; anything else that fails to parse aborts.
    let input = match std::fs::File::open(&input_path) {
        Ok(f) => std::io::BufReader::new(f),
        Err(e) => {
            eprintln!("==> cannot open {input_path}: {e}");
            return 1;
        }
    };
    let mut samples: Vec<ReplaySample> = Vec::new();
    let mut skipped: u64 = 0;
    for (idx, line) in input.lines().enumerate() {
        let lineno = idx + 1;
        let l = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("==> read error in {input_path}:{lineno}: {e}");
                return 1;
            }
        };
        if l.trim().is_empty() || is_event_line(&l) {
            skipped += 1;
            continue;
        }
        match parse_stim_line(&l) {
            Some(s) => samples.push(s),
            None => {
                eprintln!("==> malformed sample line in {input_path}:{lineno}: {l}");
                return 1;
            }
        }
    }
    if samples.is_empty() {
        eprintln!("==> no stimulus samples found in {input_path}");
        return 1;
    }
    let legacy = samples.iter().filter(|s| s.gain.is_none()).count();
    if legacy > 0 {
        eprintln!(
            "==> warning: {legacy}/{} samples carry no `gain` field (capture predates the \
             gain/full-precision schema); their rounded stim values replay approximately \
             and events below one Q8.8 step may be lost",
            samples.len()
        );
    }
    eprintln!(
        "==> replay log: port={port} baud={baud} tick={tick_ms}ms \
         input={input_path} samples={} skipped={skipped} loops={loops} out={log_path}",
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
        .truncate(true)
        .write(true)
        .open(&log_path)
    {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("==> cannot open {log_path}: {e}");
            return 1;
        }
    };

    let layout = DenseQ88Layout::silicon_bridge_v3();
    if layout.input_channels() < 5 {
        eprintln!(
            "==> layout has {} input channels, need at least 5 to replay",
            layout.input_channels()
        );
        return 1;
    }
    let mut tick: u64 = 0;
    let mut errors: u64 = 0;
    let mut total_errors: u64 = 0;
    let mut delivered: u64 = 0;
    for pass in 1..=loops {
        for sample in &samples {
            let start = Instant::now();
            tick += 1;
            let mut stimuli = vec![0.0_f32; layout.input_channels()];
            stimuli[..5].copy_from_slice(&sample.stim);

            match bridge.exchange(&layout, &stimuli) {
                Ok(resp) => {
                    delivered += 1;
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
                    let line = replay_line(
                        tick,
                        pass,
                        &stimuli,
                        sample.gain,
                        n_spikes,
                        pot_min,
                        pot_max,
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
                    total_errors += 1;
                    eprintln!("==> exchange failed at tick {tick}: {e} (errors={errors})");
                    if let Err(we) = writeln!(
                        log,
                        "{{\"tick\":{tick},\"pass\":{pass},\"event\":\"exchange_error\"}}"
                    ) {
                        eprintln!("==> log write failed at tick {tick}: {we}");
                        return 1;
                    }
                    if bridge.needs_recovery()
                        && let Err(re) = bridge.recover()
                    {
                        eprintln!("==> recover failed: {re}");
                        return 1;
                    }
                    if errors > 50 {
                        eprintln!("==> too many consecutive exchange errors, giving up");
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
    eprintln!(
        "==> replay complete: {delivered} delivered, {total_errors} failed, \
         {tick} ticks over {loops} pass(es)"
    );
    if delivered == 0 {
        eprintln!("==> no successful FPGA exchanges");
        return 1;
    }
    0
}

#[cfg(all(test, feature = "uart"))]
mod tests {
    use super::*;

    #[test]
    fn parses_live_loop_sample_line() {
        let line = "{\"tick\":82310,\"stim\":[0.6946,0.0000,0.0000,0.0000,0.0000],\"gain\":2.50,\"spikes\":0,\"pot_min\":-1.0000,\"pot_max\":1.2344}";
        let s = parse_stim_line(line).unwrap();
        assert!((s.stim[0] - 0.6946).abs() < 1e-6);
        assert_eq!(s.stim[1..], [0.0; 4]);
        assert_eq!(s.gain, Some(2.5));
    }

    #[test]
    fn parses_rotated_lane() {
        let line =
            "{\"tick\":82312,\"stim\":[0.0000,1.4467,0.0000,0.5,0.125],\"gain\":2.50,\"spikes\":2}";
        let s = parse_stim_line(line).unwrap();
        assert_eq!(s.stim[0], 0.0);
        assert!((s.stim[1] - 1.4467).abs() < 1e-6);
        assert_eq!(s.stim[3], 0.5);
        assert_eq!(s.stim[4], 0.125);
    }

    #[test]
    fn rejects_event_and_malformed_lines() {
        assert!(parse_stim_line("{\"tick\":5,\"event\":\"poll_fail\"}").is_none());
        assert!(parse_stim_line("{\"stim\":[1.0,2.0]}").is_none());
        assert!(parse_stim_line("{\"stim\":[a,0,0,0,0]}").is_none());
        assert!(parse_stim_line("").is_none());
        // non-finite values are rejected before they reach exchange
        assert!(parse_stim_line("{\"stim\":[NaN,0,0,0,0]}").is_none());
        assert!(parse_stim_line("{\"stim\":[inf,0,0,0,0]}").is_none());
        // a key boundary is required: a quoted name ending in `stim` is not one
        assert!(parse_stim_line("{\"last_stim\":[1,2,3,4,5]}").is_none());
    }

    #[test]
    fn event_lines_are_detected() {
        assert!(is_event_line("{\"tick\":5,\"event\":\"poll_fail\"}"));
        assert!(is_event_line(
            "{\"tick\":6,\"pass\":2,\"event\":\"exchange_error\"}"
        ));
        assert!(!is_event_line("{\"tick\":5,\"stim\":[0,0,0,0,0]}"));
    }

    #[test]
    fn legacy_sample_without_gain_parses_as_legacy() {
        let s = parse_stim_line("{\"tick\":1,\"stim\":[0.7003,1.0,1.0,0.9476,1.0],\"spikes\":0}")
            .unwrap();
        assert_eq!(s.gain, None);
        assert!((s.stim[0] - 0.7003).abs() < 1e-6);
    }

    #[test]
    fn replay_output_replays_identical_wire_values() {
        // the smallest nonzero Q8.8 step must survive format -> parse -> encode
        let stim = [1.0 / 256.0, 0.0, 2.5, 0.0, 0.0];
        let line = replay_line(9, 2, &stim, Some(2.5), 3, -0.9492, 0.668);
        let s = parse_stim_line(&line).unwrap();
        assert_eq!(s.stim[0], as_sent(stim[0]));
        assert_eq!(s.stim[2], as_sent(2.5));
        assert_eq!(s.gain, Some(2.5));
        assert_eq!(silicon_bridge::encode_q88_signed(s.stim[0]), 1);
    }

    #[test]
    fn replay_line_emits_null_gain_for_legacy_input() {
        let line = replay_line(1, 1, &[0.5, 0.0, 0.0, 0.0, 0.0], None, 0, 0.0, 0.0);
        assert!(line.contains("\"gain\":null"));
        let s = parse_stim_line(&line).unwrap();
        assert_eq!(s.gain, None);
    }
}
