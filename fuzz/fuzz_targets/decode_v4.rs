// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz `decode_response_v4` across layouts and malformed frames. No serial I/O.
//!
//! Input: `[inputs, outputs, frame...]` (0 means `MAX_DENSE_CHANNELS`). The
//! expected request id is read from the frame's own header when present so the
//! fuzzer reaches the CRC check instead of stopping at the id comparison.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silicon_bridge::{
    CodecError, DenseQ88Layout, MAX_DENSE_CHANNELS, UART_V4_CRC_BYTES, UART_V4_HEADER_BYTES,
    crc16_ccitt_false, decode_response, decode_response_v4, rx_frame_len_v4,
};

fn dim(byte: u8) -> usize {
    if byte == 0 {
        MAX_DENSE_CHANNELS
    } else {
        byte as usize
    }
}

fn header_id(frame: &[u8]) -> u32 {
    match frame.get(4..8) {
        Some(id) => u32::from_be_bytes([id[0], id[1], id[2], id[3]]),
        None => 0,
    }
}

fn fuzz_decode(data: &[u8]) {
    if data.len() < 2 {
        let layout = DenseQ88Layout::silicon_bridge_v3();
        let _ = decode_response_v4(&layout, 0, data);
        return;
    }
    let outputs = dim(data[1]);
    let Ok(layout) = DenseQ88Layout::dense(dim(data[0]), outputs) else {
        return;
    };
    let frame = &data[2..];
    let id = header_id(frame);
    let expected_len = rx_frame_len_v4(&layout).expect("supported layout");

    match decode_response_v4(&layout, id, frame) {
        Ok(decoded) => {
            assert_eq!(frame.len(), expected_len);
            let crc_start = frame.len() - UART_V4_CRC_BYTES;
            assert_eq!(
                u16::from_be_bytes([frame[crc_start], frame[crc_start + 1]]),
                crc16_ccitt_false(&frame[1..crc_start])
            );
            assert_eq!(decoded.potentials.len(), outputs);
            assert_eq!(decoded.spikes.len(), outputs);
            let payload = &frame[UART_V4_HEADER_BYTES..crc_start];
            assert_eq!(decode_response(&layout, payload), Ok(decoded));
            // A reply for another request must never decode.
            assert!(matches!(
                decode_response_v4(&layout, id.wrapping_add(1), frame),
                Err(CodecError::RequestIdMismatch { .. })
            ));
            // Every single-bit flip of a valid frame must be rejected.
            let mut corrupt = frame.to_vec();
            for byte in 0..corrupt.len() {
                for bit in 0..8 {
                    corrupt[byte] ^= 1 << bit;
                    assert!(
                        decode_response_v4(&layout, id, &corrupt).is_err(),
                        "flip byte {byte} bit {bit} decoded silently"
                    );
                    corrupt[byte] ^= 1 << bit;
                }
            }
        }
        Err(CodecError::WrongFrameLength { expected, actual }) => {
            assert_eq!(expected, expected_len);
            assert_eq!(actual, frame.len());
            assert_ne!(actual, expected);
        }
        Err(
            CodecError::WrongSync { .. }
            | CodecError::UnsupportedVersion { .. }
            | CodecError::WrongPayloadLength { .. }
            | CodecError::ChecksumMismatch { .. },
        ) => assert_eq!(frame.len(), expected_len),
        Err(other) => panic!("v4 decode of a bounded frame must not fail as {other:?}"),
    }
    if expected_len > 0 {
        let _ = decode_response_v4(&layout, id, &frame[..frame.len().min(expected_len - 1)]);
    }
}

fuzz_target!(|data: &[u8]| {
    fuzz_decode(data);
});
