// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic regressions for codec fuzz findings (Linear RM-1352).
//!
//! Promote any `fuzz/artifacts/` crash into a named test here. The initial
//! cases are the empty / truncated / non-finite / extrema inputs that a
//! short campaign is expected to hit; they must not panic.

mod common;

use common::{encode_response, parse_hex};
use silicon_bridge::{
    CodecError, DenseQ88Layout, MAX_DENSE_CHANNELS, NonFiniteKind, decode_response,
    encode_q88_signed, encode_q88_signed_full, encode_q88_unsigned, encode_stimuli,
    encode_stimuli_legacy_v3,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn empty_buffer_is_wrong_frame_length_not_a_panic() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    assert_eq!(
        decode_response(&layout, &[]),
        Err(CodecError::WrongFrameLength {
            expected: 36,
            actual: 0
        })
    );
}

#[test]
fn one_byte_of_sync_is_not_a_v3_reply() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    assert_eq!(
        decode_response(&layout, &[0xAA]),
        Err(CodecError::WrongFrameLength {
            expected: 36,
            actual: 1
        })
    );
}

#[test]
fn all_ones_same_length_v3_reply_decodes_without_panic() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    let rx = vec![0xFFu8; 36];
    let decoded = decode_response(&layout, &rx).expect("same-length garbage is still a decode");
    assert_eq!(decoded.potentials.len(), 16);
    assert_eq!(decoded.spikes.len(), 16);
}

#[test]
fn checked_encode_rejects_nan_at_index_zero_before_any_bytes() {
    let layout = DenseQ88Layout::dense(1, 1).unwrap();
    assert_eq!(
        encode_stimuli(&layout, &[f32::NAN]),
        Err(CodecError::NonFinite {
            index: 0,
            kind: NonFiniteKind::Nan
        })
    );
}

#[test]
fn q88_bit_patterns_including_signaling_nan_do_not_panic() {
    for bits in [0x7FC0_0000u32, 0x7F80_0001, 0xFFC0_0000, 0, u32::MAX] {
        let value = f32::from_bits(bits);
        let _ = encode_q88_signed(value);
        let _ = encode_q88_signed_full(value);
        let _ = encode_q88_unsigned(value);
        let _ = encode_stimuli_legacy_v3(&[value]);
    }
}

#[test]
fn golden_v3_rx_still_decodes_after_being_split_into_single_bytes() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    let rx = parse_hex("FF00008000000000000000000000000000000000000000000000000000000000800100A5");
    let chunks: Vec<&[u8]> = rx.chunks(1).collect();
    let mut buf = Vec::new();
    for chunk in chunks {
        buf.extend_from_slice(chunk);
    }
    assert_eq!(
        decode_response(&layout, &buf).unwrap(),
        decode_response(&layout, &rx).unwrap()
    );
}

#[test]
fn oversize_layout_is_a_typed_error() {
    assert!(matches!(
        DenseQ88Layout::dense(MAX_DENSE_CHANNELS + 1, 1),
        Err(CodecError::ChannelLimit { .. })
    ));
    assert!(matches!(
        DenseQ88Layout::dense(1, usize::MAX),
        Err(CodecError::ChannelLimit { .. })
    ));
}

#[test]
fn constructed_max_mask_with_all_bits_set_stays_in_neuron_count() {
    let layout = DenseQ88Layout::dense(3, 9).unwrap();
    let mut spikes = vec![true; 9];
    spikes[8] = true;
    let mut rx = encode_response(&layout, &[0; 9], &spikes, 0xFFFF);
    let mask_start = 18;
    rx[mask_start] = 0xFF;
    rx[mask_start + 1] = 0xFF;
    let decoded = decode_response(&layout, &rx).unwrap();
    assert_eq!(decoded.spikes.len(), 9);
    assert_eq!(decoded.switches, Some(0xFFFF));
}

#[test]
fn fuzz_seed_corpus_contains_golden_uart_vectors() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let v3_rx =
        parse_hex("FF00008000000000000000000000000000000000000000000000000000000000800100A5");
    let corpus_rx = fs::read(root.join("fuzz/corpus/decode_response/golden_v3_rx_raw"))
        .expect("committed golden v3 RX seed");
    assert_eq!(corpus_rx, v3_rx);

    let signed = fs::read(root.join("fuzz/corpus/q88_codecs/golden_extrema_f32"))
        .expect("committed Q8.8 extrema seed");
    assert!(
        signed.len() >= 4,
        "Q8.8 seed corpus must contain at least one f32 word"
    );
}
