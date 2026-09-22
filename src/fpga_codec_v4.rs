// SPDX-License-Identifier: MIT OR Apache-2.0
//! Integrity-bearing UART v4 framing for dense Q8.8 payloads.
//!
//! Version 4 is an opt-in, host-only contract until a matching `silicon-hdl`
//! revision is pinned. It is **not compatible** with current Basys 3
//! SiliconBridge v3.0 firmware. See `docs/uart-v4-protocol.md` for the exact
//! wire format, bounds, CRC parameters, and security limitations.

use crate::{CodecError, DenseQ88Layout, StimulusResponse, decode_response, encode_stimuli};

/// Sync byte at the beginning of every UART v4 request and reply.
pub const UART_V4_SYNC: u8 = 0xAA;
/// Protocol version byte carried by every UART v4 request and reply.
pub const UART_V4_VERSION: u8 = 0x04;
/// Bytes before the payload: sync, version, payload length, and request id.
pub const UART_V4_HEADER_BYTES: usize = 8;
/// Bytes in the trailing CRC-16/CCITT-FALSE value.
pub const UART_V4_CRC_BYTES: usize = 2;

const CRC16_CCITT_FALSE_POLYNOMIAL: u16 = 0x1021;
const CRC16_CCITT_FALSE_INITIAL: u16 = 0xFFFF;

fn frame_len(payload_len: usize) -> Result<usize, CodecError> {
    payload_len
        .checked_add(UART_V4_HEADER_BYTES)
        .and_then(|length| length.checked_add(UART_V4_CRC_BYTES))
        .ok_or(CodecError::LengthOverflow)
}

fn payload_len_field(payload_len: usize) -> Result<u16, CodecError> {
    u16::try_from(payload_len).map_err(|_| CodecError::LengthOverflow)
}

/// Total byte length of a UART v4 request for `layout`.
pub fn tx_frame_len_v4(layout: &DenseQ88Layout) -> Result<usize, CodecError> {
    // The legacy request has one sync byte that is not part of the v4 payload.
    let payload_len = layout
        .tx_len()?
        .checked_sub(1)
        .ok_or(CodecError::LengthOverflow)?;
    let _ = payload_len_field(payload_len)?;
    frame_len(payload_len)
}

/// Total byte length of a UART v4 reply for `layout`.
pub fn rx_frame_len_v4(layout: &DenseQ88Layout) -> Result<usize, CodecError> {
    let payload_len = layout.rx_len()?;
    let _ = payload_len_field(payload_len)?;
    frame_len(payload_len)
}

/// Compute CRC-16/CCITT-FALSE (`poly=0x1021`, `init=0xFFFF`, no reflection,
/// `xorout=0`) over `bytes`.
///
/// UART v4 applies this to every byte from `version` through the final payload
/// byte. The sync byte and the transmitted CRC itself are excluded.
pub fn crc16_ccitt_false(bytes: &[u8]) -> u16 {
    let mut crc = CRC16_CCITT_FALSE_INITIAL;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ CRC16_CCITT_FALSE_POLYNOMIAL
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Encode a checked UART v4 request with a caller-selected request/tick id.
///
/// The dense stimulus payload is identical to v3 after v3's leading sync byte.
/// No I/O or automatic resend is performed.
pub fn encode_request_v4(
    layout: &DenseQ88Layout,
    request_id: u32,
    stimuli: &[f32],
) -> Result<Vec<u8>, CodecError> {
    let legacy = encode_stimuli(layout, stimuli)?;
    let payload = legacy.get(1..).ok_or(CodecError::LengthOverflow)?;
    let payload_len = payload_len_field(payload.len())?;
    let capacity = frame_len(payload.len())?;
    let mut frame = Vec::with_capacity(capacity);
    frame.push(UART_V4_SYNC);
    frame.push(UART_V4_VERSION);
    frame.extend_from_slice(&payload_len.to_be_bytes());
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(payload);
    let crc = crc16_ccitt_false(&frame[1..]);
    frame.extend_from_slice(&crc.to_be_bytes());
    debug_assert_eq!(frame.len(), capacity);
    Ok(frame)
}

/// Decode and authenticate the structure of a UART v4 reply.
///
/// The decoder rejects truncation/trailing bytes, wrong sync or version,
/// payload-length disagreement, a reply for another request id, and a bad
/// CRC before decoding the dense Q8.8 payload. The id provides correlation
/// and freshness metadata only; CRC and request ids are not authentication or
/// replay protection.
pub fn decode_response_v4(
    layout: &DenseQ88Layout,
    expected_request_id: u32,
    frame: &[u8],
) -> Result<StimulusResponse, CodecError> {
    let expected_payload_len = layout.rx_len()?;
    let expected_frame_len = frame_len(expected_payload_len)?;
    if frame.len() != expected_frame_len {
        return Err(CodecError::WrongFrameLength {
            expected: expected_frame_len,
            actual: frame.len(),
        });
    }
    if frame[0] != UART_V4_SYNC {
        return Err(CodecError::WrongSync {
            expected: UART_V4_SYNC,
            actual: frame[0],
        });
    }
    if frame[1] != UART_V4_VERSION {
        return Err(CodecError::UnsupportedVersion {
            expected: UART_V4_VERSION,
            actual: frame[1],
        });
    }
    let advertised_payload_len = usize::from(u16::from_be_bytes([frame[2], frame[3]]));
    if advertised_payload_len != expected_payload_len {
        return Err(CodecError::WrongPayloadLength {
            expected: expected_payload_len,
            actual: advertised_payload_len,
        });
    }
    let request_id = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]);
    if request_id != expected_request_id {
        return Err(CodecError::RequestIdMismatch {
            expected: expected_request_id,
            actual: request_id,
        });
    }
    let crc_start = expected_frame_len - UART_V4_CRC_BYTES;
    let actual_crc = u16::from_be_bytes([frame[crc_start], frame[crc_start + 1]]);
    let expected_crc = crc16_ccitt_false(&frame[1..crc_start]);
    if actual_crc != expected_crc {
        return Err(CodecError::ChecksumMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }
    decode_response(layout, &frame[UART_V4_HEADER_BYTES..crc_start])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> DenseQ88Layout {
        DenseQ88Layout::dense(4, 10).expect("bounded layout")
    }

    fn valid_reply() -> Vec<u8> {
        let mut payload = vec![0; layout().rx_len().expect("bounded layout")];
        payload[0..2].copy_from_slice(&(-256_i16).to_be_bytes());
        payload[20..22].copy_from_slice(&0x0201_u16.to_be_bytes());
        payload[22..24].copy_from_slice(&0x00A5_u16.to_be_bytes());
        let mut frame = vec![UART_V4_SYNC, UART_V4_VERSION, 0, 24, 0x10, 0x20, 0x30, 0x40];
        frame.extend_from_slice(&payload);
        let crc = crc16_ccitt_false(&frame[1..]);
        frame.extend_from_slice(&crc.to_be_bytes());
        frame
    }

    #[test]
    fn standard_crc_check_vector() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
    }

    #[test]
    fn rejects_each_integrity_failure_with_a_typed_error() {
        let valid = valid_reply();
        assert!(matches!(
            decode_response_v4(&layout(), 0x1020_3040, &valid[..valid.len() - 1]),
            Err(CodecError::WrongFrameLength { .. })
        ));
        let mut wrong_version = valid.clone();
        wrong_version[1] = 3;
        assert!(matches!(
            decode_response_v4(&layout(), 0x1020_3040, &wrong_version),
            Err(CodecError::UnsupportedVersion { actual: 3, .. })
        ));
        assert!(matches!(
            decode_response_v4(&layout(), 7, &valid),
            Err(CodecError::RequestIdMismatch {
                actual: 0x1020_3040,
                ..
            })
        ));
        let mut corrupt = valid;
        corrupt[10] ^= 1;
        assert!(matches!(
            decode_response_v4(&layout(), 0x1020_3040, &corrupt),
            Err(CodecError::ChecksumMismatch { .. })
        ));
    }
}
