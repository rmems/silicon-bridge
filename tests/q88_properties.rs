// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property coverage for signed parameter Q8.8, unsigned-magnitude Q8.8,
//! and the narrower legacy UART encoding (Linear RM-1352).
//!
//! Exhaustive loops cover every 16-bit word. Proptest samples boundary-
//! adjacent and non-finite `f32` inputs. These tests do not replace the
//! committed `tests/golden/` fixtures.

use proptest::prelude::*;
use silicon_bridge::{
    Q88_SIGNED_MAX, Q88_SIGNED_MIN, Q88_UNSIGNED_MAX, STIMULUS_Q88_MAX, STIMULUS_Q88_MIN,
    encode_q88_signed, encode_q88_signed_full, encode_q88_unsigned, format_q88_hex,
    q88_signed_to_f32, q88_to_f32,
};

const QUANTUM: f32 = 1.0 / 256.0;
const UART_RAW_MIN: i16 = -32765;
const UART_RAW_MAX: i16 = 32765;

fn q88_f32_strategy() -> impl Strategy<Value = f32> {
    prop_oneof![
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
        Just(0.0),
        Just(-0.0),
        Just(Q88_SIGNED_MIN),
        Just(Q88_SIGNED_MAX),
        Just(STIMULUS_Q88_MIN),
        Just(STIMULUS_Q88_MAX),
        Just(Q88_UNSIGNED_MAX),
        any::<i16>().prop_map(q88_signed_to_f32),
        any::<u16>().prop_map(q88_to_f32),
        any::<f32>(),
    ]
}

fn reconstruction_error_ok(original: f32, reconstructed: f32) -> bool {
    original.is_finite() && (reconstructed - original).abs() < QUANTUM
}

#[test]
fn every_i16_round_trips_through_signed_parameter_codec() {
    for raw in i16::MIN..=i16::MAX {
        let value = q88_signed_to_f32(raw);
        assert_eq!(
            encode_q88_signed_full(value),
            raw,
            "signed parameter round-trip failed for raw {raw}"
        );
        assert_eq!(format_q88_hex(value), format!("{:04X}", raw as u16));
    }
}

#[test]
fn every_u16_round_trips_through_unsigned_parameter_codec() {
    for raw in 0..=u16::MAX {
        let value = q88_to_f32(raw);
        assert_eq!(
            encode_q88_unsigned(value),
            raw,
            "unsigned parameter round-trip failed for raw {raw}"
        );
    }
}

#[test]
fn every_uart_in_range_i16_round_trips_through_legacy_helper() {
    for raw in UART_RAW_MIN..=UART_RAW_MAX {
        let value = q88_signed_to_f32(raw);
        assert_eq!(
            encode_q88_signed(value),
            raw,
            "legacy UART round-trip failed for raw {raw}"
        );
    }
}

#[test]
fn uart_words_outside_the_clamp_saturate_to_the_documented_endpoints() {
    for raw in [i16::MIN, -32767, -32766, 32766, i16::MAX] {
        let value = q88_signed_to_f32(raw);
        let encoded = encode_q88_signed(value);
        if raw < UART_RAW_MIN {
            assert_eq!(encoded, UART_RAW_MIN);
        } else {
            assert_eq!(encoded, UART_RAW_MAX);
        }
    }
}

#[test]
fn extrema_and_neighbors_match_documented_saturation() {
    let extrema = [
        Q88_SIGNED_MIN,
        Q88_SIGNED_MAX,
        STIMULUS_Q88_MIN,
        STIMULUS_Q88_MAX,
        Q88_UNSIGNED_MAX,
        0.0,
        -0.0,
        128.0,
        -128.0,
        256.0,
        -129.0,
    ];
    for &center in &extrema {
        for value in [center.next_down(), center, center.next_up()] {
            assert_signed_full_docs(value);
            assert_uart_docs(value);
            assert_unsigned_docs(value);
        }
    }
}

#[test]
fn non_finite_saturating_helpers_match_docs() {
    for encoder in [encode_q88_signed as fn(f32) -> i16, encode_q88_signed_full] {
        assert_eq!(encoder(f32::NAN), 0);
    }
    assert_eq!(encode_q88_unsigned(f32::NAN), 0);
    assert_eq!(encode_q88_signed_full(f32::INFINITY), i16::MAX);
    assert_eq!(encode_q88_signed_full(f32::NEG_INFINITY), i16::MIN);
    assert_eq!(encode_q88_signed(f32::INFINITY), UART_RAW_MAX);
    assert_eq!(encode_q88_signed(f32::NEG_INFINITY), UART_RAW_MIN);
    assert_eq!(encode_q88_unsigned(f32::INFINITY), u16::MAX);
    assert_eq!(encode_q88_unsigned(f32::NEG_INFINITY), 0);
}

fn assert_signed_full_docs(value: f32) {
    let raw = encode_q88_signed_full(value);
    if value.is_nan() {
        assert_eq!(raw, 0);
        return;
    }
    if value < Q88_SIGNED_MIN {
        assert_eq!(raw, i16::MIN, "signed-full underflow for {value}");
    } else if value > Q88_SIGNED_MAX {
        assert_eq!(raw, i16::MAX, "signed-full overflow for {value}");
    } else {
        assert!(
            reconstruction_error_ok(value, q88_signed_to_f32(raw)),
            "signed-full reconstruction for {value} -> {}",
            q88_signed_to_f32(raw)
        );
    }
}

fn assert_uart_docs(value: f32) {
    let raw = encode_q88_signed(value);
    if value.is_nan() {
        assert_eq!(raw, 0);
        return;
    }
    if value < STIMULUS_Q88_MIN {
        assert_eq!(raw, UART_RAW_MIN, "UART underflow for {value}");
    } else if value > STIMULUS_Q88_MAX {
        assert_eq!(raw, UART_RAW_MAX, "UART overflow for {value}");
    } else {
        assert!(
            reconstruction_error_ok(value, q88_signed_to_f32(raw)),
            "UART reconstruction for {value} -> {}",
            q88_signed_to_f32(raw)
        );
    }
}

fn assert_unsigned_docs(value: f32) {
    let raw = encode_q88_unsigned(value);
    if value.is_nan() {
        assert_eq!(raw, 0);
        return;
    }
    if value <= 0.0 {
        assert_eq!(raw, 0, "unsigned underflow for {value}");
    } else if value > Q88_UNSIGNED_MAX {
        assert_eq!(raw, u16::MAX, "unsigned overflow for {value}");
    } else {
        assert!(
            reconstruction_error_ok(value, q88_to_f32(raw)),
            "unsigned reconstruction for {value} -> {}",
            q88_to_f32(raw)
        );
    }
}

fn assert_truncation_toward_zero_signed_full(raw: i16) {
    let frac = 0.4;
    let value = if raw >= 0 {
        (raw as f32 + frac) / 256.0
    } else {
        (raw as f32 - frac) / 256.0
    };
    assert_eq!(
        encode_q88_signed_full(value),
        raw,
        "signed-full toward-zero truncation failed for raw {raw} value {value}"
    );
}

#[test]
fn signed_parameter_truncates_toward_zero_on_fractional_quanta() {
    for raw in [i16::MIN, -256, -1, 0, 1, 255, 256, i16::MAX] {
        assert_truncation_toward_zero_signed_full(raw);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn signed_parameter_invariants(value in q88_f32_strategy()) {
        assert_signed_full_docs(value);
        let raw = encode_q88_signed_full(value);
        prop_assert_eq!(encode_q88_signed_full(q88_signed_to_f32(raw)), raw);
    }

    #[test]
    fn unsigned_parameter_invariants(value in q88_f32_strategy()) {
        assert_unsigned_docs(value);
        let raw = encode_q88_unsigned(value);
        prop_assert_eq!(encode_q88_unsigned(q88_to_f32(raw)), raw);
    }

    #[test]
    fn legacy_uart_invariants(value in q88_f32_strategy()) {
        assert_uart_docs(value);
        let raw = encode_q88_signed(value);
        prop_assert!((UART_RAW_MIN..=UART_RAW_MAX).contains(&raw));
        if value.is_finite() && (STIMULUS_Q88_MIN..=STIMULUS_Q88_MAX).contains(&value) {
            prop_assert_eq!(encode_q88_signed(q88_signed_to_f32(raw)), raw);
        }
    }

    #[test]
    fn uart_and_parameter_paths_agree_inside_the_uart_clamp(value in q88_f32_strategy()) {
        prop_assume!(value.is_finite());
        if (STIMULUS_Q88_MIN..=STIMULUS_Q88_MAX).contains(&value) {
            prop_assert_eq!(encode_q88_signed(value), encode_q88_signed_full(value));
        }
        // Documented extrema from the golden contract: UART clamp vs .mem.
        prop_assert_ne!(encode_q88_signed(-128.0), encode_q88_signed_full(-128.0));
        prop_assert_ne!(
            encode_q88_signed(Q88_SIGNED_MAX),
            encode_q88_signed_full(Q88_SIGNED_MAX)
        );
    }

    #[test]
    fn boundary_adjacent_bits_do_not_panic(raw in any::<i16>()) {
        let exact = q88_signed_to_f32(raw);
        for value in [exact.next_down(), exact, exact.next_up()] {
            let _ = encode_q88_signed(value);
            let _ = encode_q88_signed_full(value);
            let _ = encode_q88_unsigned(value);
        }
    }
}
