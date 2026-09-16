// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property coverage for dense Q8.8 UART frames (Linear RM-1352).
//!
//! Covers encode→decode round trips, truncation at every byte of the golden
//! frames, garbage prefixes/suffixes, unused spike-mask bits, and decoder
//! stability across arbitrary chunk splits. Does not open a serial port.

mod common;

use common::{
    decode_exact_from_stream, decode_from_chunks, encode_response, parse_hex, split_with_cuts,
};
use proptest::prelude::*;
use silicon_bridge::{
    CodecError, DENSE_Q88_SYNC, DenseQ88Layout, MAX_DENSE_CHANNELS, NonFiniteKind,
    SILICON_BRIDGE_V3_RX_LEN, SILICON_BRIDGE_V3_TX_LEN, decode_response, encode_q88_signed,
    encode_stimuli, encode_stimuli_legacy_v3, q88_signed_to_f32,
};

fn layout_strategy() -> impl Strategy<Value = DenseQ88Layout> {
    (1..=MAX_DENSE_CHANNELS, 1..=MAX_DENSE_CHANNELS).prop_map(|(inputs, outputs)| {
        DenseQ88Layout::dense(inputs, outputs).expect("1..=MAX is supported")
    })
}

fn finite_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        any::<i16>().prop_map(q88_signed_to_f32),
        any::<f32>().prop_filter("finite", |v| v.is_finite()),
    ]
}

fn golden_uart_frames() -> Vec<(&'static str, DenseQ88Layout, Vec<u8>, Vec<u8>)> {
    let v3 = DenseQ88Layout::silicon_bridge_v3();
    let dense_8 = DenseQ88Layout::dense(8, 8).unwrap();
    let dense_32 = DenseQ88Layout::dense(32, 32).unwrap();
    let dense_8x10 = DenseQ88Layout::dense(8, 10).unwrap();
    vec![
        (
            "legacy_v3",
            v3,
            parse_hex("AA000000800100FF000040FF800200FE000A00F6007FFD800380037FFD0001FFFF"),
            parse_hex("FF00008000000000000000000000000000000000000000000000000000000000800100A5"),
        ),
        (
            "dense_8",
            dense_8,
            parse_hex("AA0100FF000080FF800000000000000000"),
            parse_hex("0080FF00000000000000000000000000810001"),
        ),
        (
            "dense_32",
            dense_32,
            parse_hex(
                "AAFF000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100",
            ),
            parse_hex(
                "FF000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000080800000011234",
            ),
        ),
        (
            "dense_8in_10out",
            dense_8x10,
            parse_hex("AA00400040004000400040004000400040"),
            parse_hex("000000000000000000000000000000000000000002011234"),
        ),
    ]
}

#[test]
fn golden_frames_truncate_at_every_byte_boundary() {
    for (name, layout, tx, rx) in golden_uart_frames() {
        let expected_rx = layout.rx_len().unwrap();
        assert_eq!(rx.len(), expected_rx, "{name} rx");
        assert_eq!(tx.len(), layout.tx_len().unwrap(), "{name} tx");
        for len in 0..rx.len() {
            let err = decode_response(&layout, &rx[..len]).expect_err(name);
            assert_eq!(
                err,
                CodecError::WrongFrameLength {
                    expected: expected_rx,
                    actual: len
                },
                "{name} truncated to {len}"
            );
        }
        assert!(decode_response(&layout, &rx).is_ok(), "{name} complete rx");
    }
}

#[test]
fn golden_frames_reject_garbage_prefix_and_suffix() {
    for (name, layout, tx, rx) in golden_uart_frames() {
        let expected = layout.rx_len().unwrap();
        let mut prefixed = vec![0xFFu8];
        prefixed.extend_from_slice(&rx);
        assert_eq!(
            decode_response(&layout, &prefixed),
            Err(CodecError::WrongFrameLength {
                expected,
                actual: expected + 1
            }),
            "{name} prefix"
        );
        let mut suffixed = rx.clone();
        suffixed.push(0x00);
        assert_eq!(
            decode_response(&layout, &suffixed),
            Err(CodecError::WrongFrameLength {
                expected,
                actual: expected + 1
            }),
            "{name} suffix"
        );
        assert_eq!(tx[0], DENSE_Q88_SYNC);
        let mut tx_prefixed = vec![0x00];
        tx_prefixed.extend_from_slice(&tx);
        assert_ne!(tx_prefixed[0], DENSE_Q88_SYNC);
        assert_eq!(tx_prefixed.len(), layout.tx_len().unwrap() + 1);
    }
}

#[test]
fn unused_spike_mask_bits_do_not_invent_neurons() {
    let layout = DenseQ88Layout::dense(8, 10).unwrap();
    let mut rx = encode_response(&layout, &[0; 10], &[false; 10], 0x1234);
    let mask_start = 10 * 2;
    // Two mask bytes; bits 10..=15 must be ignored (neurons exist only for 0..9).
    rx[mask_start] = 0xFC; // 0b1111_1100 — high bits of the high byte
    rx[mask_start + 1] = 0x00;
    let decoded = decode_response(&layout, &rx).expect("10-neuron reply");
    assert_eq!(decoded.spikes.len(), 10);
    assert!(decoded.spikes.iter().all(|&fired| !fired));
    assert_eq!(decoded.switches, Some(0x1234));
}

#[test]
fn legacy_v3_encode_is_always_33_bytes_and_maps_nan_to_zero() {
    assert_eq!(
        encode_stimuli_legacy_v3(&[]).len(),
        SILICON_BRIDGE_V3_TX_LEN
    );
    let mut long = vec![1.0; 64];
    long[0] = f32::NAN;
    let tx = encode_stimuli_legacy_v3(&long);
    assert_eq!(tx.len(), SILICON_BRIDGE_V3_TX_LEN);
    assert_eq!(&tx[1..3], &[0x00, 0x00]);
    assert_eq!(tx[0], DENSE_Q88_SYNC);
}

#[test]
fn max_layout_stays_bounded() {
    let layout = DenseQ88Layout::dense(MAX_DENSE_CHANNELS, MAX_DENSE_CHANNELS).unwrap();
    let tx_len = layout.tx_len().unwrap();
    let rx_len = layout.rx_len().unwrap();
    assert_eq!(tx_len, 1 + MAX_DENSE_CHANNELS * 2);
    assert!(tx_len < 2048, "TX must stay far below unbounded allocation");
    assert!(rx_len < 2048, "RX must stay far below unbounded allocation");
    let stimuli = vec![0.0; MAX_DENSE_CHANNELS];
    let tx = encode_stimuli(&layout, &stimuli).unwrap();
    assert_eq!(tx.len(), tx_len);
    let rx = vec![0u8; rx_len];
    let decoded = decode_response(&layout, &rx).unwrap();
    assert_eq!(decoded.potentials.len(), MAX_DENSE_CHANNELS);
    assert_eq!(decoded.spikes.len(), MAX_DENSE_CHANNELS);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn dense_layout_rejects_zero_and_oversize(
        inputs in 0usize..=MAX_DENSE_CHANNELS + 8,
        outputs in 0usize..=MAX_DENSE_CHANNELS + 8,
    ) {
        let result = DenseQ88Layout::dense(inputs, outputs);
        if inputs == 0 || outputs == 0 {
            let is_zero = matches!(result, Err(CodecError::ZeroDimension { .. }));
            prop_assert!(is_zero);
        } else if inputs > MAX_DENSE_CHANNELS || outputs > MAX_DENSE_CHANNELS {
            let is_limit = matches!(result, Err(CodecError::ChannelLimit { .. }));
            prop_assert!(is_limit);
        } else {
            let layout = result.unwrap();
            prop_assert_eq!(layout.tx_len().unwrap(), 1 + inputs * 2);
            let mask = outputs.div_ceil(8);
            prop_assert_eq!(layout.rx_len().unwrap(), outputs * 2 + mask + 2);
        }
    }

    #[test]
    fn encode_decode_request_round_trip(
        layout in layout_strategy(),
        values in proptest::collection::vec(finite_f32(), 0..=MAX_DENSE_CHANNELS + 4),
    ) {
        let expected = layout.input_channels();
        match encode_stimuli(&layout, &values) {
            Err(CodecError::WrongInputLength { expected: n, actual }) => {
                prop_assert_eq!(n, expected);
                prop_assert_eq!(actual, values.len());
            }
            Ok(tx) => {
                prop_assert_eq!(values.len(), expected);
                prop_assert_eq!(tx[0], DENSE_Q88_SYNC);
                prop_assert_eq!(tx.len(), layout.tx_len().unwrap());
                for (i, &value) in values.iter().enumerate() {
                    let raw = i16::from_be_bytes([tx[1 + i * 2], tx[2 + i * 2]]);
                    prop_assert_eq!(raw, encode_q88_signed(value));
                    prop_assert_eq!(
                        q88_signed_to_f32(raw),
                        q88_signed_to_f32(encode_q88_signed(value))
                    );
                }
            }
            Err(other) => {
                prop_assert!(
                    false,
                    "unexpected encode error {other:?} for {} stimuli",
                    values.len()
                );
            }
        }
    }

    #[test]
    fn checked_encode_rejects_first_non_finite(
        layout in layout_strategy(),
        index in 0usize..MAX_DENSE_CHANNELS,
        kind in prop_oneof![
            Just(NonFiniteKind::Nan),
            Just(NonFiniteKind::PosInfinity),
            Just(NonFiniteKind::NegInfinity),
        ],
    ) {
        let n = layout.input_channels();
        let index = index % n;
        let mut stimuli = vec![0.25_f32; n];
        stimuli[index] = match kind {
            NonFiniteKind::Nan => f32::NAN,
            NonFiniteKind::PosInfinity => f32::INFINITY,
            NonFiniteKind::NegInfinity => f32::NEG_INFINITY,
        };
        prop_assert_eq!(
            encode_stimuli(&layout, &stimuli),
            Err(CodecError::NonFinite { index, kind })
        );
    }

    #[test]
    fn response_round_trip_and_chunk_stability(
        layout in layout_strategy(),
        raw_potentials in proptest::collection::vec(any::<i16>(), MAX_DENSE_CHANNELS),
        spike_bits in any::<u64>(),
        switches in any::<u16>(),
        cuts in proptest::collection::vec(any::<usize>(), 0..8),
    ) {
        let n = layout.output_neurons();
        let potentials = &raw_potentials[..n];
        let spikes: Vec<bool> = (0..n).map(|i| ((spike_bits >> (i % 64)) & 1) == 1).collect();
        let rx = encode_response(&layout, potentials, &spikes, switches);
        let decoded = decode_response(&layout, &rx).expect("constructed reply");
        prop_assert_eq!(decoded.potentials.len(), n);
        prop_assert_eq!(&decoded.spikes, &spikes);
        prop_assert_eq!(decoded.switches, Some(switches));
        for (i, &raw) in potentials.iter().enumerate() {
            prop_assert_eq!(decoded.potentials[i], q88_signed_to_f32(raw));
        }

        let parts = split_with_cuts(&rx, &cuts);
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        prop_assert_eq!(decode_from_chunks(&layout, &refs), Ok(decoded.clone()));
        prop_assert_eq!(decode_exact_from_stream(&layout, &refs), Ok(decoded));
    }

    #[test]
    fn truncation_and_oversize_are_typed_errors(
        layout in layout_strategy(),
        payload in proptest::collection::vec(any::<u8>(), 0..1024),
    ) {
        let expected = layout.rx_len().unwrap();
        let result = decode_response(&layout, &payload);
        if payload.len() == expected {
            prop_assert!(result.is_ok());
            prop_assert_eq!(result.unwrap().potentials.len(), layout.output_neurons());
        } else {
            prop_assert_eq!(
                result,
                Err(CodecError::WrongFrameLength {
                    expected,
                    actual: payload.len()
                })
            );
        }
    }

    #[test]
    fn suffix_is_ignored_once_a_complete_frame_has_arrived(
        layout in layout_strategy(),
        suffix in proptest::collection::vec(any::<u8>(), 0..64),
        stride in 1usize..=17,
    ) {
        let n = layout.output_neurons();
        let rx = encode_response(&layout, &vec![0i16; n], &vec![false; n], 0xA55A);
        let mut stream = rx.clone();
        stream.extend_from_slice(&suffix);
        let chunks: Vec<&[u8]> = stream.chunks(stride).collect();
        let decoded = decode_exact_from_stream(&layout, &chunks).expect("complete frame");
        prop_assert_eq!(&decoded, &decode_response(&layout, &rx).unwrap());
        if suffix.is_empty() {
            prop_assert_eq!(decode_from_chunks(&layout, &chunks), Ok(decoded));
        } else {
            prop_assert_eq!(
                decode_from_chunks(&layout, &chunks),
                Err(CodecError::WrongFrameLength {
                    expected: layout.rx_len().unwrap(),
                    actual: stream.len()
                })
            );
        }
    }

    #[test]
    fn legacy_wrapper_pads_truncates_and_stays_33_bytes(
        stimuli in proptest::collection::vec(any::<f32>(), 0..64),
    ) {
        let tx = encode_stimuli_legacy_v3(&stimuli);
        prop_assert_eq!(tx.len(), SILICON_BRIDGE_V3_TX_LEN);
        prop_assert_eq!(tx[0], DENSE_Q88_SYNC);
        let mut padded = stimuli.clone();
        padded.resize(16, 0.0);
        padded.truncate(16);
        for (i, &value) in padded.iter().enumerate() {
            let raw = i16::from_be_bytes([tx[1 + i * 2], tx[2 + i * 2]]);
            prop_assert_eq!(raw, encode_q88_signed(value));
        }
        if stimuli.len() == 16 && stimuli.iter().all(|v| v.is_finite()) {
            prop_assert_eq!(
                tx,
                encode_stimuli(&DenseQ88Layout::silicon_bridge_v3(), &stimuli).unwrap()
            );
        }
    }
}

#[test]
fn v3_constants_stay_aligned_with_the_layout() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    assert_eq!(layout.tx_len(), Ok(SILICON_BRIDGE_V3_TX_LEN));
    assert_eq!(layout.rx_len(), Ok(SILICON_BRIDGE_V3_RX_LEN));
}
