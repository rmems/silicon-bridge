// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz decoder stability across chunk splits. No serial I/O.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silicon_bridge::{CodecError, DenseQ88Layout, MAX_DENSE_CHANNELS, decode_response};

fn dim(byte: u8) -> usize {
    if byte == 0 {
        MAX_DENSE_CHANNELS
    } else {
        byte as usize
    }
}

fn decode_from_chunks(
    layout: &DenseQ88Layout,
    chunks: &[&[u8]],
) -> Result<silicon_bridge::StimulusResponse, CodecError> {
    let mut buf = Vec::new();
    for chunk in chunks {
        buf.extend_from_slice(chunk);
    }
    decode_response(layout, &buf)
}

fn decode_exact_from_stream(
    layout: &DenseQ88Layout,
    chunks: &[&[u8]],
) -> Result<silicon_bridge::StimulusResponse, CodecError> {
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

fn fuzz_chunks(data: &[u8]) {
    if data.len() < 3 {
        return;
    }
    let inputs = dim(data[0]);
    let outputs = dim(data[1]);
    let Ok(layout) = DenseQ88Layout::dense(inputs, outputs) else {
        return;
    };
    let stride = (data[2] as usize) % 17 + 1;
    let payload = &data[3..];
    let chunks: Vec<&[u8]> = payload.chunks(stride).collect();
    let assembled = decode_from_chunks(&layout, &chunks);
    let direct = decode_response(&layout, payload);
    assert_eq!(assembled, direct);

    let expected = layout.rx_len().expect("supported layout");
    if payload.len() >= expected {
        let streamed = decode_exact_from_stream(&layout, &chunks);
        let prefix = decode_response(&layout, &payload[..expected]);
        assert_eq!(streamed, prefix);
    }
}

fuzz_target!(|data: &[u8]| {
    fuzz_chunks(data);
});
