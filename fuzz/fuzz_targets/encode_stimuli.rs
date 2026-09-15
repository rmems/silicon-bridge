// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzz the pure stimulus encoders. No serial I/O.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silicon_bridge::{
    CodecError, DENSE_Q88_SYNC, DenseQ88Layout, MAX_DENSE_CHANNELS, SILICON_BRIDGE_V3_TX_LEN,
    encode_stimuli, encode_stimuli_legacy_v3,
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

fn assert_legacy(stimuli: &[f32]) {
    let tx = encode_stimuli_legacy_v3(stimuli);
    assert_eq!(tx.len(), SILICON_BRIDGE_V3_TX_LEN);
    assert_eq!(tx[0], DENSE_Q88_SYNC);
}

fn assert_checked(layout: &DenseQ88Layout, stimuli: &[f32]) {
    match encode_stimuli(layout, stimuli) {
        Ok(tx) => {
            assert_eq!(stimuli.len(), layout.input_channels());
            assert!(stimuli.iter().all(|value| value.is_finite()));
            assert_eq!(tx[0], DENSE_Q88_SYNC);
            assert_eq!(tx.len(), layout.tx_len().expect("supported layout"));
        }
        Err(CodecError::WrongInputLength { expected, actual }) => {
            assert_eq!(expected, layout.input_channels());
            assert_eq!(actual, stimuli.len());
        }
        Err(CodecError::NonFinite { index, .. }) => {
            assert_eq!(stimuli.len(), layout.input_channels());
            assert!(stimuli[index].is_nan() || stimuli[index].is_infinite());
        }
        Err(other) => panic!("checked encode of a bounded input must not fail as {other:?}"),
    }
}

fn fuzz_encode(data: &[u8]) {
    if data.len() < 2 {
        assert_legacy(&[]);
        return;
    }
    let inputs = dim(data[0]);
    let outputs = dim(data[1]);
    let Ok(layout) = DenseQ88Layout::dense(inputs, outputs) else {
        return;
    };
    let stimuli = f32s_from_bytes(&data[2..]);
    assert_checked(&layout, &stimuli);
    assert_legacy(&stimuli);

    let mut exact = stimuli;
    exact.resize(inputs, 0.0);
    assert_checked(&layout, &exact);
}

fuzz_target!(|data: &[u8]| {
    fuzz_encode(data);
});
