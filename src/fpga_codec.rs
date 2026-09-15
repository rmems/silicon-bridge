// SPDX-License-Identifier: MIT OR Apache-2.0
//! SiliconBridge UART request/response codecs.
//!
//! Pure encode/decode for the host↔FPGA stimulus frame. This module does not
//! open serial ports, depend on `serialport`, or require an async runtime.
//! The optional `FpgaBridge` adapter (`uart` feature) is the I/O layer.
//!
//! ## Profiles
//!
//! [`DenseQ88Layout::silicon_bridge_v3`] is the on-wire SiliconBridge v3.0
//! frame used by current Basys3 firmware: TX `0xAA` + 16 big-endian signed
//! Q8.8 stimuli (UART ±127.99 clamp); RX 16 potentials + a 16-bit spike mask
//! + a 16-bit switch field (36 bytes). Changing host dimensions does **not**
//! make existing FPGA firmware compatible — a non-legacy layout needs a
//! matching firmware revision.
//!
//! [`DenseQ88Layout::dense`] is the same header, word interpretation, mask
//! bit-order, and switch field, with caller-chosen input and output counts.
//! Input-channel count is independent of output-neuron count. Unsupported
//! descriptions (zero sizes, sizes above [`MAX_DENSE_CHANNELS`]) return
//! [`CodecError`] rather than accepting an arbitrary protocol.
//!
//! ## Checked vs legacy
//!
//! [`encode_stimuli`] requires exactly `input_channels` finite values.
//! [`encode_stimuli_legacy_v3`] is the documented opt-in wrapper that
//! zero-pads a short slice and truncates a long one, and that lets
//! [`crate::encode_q88_signed`] map `NaN` to `0` (the historical
//! `process_stimuli` behaviour).
//!
//! ## Unframed replies
//!
//! Legacy RX has no sync byte, length, checksum, or request id. A decode
//! that sees the expected byte count cannot tell a stale or misaligned
//! same-length reply from a fresh one. This crate does not invent a
//! checksum or claim guaranteed resynchronization.

use crate::{NonFiniteKind, encode_q88_signed, q88_signed_to_f32};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Sync byte on every dense-Q8.8 request, including SiliconBridge v3.0.
pub const DENSE_Q88_SYNC: u8 = 0xAA;

/// Input and output count of the SiliconBridge v3.0 firmware profile.
pub const SILICON_BRIDGE_V3_CHANNELS: usize = 16;

/// Inclusive upper bound on dense-profile channel / neuron counts.
///
/// Chosen so TX/RX length arithmetic stays in `usize` and allocations stay
/// bounded. It is a software limit, not a hardware capability claim.
pub const MAX_DENSE_CHANNELS: usize = 256;

/// Byte length of the SiliconBridge v3.0 reply (16×2 potentials + 2-byte
/// spike mask + 2-byte switches).
pub const SILICON_BRIDGE_V3_RX_LEN: usize = 36;

/// Byte length of the SiliconBridge v3.0 request (`0xAA` + 16×2 stimuli).
pub const SILICON_BRIDGE_V3_TX_LEN: usize = 33;

/// Dense signed-Q8.8 stimulus layout.
///
/// Header is always [`DENSE_Q88_SYNC`]. Stimuli and potentials are big-endian
/// `i16` words from [`encode_q88_signed`] / [`q88_signed_to_f32`]. The spike
/// mask is `ceil(output_neurons / 8)` bytes, interpreted as a big-endian
/// integer whose bit `i` is neuron `i` (LSB of the last mask byte is neuron
/// 0). That matches the v3 16-bit big-endian spike word. A 16-bit switch
/// field follows the mask when [`Self::include_switches`] is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DenseQ88Layout {
    input_channels: usize,
    output_neurons: usize,
    include_switches: bool,
}

impl DenseQ88Layout {
    /// SiliconBridge v3.0: 16 inputs, 16 outputs, switch field present.
    ///
    /// This is the software profile that matches current Basys3 firmware.
    pub const fn silicon_bridge_v3() -> Self {
        Self {
            input_channels: SILICON_BRIDGE_V3_CHANNELS,
            output_neurons: SILICON_BRIDGE_V3_CHANNELS,
            include_switches: true,
        }
    }

    /// Dense Q8.8 frame with caller-chosen input and output counts.
    ///
    /// Uses the same sync byte, signed UART clamp, big-endian words, spike
    /// bit-order, and 16-bit switch field as v3. Existing FPGA firmware is
    /// **not** assumed to accept non-16/16 sizes — matching RTL is required.
    pub fn dense(input_channels: usize, output_neurons: usize) -> Result<Self, CodecError> {
        if input_channels == 0 || output_neurons == 0 {
            return Err(CodecError::ZeroDimension {
                input_channels,
                output_neurons,
            });
        }
        if input_channels > MAX_DENSE_CHANNELS || output_neurons > MAX_DENSE_CHANNELS {
            return Err(CodecError::ChannelLimit {
                input_channels,
                output_neurons,
                max: MAX_DENSE_CHANNELS,
            });
        }
        // Force length arithmetic to fail closed before any allocation.
        let _ = tx_len(input_channels)?;
        let _ = rx_len(output_neurons, true)?;
        Ok(Self {
            input_channels,
            output_neurons,
            include_switches: true,
        })
    }

    /// Number of stimulus words on TX (distinct from [`Self::output_neurons`]).
    pub const fn input_channels(&self) -> usize {
        self.input_channels
    }

    /// Number of potential / spike channels on RX.
    pub const fn output_neurons(&self) -> usize {
        self.output_neurons
    }

    /// Whether the RX frame ends with the 16-bit switch field.
    pub const fn include_switches(&self) -> bool {
        self.include_switches
    }

    /// Request length: 1 sync byte + `2 * input_channels`.
    pub fn tx_len(&self) -> Result<usize, CodecError> {
        tx_len(self.input_channels)
    }

    /// Reply length: potentials + spike mask + optional switches.
    pub fn rx_len(&self) -> Result<usize, CodecError> {
        rx_len(self.output_neurons, self.include_switches)
    }

    /// Spike-mask byte count: `ceil(output_neurons / 8)`.
    pub fn spike_mask_bytes(&self) -> Result<usize, CodecError> {
        spike_mask_bytes(self.output_neurons)
    }
}

fn spike_mask_bytes(output_neurons: usize) -> Result<usize, CodecError> {
    output_neurons
        .checked_add(7)
        .and_then(|n| n.checked_div(8))
        .filter(|&n| n > 0 || output_neurons == 0)
        .ok_or(CodecError::LengthOverflow)
}

fn tx_len(input_channels: usize) -> Result<usize, CodecError> {
    input_channels
        .checked_mul(2)
        .and_then(|words| words.checked_add(1))
        .ok_or(CodecError::LengthOverflow)
}

fn rx_len(output_neurons: usize, include_switches: bool) -> Result<usize, CodecError> {
    let potentials = output_neurons
        .checked_mul(2)
        .ok_or(CodecError::LengthOverflow)?;
    let mask = spike_mask_bytes(output_neurons)?;
    let switches = if include_switches { 2 } else { 0 };
    potentials
        .checked_add(mask)
        .and_then(|n| n.checked_add(switches))
        .ok_or(CodecError::LengthOverflow)
}

/// Structured decode of a stimulus reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StimulusResponse {
    /// Membrane potentials, one `f32` per output neuron (signed Q8.8 decode).
    pub potentials: Vec<f32>,
    /// Spike flags, one `bool` per output neuron (mask bit `i` = neuron `i`).
    pub spikes: Vec<bool>,
    /// Auxiliary 16-bit switch field when the layout includes it.
    pub switches: Option<u16>,
}

/// Encode / decode failure. No transport I/O is performed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecError {
    /// A dense layout asked for zero inputs or zero outputs.
    ZeroDimension {
        /// Requested TX channels.
        input_channels: usize,
        /// Requested RX neurons.
        output_neurons: usize,
    },
    /// A dense layout exceeded [`MAX_DENSE_CHANNELS`].
    ChannelLimit {
        /// Requested TX channels.
        input_channels: usize,
        /// Requested RX neurons.
        output_neurons: usize,
        /// Inclusive software maximum.
        max: usize,
    },
    /// TX/RX length arithmetic overflowed `usize`.
    LengthOverflow,
    /// Checked encode saw the wrong number of stimuli.
    WrongInputLength {
        /// Layout `input_channels`.
        expected: usize,
        /// Slice length supplied.
        actual: usize,
    },
    /// Checked encode rejected `NaN` or an infinity.
    NonFinite {
        /// Index in the stimulus slice.
        index: usize,
        /// Whether the value was NaN, `+inf`, or `-inf`.
        kind: NonFiniteKind,
    },
    /// Decode saw a buffer whose length is not the layout's RX size.
    WrongFrameLength {
        /// [`DenseQ88Layout::rx_len`].
        expected: usize,
        /// Buffer length supplied.
        actual: usize,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension {
                input_channels,
                output_neurons,
            } => write!(
                f,
                "dense Q8.8 layout cannot have a zero dimension \
                 (inputs={input_channels}, outputs={output_neurons})"
            ),
            Self::ChannelLimit {
                input_channels,
                output_neurons,
                max,
            } => write!(
                f,
                "dense Q8.8 layout inputs={input_channels} outputs={output_neurons} \
                 exceeds software limit {max}"
            ),
            Self::LengthOverflow => {
                write!(f, "dense Q8.8 frame length overflowed usize")
            }
            Self::WrongInputLength { expected, actual } => {
                write!(f, "expected {expected} finite stimuli, got {actual}")
            }
            Self::NonFinite { index, kind } => {
                write!(f, "stimulus[{index}] is {kind}; checked encode rejects it")
            }
            Self::WrongFrameLength { expected, actual } => write!(
                f,
                "reply is {actual} bytes, expected {expected} for this layout"
            ),
        }
    }
}

impl std::error::Error for CodecError {}

fn non_finite_kind(value: f32) -> Option<NonFiniteKind> {
    if value.is_nan() {
        Some(NonFiniteKind::Nan)
    } else if value.is_infinite() {
        if value.is_sign_positive() {
            Some(NonFiniteKind::PosInfinity)
        } else {
            Some(NonFiniteKind::NegInfinity)
        }
    } else {
        None
    }
}

/// Encode a checked stimulus request for `layout`.
///
/// `stimuli` must have length [`DenseQ88Layout::input_channels`] and every
/// value must be finite. No padding or truncation. Does not write to a port.
pub fn encode_stimuli(layout: &DenseQ88Layout, stimuli: &[f32]) -> Result<Vec<u8>, CodecError> {
    if stimuli.len() != layout.input_channels {
        return Err(CodecError::WrongInputLength {
            expected: layout.input_channels,
            actual: stimuli.len(),
        });
    }
    for (index, &value) in stimuli.iter().enumerate() {
        if let Some(kind) = non_finite_kind(value) {
            return Err(CodecError::NonFinite { index, kind });
        }
    }
    let cap = layout.tx_len()?;
    let mut tx = Vec::with_capacity(cap);
    tx.push(DENSE_Q88_SYNC);
    for &s in stimuli {
        tx.extend_from_slice(&encode_q88_signed(s).to_be_bytes());
    }
    debug_assert_eq!(tx.len(), cap);
    Ok(tx)
}

/// Historical v3 encode: pad a short slice with `0.0`, keep the first 16 of a
/// long slice, and allow `NaN` (mapped to raw `0` by [`encode_q88_signed`]).
///
/// Prefer [`encode_stimuli`] with [`DenseQ88Layout::silicon_bridge_v3`] when
/// the caller can supply an exact finite vector.
pub fn encode_stimuli_legacy_v3(stimuli: &[f32]) -> Vec<u8> {
    let mut tx = Vec::with_capacity(SILICON_BRIDGE_V3_TX_LEN);
    tx.push(DENSE_Q88_SYNC);
    for i in 0..SILICON_BRIDGE_V3_CHANNELS {
        let s = stimuli.get(i).copied().unwrap_or(0.0);
        tx.extend_from_slice(&encode_q88_signed(s).to_be_bytes());
    }
    tx
}

/// Decode a reply for `layout`.
///
/// `rx` must be exactly [`DenseQ88Layout::rx_len`] bytes. A wrong length is
/// rejected; a correct length is **not** proof the bytes are a fresh,
/// aligned SiliconBridge reply.
pub fn decode_response(layout: &DenseQ88Layout, rx: &[u8]) -> Result<StimulusResponse, CodecError> {
    let expected = layout.rx_len()?;
    if rx.len() != expected {
        return Err(CodecError::WrongFrameLength {
            expected,
            actual: rx.len(),
        });
    }
    let mut potentials = Vec::with_capacity(layout.output_neurons);
    for i in 0..layout.output_neurons {
        let raw = i16::from_be_bytes([rx[i * 2], rx[i * 2 + 1]]);
        potentials.push(q88_signed_to_f32(raw));
    }
    let mask_start = layout
        .output_neurons
        .checked_mul(2)
        .ok_or(CodecError::LengthOverflow)?;
    let mask_len = layout.spike_mask_bytes()?;
    let mask_end = mask_start
        .checked_add(mask_len)
        .ok_or(CodecError::LengthOverflow)?;
    let spikes = spikes_from_be_mask(&rx[mask_start..mask_end], layout.output_neurons);
    let switches = if layout.include_switches {
        Some(u16::from_be_bytes([rx[mask_end], rx[mask_end + 1]]))
    } else {
        None
    };
    Ok(StimulusResponse {
        potentials,
        spikes,
        switches,
    })
}

/// Interpret `mask` as a big-endian integer; bit `i` is neuron `i`.
fn spikes_from_be_mask(mask: &[u8], neurons: usize) -> Vec<bool> {
    (0..neurons)
        .map(|i| {
            let byte_from_end = i / 8;
            let bit = i % 8;
            let byte = mask[mask.len() - 1 - byte_from_end];
            (byte & (1 << bit)) != 0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v3() -> DenseQ88Layout {
        DenseQ88Layout::silicon_bridge_v3()
    }

    #[test]
    fn v3_layout_has_the_legacy_byte_counts() {
        let layout = v3();
        assert_eq!(layout.input_channels(), 16);
        assert_eq!(layout.output_neurons(), 16);
        assert!(layout.include_switches());
        assert_eq!(layout.tx_len(), Ok(SILICON_BRIDGE_V3_TX_LEN));
        assert_eq!(layout.rx_len(), Ok(SILICON_BRIDGE_V3_RX_LEN));
        assert_eq!(layout.spike_mask_bytes(), Ok(2));
    }

    #[test]
    fn golden_v3_request_is_byte_for_byte_compatible() {
        let stimuli = [
            0.0,
            0.5,
            1.0,
            -1.0,
            0.1,
            -0.1,
            127.99,
            -127.99,
            0.00390625,
            -0.00390625,
            2.0,
            -2.0,
            10.0,
            -10.0,
            0.25,
            -0.25,
        ];
        let checked = encode_stimuli(&v3(), &stimuli).expect("finite 16-vector");
        let legacy = encode_stimuli_legacy_v3(&stimuli);
        assert_eq!(checked, legacy);
        assert_eq!(checked[0], DENSE_Q88_SYNC);
        assert_eq!(checked.len(), 33);
        // 0.5 → 128 → 0080; -1.0 → -256 → FF00; 0.1 → 25 → 0019
        assert_eq!(&checked[1..3], &[0x00, 0x00]);
        assert_eq!(&checked[3..5], &[0x00, 0x80]);
        assert_eq!(&checked[5..7], &[0x01, 0x00]);
        assert_eq!(&checked[7..9], &[0xFF, 0x00]);
        assert_eq!(&checked[9..11], &[0x00, 0x19]);
    }

    #[test]
    fn legacy_wrapper_pads_and_truncates() {
        let short = encode_stimuli_legacy_v3(&[1.0, -1.0]);
        let mut expected = vec![DENSE_Q88_SYNC];
        expected.extend_from_slice(&encode_q88_signed(1.0).to_be_bytes());
        expected.extend_from_slice(&encode_q88_signed(-1.0).to_be_bytes());
        expected.extend_from_slice(&[0u8; 28]);
        assert_eq!(short, expected);

        let mut long = vec![0.5; 20];
        long[0] = -1.0;
        let truncated = encode_stimuli_legacy_v3(&long);
        let first_16: Vec<f32> = std::iter::once(-1.0)
            .chain(std::iter::repeat_n(0.5, 15))
            .collect();
        assert_eq!(truncated, encode_stimuli_legacy_v3(&first_16));
        assert_eq!(truncated.len(), 33);
    }

    #[test]
    fn checked_encode_rejects_wrong_length_before_any_bytes() {
        let err = encode_stimuli(&v3(), &[0.1; 15]).expect_err("short");
        assert_eq!(
            err,
            CodecError::WrongInputLength {
                expected: 16,
                actual: 15
            }
        );
        let err = encode_stimuli(&v3(), &[0.1; 17]).expect_err("long");
        assert_eq!(
            err,
            CodecError::WrongInputLength {
                expected: 16,
                actual: 17
            }
        );
    }

    #[test]
    fn checked_encode_rejects_non_finite_stimuli() {
        let mut stimuli = [0.1; 16];
        stimuli[3] = f32::NAN;
        assert_eq!(
            encode_stimuli(&v3(), &stimuli),
            Err(CodecError::NonFinite {
                index: 3,
                kind: NonFiniteKind::Nan
            })
        );
        stimuli[3] = f32::INFINITY;
        assert_eq!(
            encode_stimuli(&v3(), &stimuli),
            Err(CodecError::NonFinite {
                index: 3,
                kind: NonFiniteKind::PosInfinity
            })
        );
        stimuli[3] = f32::NEG_INFINITY;
        assert_eq!(
            encode_stimuli(&v3(), &stimuli),
            Err(CodecError::NonFinite {
                index: 3,
                kind: NonFiniteKind::NegInfinity
            })
        );
    }

    #[test]
    fn golden_v3_reply_round_trip() {
        let mut rx = vec![0u8; 36];
        // neuron 0 potential = -1.0 → FF00
        rx[0] = 0xFF;
        rx[1] = 0x00;
        // neuron 1 potential = 0.5 → 0080
        rx[2] = 0x00;
        rx[3] = 0x80;
        // spike bits 0 and 8 set: BE word 0x0101
        rx[32] = 0x01;
        rx[33] = 0x01;
        // switches 0x00A5
        rx[34] = 0x00;
        rx[35] = 0xA5;

        let decoded = decode_response(&v3(), &rx).expect("36-byte v3 reply");
        assert_eq!(decoded.potentials[0], -1.0);
        assert_eq!(decoded.potentials[1], 0.5);
        assert!(decoded.spikes[0]);
        assert!(decoded.spikes[8]);
        assert!(!decoded.spikes[1]);
        assert_eq!(decoded.switches, Some(0x00A5));
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert!(matches!(
            decode_response(&v3(), &[0u8; 35]),
            Err(CodecError::WrongFrameLength {
                expected: 36,
                actual: 35
            })
        ));
        assert!(matches!(
            decode_response(&v3(), &[0u8; 37]),
            Err(CodecError::WrongFrameLength {
                expected: 36,
                actual: 37
            })
        ));
    }

    #[test]
    fn dense_8_16_32_and_ragged_mask_shapes() {
        for (inputs, outputs, mask_bytes, rx) in [
            (8, 8, 1, 8 * 2 + 1 + 2),
            (16, 16, 2, 36),
            (32, 32, 4, 32 * 2 + 4 + 2),
            (8, 16, 2, 16 * 2 + 2 + 2),
            (16, 8, 1, 8 * 2 + 1 + 2),
            // 10 is not a multiple of 8: two mask bytes, bits 0..9 live.
            (10, 10, 2, 10 * 2 + 2 + 2),
        ] {
            let layout = DenseQ88Layout::dense(inputs, outputs).expect("supported dense shape");
            assert_eq!(layout.input_channels(), inputs);
            assert_eq!(layout.output_neurons(), outputs);
            assert_eq!(layout.spike_mask_bytes(), Ok(mask_bytes));
            assert_eq!(layout.rx_len(), Ok(rx));
            assert_eq!(layout.tx_len(), Ok(1 + inputs * 2));

            let stimuli = vec![0.25; inputs];
            let tx = encode_stimuli(&layout, &stimuli).expect("finite exact input");
            assert_eq!(tx[0], DENSE_Q88_SYNC);
            assert_eq!(tx.len(), 1 + inputs * 2);

            let mut reply = vec![0u8; rx];
            // Set neuron 0 spike (LSB of last mask byte) and a switch word.
            let mask_start = outputs * 2;
            reply[mask_start + mask_bytes - 1] = 0x01;
            reply[mask_start + mask_bytes] = 0x12;
            reply[mask_start + mask_bytes + 1] = 0x34;
            if outputs >= 10 {
                // neuron 9 = bit 1 of the high mask byte for a 2-byte mask.
                reply[mask_start + mask_bytes - 2] |= 1 << 1;
            }
            let decoded = decode_response(&layout, &reply).expect("shaped reply");
            assert_eq!(decoded.potentials.len(), outputs);
            assert_eq!(decoded.spikes.len(), outputs);
            assert!(decoded.spikes[0]);
            if outputs >= 10 {
                assert!(decoded.spikes[9], "neuron 9 must use the second mask byte");
            }
            assert_eq!(decoded.switches, Some(0x1234));
        }
    }

    #[test]
    fn dense_rejects_zero_and_oversize_layouts() {
        assert!(matches!(
            DenseQ88Layout::dense(0, 16),
            Err(CodecError::ZeroDimension { .. })
        ));
        assert!(matches!(
            DenseQ88Layout::dense(16, 0),
            Err(CodecError::ZeroDimension { .. })
        ));
        assert!(matches!(
            DenseQ88Layout::dense(MAX_DENSE_CHANNELS + 1, 16),
            Err(CodecError::ChannelLimit { .. })
        ));
        assert!(DenseQ88Layout::dense(MAX_DENSE_CHANNELS, MAX_DENSE_CHANNELS).is_ok());
    }

    #[test]
    fn eight_neuron_mask_is_a_single_byte() {
        let layout = DenseQ88Layout::dense(8, 8).unwrap();
        let mut rx = vec![0u8; layout.rx_len().unwrap()];
        rx[16] = 0b1000_0001; // neurons 0 and 7
        rx[17] = 0x00;
        rx[18] = 0x01;
        let decoded = decode_response(&layout, &rx).unwrap();
        assert!(decoded.spikes[0]);
        assert!(decoded.spikes[7]);
        assert!(!decoded.spikes[1]);
        assert_eq!(decoded.switches, Some(0x0001));
    }
}
