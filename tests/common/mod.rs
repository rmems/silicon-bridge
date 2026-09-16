// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared helpers for Q8.8 / UART codec property and regression tests.

#![allow(dead_code)]

use silicon_bridge::{CodecError, DenseQ88Layout, StimulusResponse, decode_response};

/// Parse a compact even-length hex string into bytes.
pub fn parse_hex(hex: &str) -> Vec<u8> {
    assert!(
        hex.len().is_multiple_of(2),
        "hex must be even-length: {hex}"
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex byte"))
        .collect()
}

/// Concatenate `chunks` then decode. Equivalent to decoding the joined buffer.
pub fn decode_from_chunks(
    layout: &DenseQ88Layout,
    chunks: &[&[u8]],
) -> Result<StimulusResponse, CodecError> {
    let mut buf = Vec::new();
    for chunk in chunks {
        buf.extend_from_slice(chunk);
    }
    decode_response(layout, &buf)
}

/// Decode as soon as `expected` bytes have arrived, ignoring a trailing suffix.
///
/// This is the `read_exact(rx_len)` model used by the UART adapter: extra bytes
/// after a complete reply are not part of the frame.
pub fn decode_exact_from_stream(
    layout: &DenseQ88Layout,
    chunks: &[&[u8]],
) -> Result<StimulusResponse, CodecError> {
    let expected = layout.rx_len()?;
    let mut buf = Vec::new();
    for chunk in chunks {
        buf.extend_from_slice(chunk);
        if buf.len() >= expected {
            return decode_response(layout, &buf[..expected]);
        }
    }
    decode_response(layout, &buf)
}

/// Split `data` at `cuts` (byte offsets). Empty `cuts` yields a single chunk.
pub fn split_with_cuts(data: &[u8], cuts: &[usize]) -> Vec<Vec<u8>> {
    let mut points: Vec<usize> = cuts
        .iter()
        .copied()
        .filter(|&cut| cut > 0 && cut < data.len())
        .collect();
    points.sort_unstable();
    points.dedup();
    let mut chunks = Vec::new();
    let mut start = 0;
    for cut in points {
        chunks.push(data[start..cut].to_vec());
        start = cut;
    }
    chunks.push(data[start..].to_vec());
    chunks
}

/// Inverse of the public RX layout: potentials, BE spike mask, switch word.
pub fn encode_response(
    layout: &DenseQ88Layout,
    potentials: &[i16],
    spikes: &[bool],
    switches: u16,
) -> Vec<u8> {
    assert_eq!(potentials.len(), layout.output_neurons());
    assert_eq!(spikes.len(), layout.output_neurons());
    let mut rx = Vec::with_capacity(layout.rx_len().expect("supported layout"));
    for &potential in potentials {
        rx.extend_from_slice(&potential.to_be_bytes());
    }
    let mask_len = layout.spike_mask_bytes().expect("supported layout");
    let mut mask = vec![0u8; mask_len];
    for (i, &fired) in spikes.iter().enumerate() {
        if fired {
            let byte_from_end = i / 8;
            let bit = i % 8;
            mask[mask_len - 1 - byte_from_end] |= 1 << bit;
        }
    }
    rx.extend_from_slice(&mask);
    rx.extend_from_slice(&switches.to_be_bytes());
    rx
}
