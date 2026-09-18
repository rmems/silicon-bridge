// SPDX-License-Identifier: MIT OR Apache-2.0
//! Independent UART request/response golden bytes (#53, after #52).
//!
//! ASCII `.mem` hex is a different contract (`tests/golden_export_contract.rs`).
//! These fixtures are raw big-endian frames. Expected hex is committed, not
//! produced by `encode_stimuli` / `decode_response`.

use serde::Deserialize;
use silicon_bridge::{
    DENSE_Q88_SYNC, DenseQ88Layout, Q88_SIGNED_MAX, SILICON_BRIDGE_V3_RX_LEN,
    SILICON_BRIDGE_V3_TX_LEN, SILICON_HDL_V3_INPUT_CHANNELS, decode_response, encode_stimuli,
    encode_stimuli_legacy_v3, format_q88_hex,
};
use std::fs;
use std::path::PathBuf;

fn uart_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/uart")
}

fn parse_hex(hex: &str) -> Vec<u8> {
    assert!(
        hex.len().is_multiple_of(2),
        "golden hex must be even-length: {hex}"
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex byte"))
        .collect()
}

#[derive(Debug, Deserialize)]
struct LegacyV3 {
    tx_hex: String,
    rx_hex: String,
    stimuli_f32: Vec<f32>,
    tx_words_hex: Vec<String>,
    clamp_vs_mem: Vec<ClampVsMem>,
    rx_potentials_f32: Vec<f32>,
    rx_spike_bits: Vec<usize>,
    rx_switches: u16,
}

#[derive(Debug, Deserialize)]
struct ClampVsMem {
    value: f32,
    uart: String,
    mem: String,
}

#[derive(Debug, Deserialize)]
struct DenseFrame {
    input_channels: usize,
    output_neurons: usize,
    tx_len: usize,
    rx_len: usize,
    spike_mask_bytes: usize,
    tx_hex: String,
    rx_hex: String,
    rx_spike_bits: Vec<usize>,
    rx_switches: u16,
    #[serde(default)]
    stimuli_f32: Vec<f32>,
}

fn load_json<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    serde_json::from_str(&fs::read_to_string(uart_root().join(name)).expect(name)).expect(name)
}

#[test]
fn mem_hex_is_not_uart_byte_order() {
    // Parameter path: one ASCII word. UART: two big-endian bytes, no newline.
    assert_eq!(format_q88_hex(-1.0), "FF00");
    let uart = encode_stimuli(
        &DenseQ88Layout::silicon_bridge_v3(),
        &[
            -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    )
    .expect("v3");
    assert_eq!(uart[0], DENSE_Q88_SYNC);
    assert_eq!(&uart[1..3], &[0xFF, 0x00]);
    assert_ne!(uart, b"FF00");
}

#[test]
fn legacy_v3_tx_matches_independent_bytes() {
    let fix: LegacyV3 = load_json("legacy_v3.json");
    let expected = parse_hex(&fix.tx_hex);
    assert_eq!(expected.len(), SILICON_BRIDGE_V3_TX_LEN);
    assert_eq!(expected[0], DENSE_Q88_SYNC);
    assert_eq!(fix.tx_words_hex.len(), 16);
    let mut rebuilt = vec![DENSE_Q88_SYNC];
    for word in &fix.tx_words_hex {
        let raw = u16::from_str_radix(word, 16).expect("word");
        rebuilt.extend_from_slice(&raw.to_be_bytes());
    }
    assert_eq!(
        rebuilt, expected,
        "tx_hex must match independently listed words"
    );

    let checked = encode_stimuli(&DenseQ88Layout::silicon_bridge_v3(), &fix.stimuli_f32)
        .expect("finite 16-vector");
    assert_eq!(checked, expected);
    assert_eq!(encode_stimuli_legacy_v3(&fix.stimuli_f32), expected);

    for pair in &fix.clamp_vs_mem {
        if (pair.value + 128.0).abs() < f32::EPSILON {
            assert_eq!(pair.uart, "8003");
            assert_eq!(pair.mem, "8000");
        }
        if (pair.value - Q88_SIGNED_MAX).abs() < f32::EPSILON {
            assert_eq!(pair.uart, "7FFD");
            assert_eq!(pair.mem, "7FFF");
        }
        assert_ne!(
            pair.uart, pair.mem,
            "UART clamp and parameter .mem must stay distinct for {}",
            pair.value
        );
    }
}

#[test]
fn silicon_hdl_v3_profile_uses_the_existing_silicon_bridge_v3_uart_layout() {
    let layout = DenseQ88Layout::silicon_bridge_v3();
    assert_eq!(SILICON_HDL_V3_INPUT_CHANNELS, 16);
    assert_eq!(layout.input_channels(), SILICON_HDL_V3_INPUT_CHANNELS);
    assert_eq!(layout.tx_len(), Ok(SILICON_BRIDGE_V3_TX_LEN));
    assert_eq!(layout.rx_len(), Ok(SILICON_BRIDGE_V3_RX_LEN));
}

#[test]
fn legacy_v3_rx_matches_independent_bytes() {
    let fix: LegacyV3 = load_json("legacy_v3.json");
    let rx = parse_hex(&fix.rx_hex);
    assert_eq!(rx.len(), SILICON_BRIDGE_V3_RX_LEN);
    let decoded =
        decode_response(&DenseQ88Layout::silicon_bridge_v3(), &rx).expect("36-byte v3 reply");
    assert_eq!(decoded.potentials, fix.rx_potentials_f32);
    for i in 0..16 {
        let fired = fix.rx_spike_bits.contains(&i);
        assert_eq!(decoded.spikes[i], fired, "spike bit {i}");
    }
    assert_eq!(decoded.switches, Some(fix.rx_switches));
}

fn assert_dense(name: &str, stimuli: &[f32]) {
    let fix: DenseFrame = load_json(name);
    let layout = DenseQ88Layout::dense(fix.input_channels, fix.output_neurons).expect(name);
    assert_eq!(layout.tx_len(), Ok(fix.tx_len));
    assert_eq!(layout.rx_len(), Ok(fix.rx_len));
    assert_eq!(layout.spike_mask_bytes(), Ok(fix.spike_mask_bytes));

    let expected_tx = parse_hex(&fix.tx_hex);
    assert_eq!(expected_tx.len(), fix.tx_len);
    assert_eq!(encode_stimuli(&layout, stimuli).expect(name), expected_tx);

    let rx = parse_hex(&fix.rx_hex);
    assert_eq!(rx.len(), fix.rx_len);
    let decoded = decode_response(&layout, &rx).expect(name);
    assert_eq!(decoded.potentials.len(), fix.output_neurons);
    for i in 0..fix.output_neurons {
        let fired = fix.rx_spike_bits.contains(&i);
        assert_eq!(decoded.spikes[i], fired, "{name} spike {i}");
    }
    assert_eq!(decoded.switches, Some(fix.rx_switches));
}

#[test]
fn dense_8_matches_independent_bytes() {
    let fix: DenseFrame = load_json("dense_8.json");
    assert_dense("dense_8.json", &fix.stimuli_f32);
}

#[test]
fn dense_32_matches_independent_bytes() {
    let mut stimuli = vec![0.0; 32];
    stimuli[0] = -1.0;
    stimuli[31] = 1.0;
    assert_dense("dense_32.json", &stimuli);
}

#[test]
fn dense_8in_10out_matches_independent_bytes() {
    let fix: DenseFrame = load_json("dense_8in_10out.json");
    assert_dense("dense_8in_10out.json", &fix.stimuli_f32);
}
