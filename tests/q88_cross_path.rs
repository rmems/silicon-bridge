// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exhaustive cross-path agreement for every Q8.8 encode/decode site.
//!
//! `tests/q88_properties.rs` checks each helper against its own contract.
//! This file checks that the *independent call sites* agree with each other
//! and with a reference model written here in `f64` (not reusing crate code):
//!
//! | Id | Site | Family |
//! |---|---|---|
//! | D1 | `q88_signed_to_f32` | signed decode |
//! | D2 | `decode_response` (dense, v3 framing) | signed decode |
//! | D3 | `decode_response_v4` | signed decode |
//! | D4 | `q88_to_f32` | unsigned decode (differs by design for raw < 0) |
//! | P1 | `encode_q88_signed_full` | signed parameter, full i16 |
//! | P2 | `FixedPointEncode::encode_q88` / `FpgaParameterExporter::to_q88` | signed parameter |
//! | P3 | `format_q88_hex` | signed parameter, hex |
//! | P4 | `ParameterExport::export` (legacy infallible bundle) | signed parameter |
//! | P5 | `try_export` (checked, `RangePolicy::Reject`) | signed parameter |
//! | P6 | `try_export_with_report` (`RangePolicy::Saturate`) | signed parameter |
//! | P7 | `MemFileWriter::write_mem_files` (`.mem` text) | signed parameter |
//! | U1 | `encode_q88_signed` | legacy UART clamp ±127.99 |
//! | U2 | `encode_stimuli_legacy_v3` | legacy UART clamp |
//! | U3 | `encode_stimuli` (checked) | legacy UART clamp |
//! | U4 | `encode_request_v4` (checked) | legacy UART clamp |
//! | M1 | `encode_q88_unsigned` | unsigned magnitude |
//! | M2 | `try_export` with `Q88Encoding::Unsigned` | unsigned magnitude |
//!
//! A disagreement here is a contract finding, not something to "fix" in the
//! test. See the encode-site table in `docs/host-hdl-contract.md`.

use silicon_bridge::{
    CodecError, DenseQ88Layout, FixedPointEncode, FpgaParameterExporter, MemFileWriter,
    NonFiniteKind, ParameterBlock, ParameterExport, ParameterShapeError, Q88_SIGNED_MAX,
    Q88_SIGNED_MIN, Q88_UNSIGNED_MAX, Q88Encoding, RangePolicy, STIMULUS_Q88_MAX, STIMULUS_Q88_MIN,
    crc16_ccitt_false, decode_response, decode_response_v4, encode_q88_signed,
    encode_q88_signed_full, encode_q88_unsigned, encode_request_v4, encode_stimuli,
    encode_stimuli_legacy_v3, format_q88_hex, q88_signed_to_f32, q88_to_f32,
};

const UART_RAW_MIN: i16 = -32765;
const UART_RAW_MAX: i16 = 32765;
/// Words per frame used for the bulk codec sweeps (`MAX_DENSE_CHANNELS`).
const CHUNK: usize = 256;

// ---------------------------------------------------------------------------
// Reference model (f64, independent of crate code)
// ---------------------------------------------------------------------------

/// `trunc(clamp(v, lo, hi) * 256)` saturated into `[raw_lo, raw_hi]`;
/// NaN → 0 (legacy behavior). `×256` is exact in both f32 and f64, so this
/// is bit-exact against an f32 implementation unless f32 overflows to ±inf,
/// which the final clamp also covers.
fn reference(v: f32, lo: f64, hi: f64, raw_lo: i64, raw_hi: i64) -> i64 {
    if v.is_nan() {
        return 0;
    }
    let clamped = f64::from(v).clamp(lo, hi);
    let scaled = (clamped * 256.0).trunc();
    let as_int = if scaled >= 9.0e18 {
        i64::MAX
    } else if scaled <= -9.0e18 {
        i64::MIN
    } else {
        scaled as i64
    };
    as_int.clamp(raw_lo, raw_hi)
}

fn ref_signed_full(v: f32) -> i16 {
    reference(v, f64::NEG_INFINITY, f64::INFINITY, -32768, 32767) as i16
}

fn ref_uart(v: f32) -> i16 {
    reference(
        v,
        f64::from(-127.99_f32),
        f64::from(127.99_f32),
        -32768,
        32767,
    ) as i16
}

fn ref_unsigned(v: f32) -> u16 {
    reference(v, f64::NEG_INFINITY, f64::INFINITY, 0, 65535) as u16
}

fn in_signed_range(v: f32) -> bool {
    v.is_finite() && f64::from(v) >= -128.0 && f64::from(v) <= 32767.0 / 256.0
}

fn in_unsigned_range(v: f32) -> bool {
    v.is_finite() && f64::from(v) >= 0.0 && f64::from(v) <= 65535.0 / 256.0
}

fn non_finite_kind(v: f32) -> Option<NonFiniteKind> {
    if v.is_nan() {
        Some(NonFiniteKind::Nan)
    } else if v == f32::INFINITY {
        Some(NonFiniteKind::PosInfinity)
    } else if v == f32::NEG_INFINITY {
        Some(NonFiniteKind::NegInfinity)
    } else {
        None
    }
}

fn all_words() -> impl Iterator<Item = i16> {
    i16::MIN..=i16::MAX
}

fn exporter_1x1(v: f32) -> FpgaParameterExporter {
    // Put `v` in the weight slot; thresholds/decay are benign in-range values.
    FpgaParameterExporter::from_params(vec![1.0], vec![vec![v]], vec![0.5])
}

fn bulk_exporter(values: &[f32]) -> FpgaParameterExporter {
    let rows: Vec<Vec<f32>> = values.chunks(CHUNK).map(<[f32]>::to_vec).collect();
    assert!(rows.iter().all(|r| r.len() == CHUNK), "pad to CHUNK");
    let n = rows.len();
    FpgaParameterExporter::from_params(vec![1.0; n], rows, vec![0.5; n])
}

fn read_mem_words(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("mem file")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn v4_reply(layout: &DenseQ88Layout, request_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0xAA, 0x04];
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(payload);
    let crc = crc16_ccitt_false(&frame[1..]);
    frame.extend_from_slice(&crc.to_be_bytes());
    assert_eq!(
        frame.len(),
        silicon_bridge::rx_frame_len_v4(layout).unwrap()
    );
    frame
}

// ---------------------------------------------------------------------------
// Pinned constants (both sides of the boundary depend on these exact values)
// ---------------------------------------------------------------------------

#[test]
fn boundary_constants_are_the_documented_values() {
    assert_eq!(Q88_SIGNED_MIN, -128.0);
    assert_eq!(Q88_SIGNED_MAX, 127.996_09_f32);
    assert_eq!(Q88_SIGNED_MAX.to_bits(), (32767.0_f32 / 256.0).to_bits());
    assert_eq!(Q88_UNSIGNED_MAX.to_bits(), (65535.0_f32 / 256.0).to_bits());
    assert_eq!(STIMULUS_Q88_MIN, -127.99_f32);
    assert_eq!(STIMULUS_Q88_MAX, 127.99_f32);
}

// ---------------------------------------------------------------------------
// Exhaustive i16 sweep: decode agreement
// ---------------------------------------------------------------------------

#[test]
fn every_word_decodes_identically_on_every_signed_decode_path() {
    let words: Vec<i16> = all_words().collect();
    let layout = DenseQ88Layout::dense_with_switches(1, CHUNK, false).unwrap();
    let mask_bytes = layout.spike_mask_bytes().unwrap();
    let mut checked = 0usize;
    for (frame_no, chunk) in words.chunks(CHUNK).enumerate() {
        let mut payload: Vec<u8> = chunk.iter().flat_map(|w| w.to_be_bytes()).collect();
        payload.extend(std::iter::repeat_n(0u8, mask_bytes));
        let v3 = decode_response(&layout, &payload).expect("D2 decode");
        let id = frame_no as u32;
        let v4 =
            decode_response_v4(&layout, id, &v4_reply(&layout, id, &payload)).expect("D3 decode");
        for (i, &raw) in chunk.iter().enumerate() {
            let d1 = q88_signed_to_f32(raw);
            let expected = f64::from(raw) / 256.0;
            assert_eq!(f64::from(d1), expected, "D1 value for raw {raw}");
            assert_eq!(v3.potentials[i].to_bits(), d1.to_bits(), "D2≠D1 raw {raw}");
            assert_eq!(v4.potentials[i].to_bits(), d1.to_bits(), "D3≠D1 raw {raw}");
            // D4 (unsigned) is a different interpretation of the same bits:
            // identical for raw >= 0, offset by exactly 256.0 for raw < 0.
            let d4 = q88_to_f32(raw as u16);
            if raw >= 0 {
                assert_eq!(d4.to_bits(), d1.to_bits(), "D4≠D1 raw {raw}");
            } else {
                assert_eq!(f64::from(d4), f64::from(d1) + 256.0, "D4 offset raw {raw}");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 65_536);
}

// ---------------------------------------------------------------------------
// Exhaustive i16 sweep: decode → f32 → re-encode
// ---------------------------------------------------------------------------

#[test]
fn every_word_round_trips_through_each_scalar_encoder() {
    let exporter = FpgaParameterExporter::new();
    for raw in all_words() {
        let v = q88_signed_to_f32(raw);
        assert_eq!(encode_q88_signed_full(v), raw, "P1 raw {raw}");
        assert_eq!(exporter.encode_q88(v), raw, "P2 trait raw {raw}");
        assert_eq!(exporter.to_q88(v), raw, "P2 inherent raw {raw}");
        assert_eq!(
            format_q88_hex(v),
            format!("{:04X}", raw as u16),
            "P3 raw {raw}"
        );
        // U1: identity inside the documented ±127.99 clamp, saturates outside.
        let uart_expected = raw.clamp(UART_RAW_MIN, UART_RAW_MAX);
        assert_eq!(encode_q88_signed(v), uart_expected, "U1 raw {raw}");
        // M1: unsigned interpretation of the same bits round-trips too.
        let u = raw as u16;
        assert_eq!(encode_q88_unsigned(q88_to_f32(u)), u, "M1 raw {raw}");
    }
}

#[test]
fn every_word_round_trips_through_bundle_and_mem_paths() {
    let values: Vec<f32> = all_words().map(q88_signed_to_f32).collect();
    let exporter = bulk_exporter(&values);
    let expected: Vec<i16> = all_words().collect();

    let legacy = ParameterExport::export(&exporter);
    assert_eq!(legacy.weights, expected, "P4 legacy bundle");

    let checked = exporter
        .try_export()
        .expect("P5 every word is representable");
    assert_eq!(checked.weights, expected, "P5 checked bundle");

    let mut saturating = bulk_exporter(&values);
    saturating.set_range_policy(RangePolicy::Saturate);
    let (sat, report) = saturating.try_export_with_report().expect("P6");
    assert_eq!(sat.weights, expected, "P6 saturating bundle");
    assert!(report.events.is_empty(), "P6 clamped an in-range word");

    let dir = tempfile::tempdir().expect("tempdir");
    MemFileWriter::write_mem_files(&exporter, dir.path()).expect("P7 write");
    let lines = read_mem_words(&dir.path().join("parameters_weights.mem"));
    assert_eq!(lines.len(), 65_536);
    for (line, raw) in lines.iter().zip(all_words()) {
        assert_eq!(line, &format!("{:04X}", raw as u16), "P7 .mem raw {raw}");
        assert_eq!(u16::from_str_radix(line, 16).unwrap() as i16, raw);
    }

    // Unsigned bundle path (M2) over every u16 pattern.
    let unsigned_values: Vec<f32> = (0..=u16::MAX).map(q88_to_f32).collect();
    let mut unsigned = bulk_exporter(&unsigned_values);
    unsigned.set_encoding(ParameterBlock::Weights, Q88Encoding::Unsigned);
    let m2 = unsigned
        .try_export()
        .expect("M2 every u16 is representable");
    let expected_u: Vec<i16> = (0..=u16::MAX).map(|u| u as i16).collect();
    assert_eq!(m2.weights, expected_u, "M2 unsigned bundle bit patterns");
}

#[test]
fn every_word_round_trips_through_uart_frame_encoders() {
    let layout = DenseQ88Layout::dense(CHUNK, 1).unwrap();
    let words: Vec<i16> = all_words().collect();
    for (frame_no, chunk) in words.chunks(CHUNK).enumerate() {
        let stimuli: Vec<f32> = chunk.iter().map(|&w| q88_signed_to_f32(w)).collect();
        let expected: Vec<u8> = chunk
            .iter()
            .flat_map(|&w| w.clamp(UART_RAW_MIN, UART_RAW_MAX).to_be_bytes())
            .collect();

        let u3 = encode_stimuli(&layout, &stimuli).expect("U3");
        assert_eq!(u3[0], 0xAA);
        assert_eq!(&u3[1..], expected.as_slice(), "U3 frame {frame_no}");

        let u4 = encode_request_v4(&layout, frame_no as u32, &stimuli).expect("U4");
        let header = silicon_bridge::UART_V4_HEADER_BYTES;
        let crc = silicon_bridge::UART_V4_CRC_BYTES;
        assert_eq!(
            &u4[header..u4.len() - crc],
            expected.as_slice(),
            "U4 frame {frame_no}"
        );

        for (sub, block) in stimuli.chunks(16).enumerate() {
            let u2 = encode_stimuli_legacy_v3(block);
            let lo = sub * 32;
            assert_eq!(
                &u2[1..],
                &expected[lo..lo + 32],
                "U2 frame {frame_no}.{sub}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Dense f32 grid: encode agreement
// ---------------------------------------------------------------------------

fn neighbors(v: f32) -> [f32; 3] {
    [v.next_down(), v, v.next_up()]
}

fn grid() -> Vec<f32> {
    let lsb = 1.0_f32 / 256.0;
    let mut g = Vec::new();
    let specials = [
        0.0,
        -0.0,
        lsb,
        -lsb,
        lsb / 2.0,
        -lsb / 2.0,
        2.0 * lsb,
        -2.0 * lsb,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        f32::from_bits(1), // smallest subnormal
        -f32::from_bits(1),
        127.99,
        -127.99,
        32765.0 / 256.0,
        -32765.0 / 256.0,
        32766.0 / 256.0,
        -32766.0 / 256.0,
        Q88_SIGNED_MAX,
        -Q88_SIGNED_MAX,
        128.0,
        -128.0,
        128.0 + lsb,
        -128.0 - lsb,
        128.0 + lsb / 2.0,
        -128.0 - lsb / 2.0,
        129.0,
        -129.0,
        255.0,
        Q88_UNSIGNED_MAX,
        256.0,
        -256.0,
        1000.0,
        -1000.0,
        32768.0,
        -32768.0,
        1.0e9,
        -1.0e9,
        f32::MAX,
        f32::MIN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
        f32::from_bits(0x7FC0_0001), // NaN with payload
    ];
    for &s in &specials {
        if s.is_finite() {
            g.extend(neighbors(s));
        } else {
            g.push(s);
        }
    }
    // Quarter-LSB lattice across ±260 (covers both the signed and unsigned
    // saturation edges) plus each lattice point's f32 neighbours near zero.
    for k in -(260 * 1024)..=(260 * 1024) {
        g.push(k as f32 / 1024.0);
    }
    for k in -64..=64 {
        g.extend(neighbors(k as f32 / 256.0));
    }
    // Every word's decoded value ± one f32 ulp (truncation-direction edges).
    for raw in all_words() {
        let v = q88_signed_to_f32(raw);
        g.push(v.next_down());
        g.push(v.next_up());
    }
    // Strided walk over all 2^32 f32 bit patterns: every exponent, both signs.
    let mut bits: u32 = 0;
    loop {
        g.push(f32::from_bits(bits));
        match bits.checked_add(65_521) {
            Some(b) => bits = b,
            None => break,
        }
    }
    g
}

#[test]
fn grid_covers_the_required_edges() {
    let g = grid();
    let has = |x: f32| g.iter().any(|v| v.to_bits() == x.to_bits());
    for x in [
        0.0,
        -0.0,
        1.0 / 256.0,
        -1.0 / 256.0,
        127.99,
        -127.99,
        128.0,
        -128.0,
        128.0_f32.next_up(),
        (-128.0_f32).next_down(),
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert!(has(x), "grid is missing {x:?}");
    }
    assert!(g.iter().any(|v| v.is_nan()));
    assert!(g.len() > 600_000, "grid too small: {}", g.len());
}

#[test]
fn scalar_encoders_agree_with_reference_on_dense_grid() {
    let exporter = FpgaParameterExporter::new();
    let mut n = 0usize;
    for v in grid() {
        let full = ref_signed_full(v);
        assert_eq!(
            encode_q88_signed_full(v),
            full,
            "P1 {v:?} bits {:08X}",
            v.to_bits()
        );
        assert_eq!(exporter.encode_q88(v), full, "P2 {v:?}");
        assert_eq!(exporter.to_q88(v), full, "P2 inherent {v:?}");
        assert_eq!(
            format_q88_hex(v),
            format!("{:04X}", full as u16),
            "P3 {v:?}"
        );

        let uart = ref_uart(v);
        assert_eq!(
            encode_q88_signed(v),
            uart,
            "U1 {v:?} bits {:08X}",
            v.to_bits()
        );
        assert!((UART_RAW_MIN..=UART_RAW_MAX).contains(&uart));

        assert_eq!(encode_q88_unsigned(v), ref_unsigned(v), "M1 {v:?}");

        // Cross-family: signed parameter and UART agree wherever the UART
        // clamp does not engage; they never wrap (sign is preserved).
        if v.is_finite() && v.abs() <= 127.99 {
            assert_eq!(
                encode_q88_signed(v),
                encode_q88_signed_full(v),
                "U1≠P1 {v:?}"
            );
        }
        if !v.is_nan() {
            for word in [full, uart] {
                if v > 0.0 {
                    assert!(word >= 0, "wrapped negative for {v:?}");
                } else if v < 0.0 {
                    assert!(word <= 0, "wrapped positive for {v:?}");
                }
            }
        }
        // Truncation toward zero: |decode(encode(v))| <= |v|, error < 1 LSB
        // inside the representable range.
        if in_signed_range(v) {
            let back = f64::from(q88_signed_to_f32(full));
            assert!(back.abs() <= f64::from(v).abs(), "P1 rounded away {v:?}");
            assert!((f64::from(v) - back).abs() < 1.0 / 256.0, "P1 error {v:?}");
        }
        n += 1;
    }
    assert!(n > 600_000);
}

#[test]
fn legacy_bundle_and_checked_paths_agree_on_dense_grid() {
    for v in grid() {
        let full = ref_signed_full(v);
        let ex = exporter_1x1(v);

        // P4 legacy: infallible, saturates, NaN → 0.
        assert_eq!(ParameterExport::export(&ex).weights, vec![full], "P4 {v:?}");

        // P5 checked Reject: Ok == P1 iff finite and representable.
        match (ex.try_export(), non_finite_kind(v)) {
            (Err(ParameterShapeError::NonFinite { kind, .. }), Some(expected)) => {
                assert_eq!(kind, expected, "P5 kind {v:?}");
            }
            (Err(ParameterShapeError::OutOfRange { value, .. }), None) => {
                assert!(!in_signed_range(v), "P5 rejected in-range {v:?}");
                assert_eq!(value.to_bits(), v.to_bits());
            }
            (Ok(p), None) => {
                assert!(in_signed_range(v), "P5 accepted out-of-range {v:?}");
                assert_eq!(p.weights, vec![full], "P5 {v:?}");
            }
            (other, kind) => panic!("P5 unexpected {other:?} for {v:?} ({kind:?})"),
        }

        // P6 checked Saturate: finite → clamp to [MIN, MAX] then P1;
        // non-finite (including ±inf) still rejected.
        let mut sat = exporter_1x1(v);
        sat.set_range_policy(RangePolicy::Saturate);
        match (sat.try_export_with_report(), non_finite_kind(v)) {
            (Err(ParameterShapeError::NonFinite { kind, .. }), Some(expected)) => {
                assert_eq!(kind, expected, "P6 kind {v:?}");
            }
            (Ok((p, report)), None) => {
                assert_eq!(p.weights, vec![full], "P6≠P1 {v:?}");
                assert_eq!(
                    report.events.is_empty(),
                    in_signed_range(v),
                    "P6 report {v:?}"
                );
            }
            (other, kind) => panic!("P6 unexpected {other:?} for {v:?} ({kind:?})"),
        }
        // Saturate-through-report must equal the legacy saturating result for
        // every finite input (the two "saturate" paths must not disagree).
        if v.is_finite() {
            let (p, _) = sat.try_export_with_report().unwrap();
            assert_eq!(
                p.weights,
                ParameterExport::export(&ex).weights,
                "P6≠P4 {v:?}"
            );
        }

        // M2 checked unsigned bundle.
        let mut un = exporter_1x1(v);
        un.set_encoding(ParameterBlock::Weights, Q88Encoding::Unsigned);
        match (un.try_export(), non_finite_kind(v)) {
            (Err(ParameterShapeError::NonFinite { .. }), Some(_)) => {}
            (Err(ParameterShapeError::OutOfRange { .. }), None) => {
                assert!(!in_unsigned_range(v), "M2 rejected in-range {v:?}");
            }
            (Ok(p), None) => {
                assert!(in_unsigned_range(v), "M2 accepted out-of-range {v:?}");
                assert_eq!(p.weights, vec![ref_unsigned(v) as i16], "M2≠M1 {v:?}");
            }
            (other, kind) => panic!("M2 unexpected {other:?} for {v:?} ({kind:?})"),
        }
    }
}

#[test]
fn uart_frame_encoders_agree_on_dense_grid() {
    let layout = DenseQ88Layout::dense(1, 1).unwrap();
    for v in grid() {
        let uart = ref_uart(v);
        let expected = uart.to_be_bytes();

        // U2 legacy: NaN → 0, ±inf saturate.
        let u2 = encode_stimuli_legacy_v3(&[v]);
        assert_eq!(&u2[1..3], &expected, "U2 {v:?}");

        // U3/U4 checked: non-finite rejected with the right kind, else == U1.
        match (encode_stimuli(&layout, &[v]), non_finite_kind(v)) {
            (Err(CodecError::NonFinite { index: 0, kind }), Some(k)) => assert_eq!(kind, k),
            (Ok(tx), None) => assert_eq!(&tx[1..], &expected, "U3 {v:?}"),
            (other, kind) => panic!("U3 unexpected {other:?} for {v:?} ({kind:?})"),
        }
        match (encode_request_v4(&layout, 7, &[v]), non_finite_kind(v)) {
            (Err(CodecError::NonFinite { index: 0, kind }), Some(k)) => assert_eq!(kind, k),
            (Ok(tx), None) => assert_eq!(&tx[8..10], &expected, "U4 {v:?}"),
            (other, kind) => panic!("U4 unexpected {other:?} for {v:?} ({kind:?})"),
        }
    }
}

#[test]
fn mem_writer_agrees_with_bundle_on_in_range_grid_and_rejects_the_rest() {
    let mut in_range: Vec<f32> = grid().into_iter().filter(|&v| in_signed_range(v)).collect();
    in_range.resize(in_range.len().div_ceil(CHUNK) * CHUNK, 0.0);
    let ex = bulk_exporter(&in_range);
    let dir = tempfile::tempdir().expect("tempdir");
    MemFileWriter::write_mem_files(&ex, dir.path()).expect("P7 in-range write");
    let lines = read_mem_words(&dir.path().join("parameters_weights.mem"));
    assert_eq!(lines.len(), in_range.len());
    for (line, &v) in lines.iter().zip(&in_range) {
        assert_eq!(line, &format_q88_hex(v), "P7≠P3 {v:?}");
    }

    for v in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        128.0,
        (-128.0_f32).next_down(),
        Q88_SIGNED_MAX.next_up(),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = MemFileWriter::write_mem_files(&exporter_1x1(v), dir.path());
        assert!(err.is_err(), "P7 accepted {v:?}");
        assert!(
            !dir.path().join("parameters_weights.mem").exists(),
            "P7 left a partial file for {v:?}"
        );
    }
}

#[test]
fn legacy_bundle_ignores_unsigned_selection_as_documented() {
    // `ParameterExport::export` always encodes signed full-range regardless of
    // `set_encoding` and records that in metadata (src/fpga_export.rs).
    let mut ex = exporter_1x1(-1.0);
    ex.set_encoding(ParameterBlock::Weights, Q88Encoding::Unsigned);
    let legacy = ParameterExport::export(&ex);
    assert_eq!(legacy.weights, vec![encode_q88_signed_full(-1.0)]);
    assert_eq!(legacy.metadata.encodings.weights, Q88Encoding::Signed);
}
