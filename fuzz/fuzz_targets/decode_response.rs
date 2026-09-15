// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz `decode_response` across layouts and malformed payloads. No serial I/O.
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

fn fuzz_decode(data: &[u8]) {
    if data.len() < 2 {
        let layout = DenseQ88Layout::silicon_bridge_v3();
        let _ = decode_response(&layout, data);
        return;
    }
    let inputs = dim(data[0]);
    let outputs = dim(data[1]);
    let Ok(layout) = DenseQ88Layout::dense(inputs, outputs) else {
        return;
    };
    let payload = &data[2..];
    let result = decode_response(&layout, payload);
    let expected = layout.rx_len().expect("supported layout");
    match result {
        Ok(decoded) => {
            assert_eq!(payload.len(), expected);
            assert_eq!(decoded.potentials.len(), outputs);
            assert_eq!(decoded.spikes.len(), outputs);
            assert!(decoded.switches.is_some());
        }
        Err(CodecError::WrongFrameLength {
            expected: n,
            actual,
        }) => {
            assert_eq!(n, expected);
            assert_eq!(actual, payload.len());
        }
        Err(other) => panic!("decode of a bounded payload must not fail as {other:?}"),
    }
    if expected > 0 {
        let _ = decode_response(&layout, &payload[..payload.len().min(expected - 1)]);
    }
}

fuzz_target!(|data: &[u8]| {
    fuzz_decode(data);
});
