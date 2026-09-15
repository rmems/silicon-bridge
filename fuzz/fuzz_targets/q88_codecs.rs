// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz the saturating Q8.8 helpers. No serial I/O.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silicon_bridge::{
    STIMULUS_Q88_MAX, STIMULUS_Q88_MIN, encode_q88_signed, encode_q88_signed_full,
    encode_q88_unsigned, format_q88_hex, q88_signed_to_f32, q88_to_f32,
};

const UART_RAW_MIN: i16 = -32765;
const UART_RAW_MAX: i16 = 32765;

fn fuzz_q88(data: &[u8]) {
    for word in data.chunks(2) {
        if word.len() < 2 {
            break;
        }
        let raw_i = i16::from_le_bytes([word[0], word[1]]);
        let raw_u = u16::from_le_bytes([word[0], word[1]]);
        assert_eq!(encode_q88_signed_full(q88_signed_to_f32(raw_i)), raw_i);
        assert_eq!(encode_q88_unsigned(q88_to_f32(raw_u)), raw_u);
        let uart = encode_q88_signed(q88_signed_to_f32(raw_i));
        if (UART_RAW_MIN..=UART_RAW_MAX).contains(&raw_i) {
            assert_eq!(uart, raw_i);
        } else {
            assert_eq!(
                uart,
                encode_q88_signed(if raw_i.is_negative() {
                    STIMULUS_Q88_MIN
                } else {
                    STIMULUS_Q88_MAX
                })
            );
        }
    }
    for word in data.chunks(4) {
        if word.len() < 4 {
            break;
        }
        let value = f32::from_le_bytes(word.try_into().unwrap());
        let signed = encode_q88_signed(value);
        let full = encode_q88_signed_full(value);
        let unsigned = encode_q88_unsigned(value);
        let _ = format_q88_hex(value);
        if value.is_nan() {
            assert_eq!(signed, 0);
            assert_eq!(full, 0);
            assert_eq!(unsigned, 0);
        } else {
            assert!((UART_RAW_MIN..=UART_RAW_MAX).contains(&signed));
        }
        if value.is_finite() && (STIMULUS_Q88_MIN..=STIMULUS_Q88_MAX).contains(&value) {
            assert_eq!(signed, full);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    fuzz_q88(data);
});
