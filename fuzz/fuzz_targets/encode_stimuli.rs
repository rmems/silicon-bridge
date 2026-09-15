// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz the pure stimulus encoders. No serial I/O.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silicon_bridge::{
    DenseQ88Layout, MAX_DENSE_CHANNELS, encode_stimuli, encode_stimuli_legacy_v3,
};

fn dim(byte: u8) -> usize {
    if byte == 0 {
        MAX_DENSE_CHANNELS
    } else {
        byte as usize
    }
}

fn f32s_from_bytes(data: &[u8]) -> Vec<f32> {
    data.chunks(4)
        .filter_map(|chunk| {
            if chunk.len() < 4 {
                None
            } else {
                Some(f32::from_le_bytes(chunk.try_into().unwrap()))
            }
        })
        .collect()
}

fn fuzz_encode(data: &[u8]) {
    if data.len() < 2 {
        let _ = encode_stimuli_legacy_v3(&[]);
        return;
    }
    let inputs = dim(data[0]);
    let outputs = dim(data[1]);
    let Ok(layout) = DenseQ88Layout::dense(inputs, outputs) else {
        return;
    };
    let stimuli = f32s_from_bytes(&data[2..]);
    let _ = encode_stimuli(&layout, &stimuli);
    let _ = encode_stimuli_legacy_v3(&stimuli);

    let mut exact = stimuli;
    exact.resize(inputs, 0.0);
    let _ = encode_stimuli(&layout, &exact);
}

fuzz_target!(|data: &[u8]| {
    fuzz_encode(data);
});
