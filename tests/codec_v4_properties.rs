// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property coverage for integrity-bearing UART v4 frames (Linear RM-1430).
//!
//! Covers request framing and reply encode→decode round trips for arbitrary
//! layouts and request ids, rejection of every single-bit flip of a valid
//! reply, truncation at every byte, garbage prefixes/suffixes, and arbitrary
//! garbage input. Replies are built here from the wire definition in
//! `docs/uart-v4-protocol.md`; the CRC is cross-checked against an independent
//! bit-serial reference. Does not open a serial port.

mod common;

use common::parse_hex;
use proptest::prelude::*;
use silicon_bridge::{
    CodecError, DenseQ88Layout, MAX_DENSE_CHANNELS, StimulusResponse, UART_V4_CRC_BYTES,
    UART_V4_HEADER_BYTES, UART_V4_SYNC, UART_V4_VERSION, crc16_ccitt_false, decode_response,
    decode_response_v4, encode_q88_signed, encode_request_v4, encode_stimuli, q88_signed_to_f32,
    rx_frame_len_v4, tx_frame_len_v4,
};

/// Independent CRC-16/CCITT-FALSE: shift the message through the register one
/// bit at a time (MSB first), rather than xoring whole bytes into the top.
fn reference_crc16_ccitt_false(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in bytes {
        for bit in (0..8).rev() {
            let input = (byte >> bit) & 1 == 1;
            let top = crc & 0x8000 != 0;
            crc <<= 1;
            if input != top {
                crc ^= 0x1021;
            }
        }
    }
    crc
}

fn layout_strategy() -> impl Strategy<Value = DenseQ88Layout> {
    (
        1..=MAX_DENSE_CHANNELS,
        1..=MAX_DENSE_CHANNELS,
        any::<bool>(),
    )
        .prop_map(|(inputs, outputs, switches)| {
            DenseQ88Layout::dense_with_switches(inputs, outputs, switches)
                .expect("1..=MAX is supported")
        })
}

/// A small layout keeps exhaustive bit-flip sweeps fast while still crossing
/// mask-byte boundaries and both switch-field settings.
fn small_layout_strategy() -> impl Strategy<Value = DenseQ88Layout> {
    (1usize..=24, 1usize..=24, any::<bool>()).prop_map(|(inputs, outputs, switches)| {
        DenseQ88Layout::dense_with_switches(inputs, outputs, switches).expect("small layout")
    })
}

fn finite_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        any::<i16>().prop_map(q88_signed_to_f32),
        any::<f32>().prop_filter("finite", |v| v.is_finite()),
    ]
}

/// Dense reply payload from the wire definition: BE `i16` potentials, BE spike
/// mask (bit `i` = neuron `i`), then the optional BE switch word.
fn reply_payload(
    layout: &DenseQ88Layout,
    potentials: &[i16],
    spikes: &[bool],
    switches: u16,
) -> Vec<u8> {
    let n = layout.output_neurons();
    assert_eq!(potentials.len(), n);
    assert_eq!(spikes.len(), n);
    let mut payload = Vec::with_capacity(layout.rx_len().expect("supported layout"));
    for &potential in potentials {
        payload.extend_from_slice(&potential.to_be_bytes());
    }
    let mask_len = n.div_ceil(8);
    let mut mask = vec![0u8; mask_len];
    for (i, &fired) in spikes.iter().enumerate() {
        if fired {
            mask[mask_len - 1 - i / 8] |= 1 << (i % 8);
        }
    }
    payload.extend_from_slice(&mask);
    if layout.include_switches() {
        payload.extend_from_slice(&switches.to_be_bytes());
    }
    assert_eq!(payload.len(), layout.rx_len().expect("supported layout"));
    payload
}

/// Wrap `payload` in a v4 frame: sync, version, BE u16 length, BE u32 id,
/// payload, BE CRC over version..payload (independent reference CRC).
fn frame_v4(request_id: u32, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(payload.len()).expect("bounded payload");
    let mut frame = vec![UART_V4_SYNC, UART_V4_VERSION];
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(payload);
    let crc = reference_crc16_ccitt_false(&frame[1..]);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

fn spikes_from_bits(n: usize, bits: &[u64; 4]) -> Vec<bool> {
    (0..n)
        .map(|i| (bits[i / 64] >> (i % 64)) & 1 == 1)
        .collect()
}

/// Structural validity of a v4 frame, checked without the decoder under test.
fn is_structurally_valid(layout: &DenseQ88Layout, expected_id: u32, frame: &[u8]) -> bool {
    let payload_len = layout.rx_len().expect("supported layout");
    if frame.len() != UART_V4_HEADER_BYTES + payload_len + UART_V4_CRC_BYTES {
        return false;
    }
    let crc_start = frame.len() - UART_V4_CRC_BYTES;
    frame[0] == UART_V4_SYNC
        && frame[1] == UART_V4_VERSION
        && usize::from(u16::from_be_bytes([frame[2], frame[3]])) == payload_len
        && u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) == expected_id
        && u16::from_be_bytes([frame[crc_start], frame[crc_start + 1]])
            == reference_crc16_ccitt_false(&frame[1..crc_start])
}

fn golden_v4_reply() -> (DenseQ88Layout, u32, Vec<u8>) {
    // tests/golden/uart/v4/dense_4in_10out.json
    let layout = DenseQ88Layout::dense_with_switches(4, 10, true).unwrap();
    let rx = parse_hex("AA04001810203040FF00008000000000000000000000000000000000020100A52461");
    (layout, 270_544_960, rx)
}

#[test]
fn crc_reference_matches_standard_check_vector() {
    assert_eq!(reference_crc16_ccitt_false(b"123456789"), 0x29B1);
    assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
    assert_eq!(crc16_ccitt_false(&[]), 0xFFFF);
}

#[test]
fn golden_v4_reply_rebuilds_from_the_wire_definition() {
    let (layout, id, rx) = golden_v4_reply();
    let payload = &rx[UART_V4_HEADER_BYTES..rx.len() - UART_V4_CRC_BYTES];
    assert_eq!(frame_v4(id, payload), rx);
    assert!(is_structurally_valid(&layout, id, &rx));
}

#[test]
fn golden_v4_reply_truncates_at_every_byte_and_rejects_prefix_suffix() {
    let (layout, id, rx) = golden_v4_reply();
    let expected = rx.len();
    for len in 0..expected {
        assert_eq!(
            decode_response_v4(&layout, id, &rx[..len]),
            Err(CodecError::WrongFrameLength {
                expected,
                actual: len
            }),
            "truncated to {len}"
        );
    }
    let mut prefixed = vec![UART_V4_SYNC];
    prefixed.extend_from_slice(&rx);
    let mut suffixed = rx.clone();
    suffixed.push(0x00);
    for (name, frame) in [("prefix", prefixed), ("suffix", suffixed)] {
        assert_eq!(
            decode_response_v4(&layout, id, &frame),
            Err(CodecError::WrongFrameLength {
                expected,
                actual: expected + 1
            }),
            "{name}"
        );
    }
    assert!(decode_response_v4(&layout, id, &rx).is_ok());
}

#[test]
fn golden_v4_reply_rejects_every_single_bit_flip() {
    let (layout, id, rx) = golden_v4_reply();
    for byte in 0..rx.len() {
        for bit in 0..8 {
            let mut corrupt = rx.clone();
            corrupt[byte] ^= 1 << bit;
            assert!(
                decode_response_v4(&layout, id, &corrupt).is_err(),
                "flip byte {byte} bit {bit} decoded silently"
            );
        }
    }
}

#[test]
fn max_layout_v4_frames_stay_bounded() {
    let layout = DenseQ88Layout::dense(MAX_DENSE_CHANNELS, MAX_DENSE_CHANNELS).unwrap();
    let tx_len = tx_frame_len_v4(&layout).unwrap();
    let rx_len = rx_frame_len_v4(&layout).unwrap();
    assert_eq!(
        tx_len,
        UART_V4_HEADER_BYTES + MAX_DENSE_CHANNELS * 2 + UART_V4_CRC_BYTES
    );
    assert!(tx_len < 2048 && rx_len < 2048);
    let tx = encode_request_v4(&layout, u32::MAX, &vec![0.0; MAX_DENSE_CHANNELS]).unwrap();
    assert_eq!(tx.len(), tx_len);
    let payload = vec![0u8; layout.rx_len().unwrap()];
    let decoded = decode_response_v4(&layout, u32::MAX, &frame_v4(u32::MAX, &payload)).unwrap();
    assert_eq!(decoded.potentials.len(), MAX_DENSE_CHANNELS);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn crc_matches_independent_reference(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        prop_assert_eq!(crc16_ccitt_false(&bytes), reference_crc16_ccitt_false(&bytes));
    }

    #[test]
    fn request_framing_round_trip(
        layout in layout_strategy(),
        request_id in any::<u32>(),
        values in proptest::collection::vec(finite_f32(), 0..=MAX_DENSE_CHANNELS + 4),
    ) {
        let n = layout.input_channels();
        match encode_request_v4(&layout, request_id, &values) {
            Err(CodecError::WrongInputLength { expected, actual }) => {
                prop_assert_eq!(expected, n);
                prop_assert_eq!(actual, values.len());
                prop_assert_ne!(values.len(), n);
            }
            Ok(tx) => {
                prop_assert_eq!(values.len(), n);
                prop_assert_eq!(tx.len(), tx_frame_len_v4(&layout).unwrap());
                let payload_len = n * 2;
                prop_assert_eq!(tx.len(), UART_V4_HEADER_BYTES + payload_len + UART_V4_CRC_BYTES);
                prop_assert_eq!(tx[0], UART_V4_SYNC);
                prop_assert_eq!(tx[1], UART_V4_VERSION);
                prop_assert_eq!(usize::from(u16::from_be_bytes([tx[2], tx[3]])), payload_len);
                prop_assert_eq!(u32::from_be_bytes([tx[4], tx[5], tx[6], tx[7]]), request_id);
                let crc_start = tx.len() - UART_V4_CRC_BYTES;
                let payload = &tx[UART_V4_HEADER_BYTES..crc_start];
                // Payload is the v3 dense stimulus payload without its sync byte.
                prop_assert_eq!(payload, &encode_stimuli(&layout, &values).unwrap()[1..]);
                for (i, &value) in values.iter().enumerate() {
                    let raw = i16::from_be_bytes([payload[i * 2], payload[i * 2 + 1]]);
                    prop_assert_eq!(raw, encode_q88_signed(value));
                }
                prop_assert_eq!(
                    u16::from_be_bytes([tx[crc_start], tx[crc_start + 1]]),
                    reference_crc16_ccitt_false(&tx[1..crc_start])
                );
                prop_assert_eq!(&frame_v4(request_id, payload), &tx);
            }
            Err(other) => {
                prop_assert!(false, "unexpected encode error {other:?}");
            }
        }
    }

    #[test]
    fn reply_encode_decode_round_trip(
        layout in layout_strategy(),
        request_id in any::<u32>(),
        raw_potentials in proptest::collection::vec(any::<i16>(), MAX_DENSE_CHANNELS),
        spike_bits in any::<[u64; 4]>(),
        switches in any::<u16>(),
    ) {
        let n = layout.output_neurons();
        let potentials = &raw_potentials[..n];
        let spikes = spikes_from_bits(n, &spike_bits);
        let payload = reply_payload(&layout, potentials, &spikes, switches);
        let frame = frame_v4(request_id, &payload);
        prop_assert_eq!(frame.len(), rx_frame_len_v4(&layout).unwrap());

        let decoded = decode_response_v4(&layout, request_id, &frame).expect("constructed reply");
        prop_assert_eq!(decoded.potentials.len(), n);
        for (i, &raw) in potentials.iter().enumerate() {
            prop_assert_eq!(decoded.potentials[i], q88_signed_to_f32(raw));
        }
        prop_assert_eq!(&decoded.spikes, &spikes);
        let expected_switches = layout.include_switches().then_some(switches);
        prop_assert_eq!(decoded.switches, expected_switches);
        // v4 payload decoding is exactly the v3 dense decoder.
        prop_assert_eq!(&decoded, &decode_response(&layout, &payload).unwrap());

        // Any other expected id is a typed mismatch, never a decode.
        let other_id = request_id ^ 0x8000_0001;
        prop_assert_eq!(
            decode_response_v4(&layout, other_id, &frame),
            Err(CodecError::RequestIdMismatch { expected: other_id, actual: request_id })
        );
    }

    #[test]
    fn every_single_bit_flip_is_rejected(
        layout in small_layout_strategy(),
        request_id in any::<u32>(),
        raw_potentials in proptest::collection::vec(any::<i16>(), 24),
        spike_bits in any::<[u64; 4]>(),
        switches in any::<u16>(),
    ) {
        let n = layout.output_neurons();
        let spikes = spikes_from_bits(n, &spike_bits);
        let payload = reply_payload(&layout, &raw_potentials[..n], &spikes, switches);
        let frame = frame_v4(request_id, &payload);
        let crc_start = frame.len() - UART_V4_CRC_BYTES;
        let original = decode_response_v4(&layout, request_id, &frame).expect("valid reply");

        for byte in 0..frame.len() {
            for bit in 0..8 {
                let mut corrupt = frame.clone();
                corrupt[byte] ^= 1 << bit;
                let result = decode_response_v4(&layout, request_id, &corrupt);
                let rejected_as_expected = match byte {
                    0 => matches!(result, Err(CodecError::WrongSync { .. })),
                    1 => matches!(result, Err(CodecError::UnsupportedVersion { .. })),
                    2 | 3 => matches!(result, Err(CodecError::WrongPayloadLength { .. })),
                    4..=7 => matches!(result, Err(CodecError::RequestIdMismatch { .. })),
                    _ => matches!(result, Err(CodecError::ChecksumMismatch { .. })),
                };
                prop_assert!(
                    rejected_as_expected,
                    "byte {} bit {}: {:?} (original {:?})", byte, bit, result, original
                );

                // The CRC alone must also catch every flip it covers, so the
                // header checks are defence in depth, not the only barrier.
                // Re-decoding with the corrupted frame's own id isolates the CRC.
                if (4..=7).contains(&byte) {
                    let own_id = u32::from_be_bytes([corrupt[4], corrupt[5], corrupt[6], corrupt[7]]);
                    let isolated = decode_response_v4(&layout, own_id, &corrupt);
                    let crc_caught = matches!(isolated, Err(CodecError::ChecksumMismatch { .. }));
                    prop_assert!(crc_caught, "id flip byte {} bit {}: {:?}", byte, bit, isolated);
                }
                if byte >= 1 {
                    prop_assert_ne!(
                        u16::from_be_bytes([corrupt[crc_start], corrupt[crc_start + 1]]),
                        crc16_ccitt_false(&corrupt[1..crc_start]),
                        "CRC missed flip at byte {} bit {}", byte, bit
                    );
                }
            }
        }
    }

    #[test]
    fn request_crc_detects_every_single_bit_flip(
        layout in small_layout_strategy(),
        request_id in any::<u32>(),
        raw in proptest::collection::vec(any::<i16>(), 24),
    ) {
        let stimuli: Vec<f32> = raw[..layout.input_channels()]
            .iter()
            .map(|&r| q88_signed_to_f32(r))
            .collect();
        let tx = encode_request_v4(&layout, request_id, &stimuli).unwrap();
        let crc_start = tx.len() - UART_V4_CRC_BYTES;
        for byte in 1..tx.len() {
            for bit in 0..8 {
                let mut corrupt = tx.clone();
                corrupt[byte] ^= 1 << bit;
                prop_assert_ne!(
                    u16::from_be_bytes([corrupt[crc_start], corrupt[crc_start + 1]]),
                    crc16_ccitt_false(&corrupt[1..crc_start]),
                    "request CRC missed flip at byte {} bit {}", byte, bit
                );
            }
        }
    }

    #[test]
    fn truncation_and_extension_are_typed_errors(
        layout in layout_strategy(),
        request_id in any::<u32>(),
        cut in any::<usize>(),
        suffix in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        let n = layout.output_neurons();
        let payload = reply_payload(&layout, &vec![0i16; n], &vec![false; n], 0xA55A);
        let frame = frame_v4(request_id, &payload);
        let expected = frame.len();
        let len = cut % expected;
        prop_assert_eq!(
            decode_response_v4(&layout, request_id, &frame[..len]),
            Err(CodecError::WrongFrameLength { expected, actual: len })
        );
        let mut extended = frame.clone();
        extended.extend_from_slice(&suffix);
        prop_assert_eq!(
            decode_response_v4(&layout, request_id, &extended),
            Err(CodecError::WrongFrameLength { expected, actual: expected + suffix.len() })
        );
    }

    #[test]
    fn garbage_never_panics_and_only_valid_frames_decode(
        layout in layout_strategy(),
        request_id in any::<u32>(),
        garbage in proptest::collection::vec(any::<u8>(), 0..1200),
    ) {
        let result = decode_response_v4(&layout, request_id, &garbage);
        if is_structurally_valid(&layout, request_id, &garbage) {
            prop_assert!(result.is_ok());
        } else {
            prop_assert!(result.is_err(), "garbage decoded: {:?}", result);
        }
    }

    #[test]
    fn right_length_garbage_is_rejected(
        layout in layout_strategy(),
        request_id in any::<u32>(),
        seed in proptest::collection::vec(any::<u8>(), 600),
        keep_header in any::<bool>(),
    ) {
        // Length-correct garbage reaches the header/CRC checks, which a
        // random-length vector almost never does.
        let len = rx_frame_len_v4(&layout).unwrap();
        let mut frame: Vec<u8> = seed.iter().cycle().take(len).copied().collect();
        if keep_header {
            let payload_len = u16::try_from(layout.rx_len().unwrap()).unwrap();
            frame[0] = UART_V4_SYNC;
            frame[1] = UART_V4_VERSION;
            frame[2..4].copy_from_slice(&payload_len.to_be_bytes());
            frame[4..8].copy_from_slice(&request_id.to_be_bytes());
        }
        let result: Result<StimulusResponse, CodecError> =
            decode_response_v4(&layout, request_id, &frame);
        prop_assert_eq!(result.is_ok(), is_structurally_valid(&layout, request_id, &frame));
        if keep_header && result.is_err() {
            let is_crc = matches!(result, Err(CodecError::ChecksumMismatch { .. }));
            prop_assert!(is_crc);
        }
    }
}
