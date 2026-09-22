// SPDX-License-Identifier: MIT OR Apache-2.0
//! Independent UART v4 request/reply golden contract (RM-1430).
//!
//! Expected bytes are committed from `docs/uart-v4-protocol.md`, not generated
//! by the codec under test. No serial port or FPGA is opened by these tests.

use serde::Deserialize;
use silicon_bridge::{
    CodecError, DenseQ88Layout, UART_V4_SYNC, UART_V4_VERSION, decode_response_v4,
    encode_request_v4, rx_frame_len_v4, tx_frame_len_v4,
};
use std::{fs, path::PathBuf};

#[derive(Debug, Deserialize)]
struct V4Fixture {
    input_channels: usize,
    output_neurons: usize,
    include_switches: bool,
    version: u8,
    request_id: u32,
    stimuli_f32: Vec<f32>,
    tx_payload_hex: String,
    tx_hex: String,
    rx_payload_hex: String,
    rx_hex: String,
    rx_potentials_f32: Vec<f32>,
    rx_spike_bits: Vec<usize>,
    rx_switches: u16,
}

fn parse_hex(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2));
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex byte"))
        .collect()
}

fn fixture() -> V4Fixture {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/uart/v4/dense_4in_10out.json");
    serde_json::from_str(&fs::read_to_string(path).expect("v4 fixture")).expect("v4 JSON")
}

fn layout(fix: &V4Fixture) -> DenseQ88Layout {
    DenseQ88Layout::dense_with_switches(
        fix.input_channels,
        fix.output_neurons,
        fix.include_switches,
    )
    .expect("bounded fixture layout")
}

#[test]
fn v4_tx_matches_independent_bytes() {
    let fix = fixture();
    let expected = parse_hex(&fix.tx_hex);
    assert_eq!(expected[0], UART_V4_SYNC);
    assert_eq!(expected[1], UART_V4_VERSION);
    assert_eq!(fix.version, UART_V4_VERSION);
    assert_eq!(
        &expected[8..expected.len() - 2],
        parse_hex(&fix.tx_payload_hex)
    );
    assert_eq!(tx_frame_len_v4(&layout(&fix)), Ok(expected.len()));
    assert_eq!(
        encode_request_v4(&layout(&fix), fix.request_id, &fix.stimuli_f32),
        Ok(expected)
    );
}

#[test]
fn v4_rx_matches_independent_bytes() {
    let fix = fixture();
    let expected = parse_hex(&fix.rx_hex);
    assert_eq!(
        &expected[8..expected.len() - 2],
        parse_hex(&fix.rx_payload_hex)
    );
    assert_eq!(rx_frame_len_v4(&layout(&fix)), Ok(expected.len()));
    let decoded = decode_response_v4(&layout(&fix), fix.request_id, &expected)
        .expect("valid integrity-bearing fixture");
    assert_eq!(decoded.potentials, fix.rx_potentials_f32);
    for index in 0..fix.output_neurons {
        assert_eq!(decoded.spikes[index], fix.rx_spike_bits.contains(&index));
    }
    assert_eq!(decoded.switches, Some(fix.rx_switches));
}

#[test]
fn v4_checked_decode_rejects_contract_violations() {
    let fix = fixture();
    let layout = layout(&fix);
    let valid = parse_hex(&fix.rx_hex);

    assert!(matches!(
        decode_response_v4(&layout, fix.request_id, &valid[..valid.len() - 1]),
        Err(CodecError::WrongFrameLength { .. })
    ));
    let mut wrong_version = valid.clone();
    wrong_version[1] = 3;
    assert!(matches!(
        decode_response_v4(&layout, fix.request_id, &wrong_version),
        Err(CodecError::UnsupportedVersion { actual: 3, .. })
    ));
    let mut wrong_length = valid.clone();
    wrong_length[3] -= 1;
    assert!(matches!(
        decode_response_v4(&layout, fix.request_id, &wrong_length),
        Err(CodecError::WrongPayloadLength { .. })
    ));
    assert!(matches!(
        decode_response_v4(&layout, fix.request_id.wrapping_add(1), &valid),
        Err(CodecError::RequestIdMismatch { .. })
    ));
    let mut corrupt = valid;
    corrupt[8] ^= 1;
    assert!(matches!(
        decode_response_v4(&layout, fix.request_id, &corrupt),
        Err(CodecError::ChecksumMismatch { .. })
    ));
}
