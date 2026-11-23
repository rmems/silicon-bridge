// SPDX-License-Identifier: MIT OR Apache-2.0
//! SiliconBridge UART request/response codecs.
//!
//! Pure encode/decode for the host↔FPGA stimulus frame. This module does not
//! open serial ports, depend on `serialport`, or require an async runtime.
//! The optional `FpgaBridge` adapter (`uart` feature) is the I/O layer.
//!
//! ## Example layouts
//!
//! [`DenseQ88Layout::silicon_bridge_v3`] is the SiliconBridge v3.0 **example
//! layout** (16 in / 16 out, switch field) with committed golden bytes in
//! `tests/golden/uart/`. Those fixtures were recorded against Basys3
//! firmware; this crate does not only work with that board. Changing host
//! dimensions does **not** reconfigure FPGA firmware — a non-v3 layout needs
//! a matching firmware revision.
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
//! ## Legacy unframed replies
//!
//! Legacy RX has no sync byte, length, checksum, or request id. A decode
//! that sees the expected byte count cannot tell a stale or misaligned
//! same-length reply from a fresh one. This crate does not invent a
//! checksum or claim guaranteed resynchronization. Use the opt-in UART v4
//! codec ([`crate::encode_request_v4`] / [`crate::decode_response_v4`]) when
//! matching v4 firmware is available.

use crate::{
    AER_ROUTE_ENTRY_COUNT, AerRouteEntry, AerRouteError, AerRouteTable, NonFiniteKind,
    encode_q88_signed, q88_signed_to_f32,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Sync byte on every dense-Q8.8 request, including SiliconBridge v3.0.
pub const DENSE_Q88_SYNC: u8 = 0xAA;

/// Input and output count of the SiliconBridge v3.0 example layout.
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
    /// SiliconBridge v3.0 example layout: 16 inputs, 16 outputs, switch field.
    ///
    /// Golden-byte evidence lives in `tests/golden/uart/legacy_v3.json`
    /// (recorded against Basys3 firmware). This is not a claim that the crate
    /// only works with that board, and changing host channel counts does not
    /// reconfigure an FPGA.
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
        Self::dense_with_switches(input_channels, output_neurons, true)
    }

    /// Dense Q8.8 frame with an explicit optional 16-bit switch field.
    ///
    /// This is a host-side layout description. Matching FPGA firmware is
    /// required, especially when `include_switches` is false.
    pub fn dense_with_switches(
        input_channels: usize,
        output_neurons: usize,
        include_switches: bool,
    ) -> Result<Self, CodecError> {
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
        let _ = rx_len(output_neurons, include_switches)?;
        Ok(Self {
            input_channels,
            output_neurons,
            include_switches,
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
    /// A framed reply did not begin with the protocol sync byte.
    WrongSync {
        /// Required sync byte.
        expected: u8,
        /// Byte found in the frame.
        actual: u8,
    },
    /// A framed reply used an unsupported protocol version.
    UnsupportedVersion {
        /// Version implemented by this codec.
        expected: u8,
        /// Version advertised by the frame.
        actual: u8,
    },
    /// A framed reply advertised a payload length inconsistent with its layout.
    WrongPayloadLength {
        /// Payload length required by the layout.
        expected: usize,
        /// Payload length advertised in the frame header.
        actual: usize,
    },
    /// A framed reply belongs to a different request/tick.
    RequestIdMismatch {
        /// Request id supplied by the caller.
        expected: u32,
        /// Request id carried by the reply.
        actual: u32,
    },
    /// A framed reply failed its integrity check.
    ChecksumMismatch {
        /// CRC computed over the received frame fields.
        expected: u16,
        /// CRC carried by the reply.
        actual: u16,
    },
    /// A `0xA6` read request asked for zero words. The RTL consumes a
    /// zero-length request silently and emits no dump, so framing one would
    /// deadlock a host blocked in `read_exact`.
    ZeroReadLength,
    /// A `0xA6` dump frame did not echo the request's five-byte header.
    /// The echo is the only synchronization marker on this wire, so a
    /// mismatch means the byte stream is desynchronized — a stale 36-byte
    /// response streamed ahead of the dump, or the peer did not understand
    /// the read command — not a partial dump.
    ReadEchoMismatch {
        /// The five request bytes the dump was expected to echo.
        expected: [u8; RAM_READ_HEADER_LEN],
        /// The five header bytes actually received.
        actual: [u8; RAM_READ_HEADER_LEN],
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
            Self::WrongSync { expected, actual } => {
                write!(f, "reply sync is 0x{actual:02X}, expected 0x{expected:02X}")
            }
            Self::UnsupportedVersion { expected, actual } => write!(
                f,
                "reply version is {actual}, expected protocol version {expected}"
            ),
            Self::WrongPayloadLength { expected, actual } => write!(
                f,
                "reply advertises {actual} payload bytes, expected {expected}"
            ),
            Self::RequestIdMismatch { expected, actual } => write!(
                f,
                "reply request id is 0x{actual:08X}, expected 0x{expected:08X}"
            ),
            Self::ChecksumMismatch { expected, actual } => write!(
                f,
                "reply CRC-16 is 0x{actual:04X}, computed 0x{expected:04X}"
            ),
            Self::ZeroReadLength => write!(
                f,
                "RAM read of zero words is refused: the FPGA emits no dump for it"
            ),
            Self::ReadEchoMismatch { expected, actual } => write!(
                f,
                "RAM dump header is {:02X?}, expected echo of {:02X?} \
                 (byte stream is desynchronized)",
                actual, expected
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

/// Write-command byte of the `0xA5` runtime register write.
///
/// The five-byte frame is `[0xA5, target, address, data_hi, data_lo]`,
/// unchanged since SiliconBridge v3.0. Targets 0/1/2 (weight, threshold,
/// leak RAM) are outside this crate's scope; only
/// [`ROUTE_TABLE_TARGET`] is encoded here.
pub const ROUTE_WRITE_COMMAND: u8 = 0xA5;

/// `0xA5` target selecting the AER route table.
pub const ROUTE_TABLE_TARGET: u8 = 3;

/// Byte length of one `0xA5` route-write frame.
pub const ROUTE_WRITE_LEN: usize = 5;

/// Encode one target-3 route-write frame for a validated entry.
///
/// Bytes are `[0xA5, 0x03, address, data_hi, data_lo]` (big-endian 16-bit
/// entry word). `address` is the table index written, not the route's
/// delivered address.
///
/// # Errors
///
/// [`AerRouteError::AddressOutOfRange`] when `address` is 16 or above.
/// [`encode_route_write_word`] additionally checks the entry word.
pub fn encode_route_write(
    address: u8,
    entry: AerRouteEntry,
) -> Result<[u8; ROUTE_WRITE_LEN], AerRouteError> {
    if (address as usize) >= crate::AER_ROUTE_ENTRY_COUNT {
        return Err(AerRouteError::AddressOutOfRange { address });
    }
    let word = entry.word();
    Ok([
        ROUTE_WRITE_COMMAND,
        ROUTE_TABLE_TARGET,
        address,
        (word >> 8) as u8,
        word as u8,
    ])
}

/// Checked [`encode_route_write`] from a raw `(address, word)` pair.
///
/// # Errors
///
/// [`AerRouteError::AddressOutOfRange`] when `address` is 16 or above;
/// [`AerRouteError::ReservedBits`] when `word` sets reserved bits 13:4.
pub fn encode_route_write_word(
    address: u8,
    word: u16,
) -> Result<[u8; ROUTE_WRITE_LEN], AerRouteError> {
    if (address as usize) >= crate::AER_ROUTE_ENTRY_COUNT {
        return Err(AerRouteError::AddressOutOfRange { address });
    }
    let entry = AerRouteEntry::from_word(word, address)?;
    encode_route_write(address, entry)
}

/// Encode a whole table: 16 frames in ascending address order.
///
/// # Errors
///
/// [`AerRouteError`] when `table` fails [`AerRouteTable::validate`]
/// (over-budget or dangling routes); malformed tables are never framed.
pub fn encode_route_table(
    table: &AerRouteTable,
) -> Result<[[u8; ROUTE_WRITE_LEN]; AER_ROUTE_ENTRY_COUNT], AerRouteError> {
    table.validate()?;
    let mut frames = [[0u8; ROUTE_WRITE_LEN]; AER_ROUTE_ENTRY_COUNT];
    for (address, frame) in frames.iter_mut().enumerate() {
        let entry = table
            .entry(address as u8)
            .expect("address is within the fixed table");
        *frame =
            encode_route_write(address as u8, entry).expect("address is within the fixed table");
    }
    Ok(frames)
}

/// Read-command byte of the `0xA6` runtime RAM read (silicon-hdl #113).
///
/// The five-byte request is `[0xA6, target, address, len_hi, len_lo]`; a
/// valid request streams back a **dump frame**: the same five bytes as an
/// echoed header, then `len` big-endian signed Q8.8 words in ascending
/// address order (the 8-bit address wraps at 256, the RAM depth). The echo
/// is the only synchronization marker — dumps share the UART serializer
/// with 36-byte stimulus responses, and a response already in flight when
/// the request lands is streamed first, so a dump may legitimately start
/// later than the request that caused it.
pub const RAM_READ_COMMAND: u8 = 0xA6;

/// Byte length of one `0xA6` read-request frame.
pub const RAM_READ_REQ_LEN: usize = 5;

/// Byte length of the echoed header that opens every `0xA6` dump frame.
pub const RAM_READ_HEADER_LEN: usize = 5;

/// Readable `0xA6` target, sharing the `0xA5` write-target numbering.
///
/// Target 3 (the AER route table) is intentionally absent: it has no
/// config read port on the FPGA, so it can never be selected here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RamReadTarget {
    /// `WeightRam` — the learned 16×16 bank (target 0).
    Weight = 0,
    /// Threshold `NeuronParamRam` (target 1).
    Threshold = 1,
    /// Leak `NeuronParamRam` (target 2).
    Leak = 2,
}

impl RamReadTarget {
    /// `0xA5`/`0xA6` target byte for this bank.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// The bank a target byte selects, or `None` for 3 (AER route table,
    /// write-only) and anything above.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Weight),
            1 => Some(Self::Threshold),
            2 => Some(Self::Leak),
            _ => None,
        }
    }

    /// Lowercase bank name, matching `FpgaParameters` field naming and the
    /// `.mem` bank files (`weights`/`thresholds`/`decay` use their own
    /// names; this is the wire target's).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Weight => "weight",
            Self::Threshold => "threshold",
            Self::Leak => "leak",
        }
    }
}

impl fmt::Display for RamReadTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One decoded `0xA6` dump frame: the bank, the first address, and the raw
/// words in read order.
///
/// `words` carries the signed two's-complement Q8.8 patterns exactly as
/// they left the RAM — the same interpretation `FpgaParameters` vectors
/// and `.mem` images use, so the dump can feed parity comparisons and
/// `.mem` re-emission without further decoding (silicon-hdl #113).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RamDump {
    /// Bank the words were read from.
    pub target: RamReadTarget,
    /// First address the dump covered.
    pub start_address: u8,
    /// Raw Q8.8 words, `start_address` ascending (wrapping at 256).
    pub words: Vec<i16>,
}

impl RamDump {
    /// `$readmemh` lines for [`Self::words`]: uppercase `{:04X}`
    /// two's-complement hex, one word per line — the format
    /// `WeightRam`/`NeuronParamRam` `INIT_FILE` images and
    /// [`crate::MemFileWriter`] output already use.
    ///
    /// A dump that starts above address 0 is prefixed with a `@hh`
    /// address directive so its words land at the captured addresses when
    /// re-flashed, and a dump that wraps past 255 resumes behind a `@00`
    /// directive. Only a complete, zero-start dump therefore reproduces a
    /// bank image verbatim; a partial dump rewrites just the covered
    /// range and leaves the rest to the image's other lines or RAM
    /// initialization.
    pub fn mem_lines(&self) -> impl Iterator<Item = String> + '_ {
        let start = usize::from(self.start_address);
        // Words past address 255 wrap to 0 in the RTL counter, so the
        // image needs a second address directive at the wrap point.
        let split = (256 - start).min(self.words.len());
        let hex = |word: &i16| format!("{:04X}", *word as u16);
        (start != 0)
            .then(|| format!("@{start:02X}"))
            .into_iter()
            .chain(self.words[..split].iter().map(hex))
            .chain((split < self.words.len()).then(|| "@00".to_string()))
            .chain(self.words[split..].iter().map(hex))
    }
}

/// Encode one `0xA6` read-request frame.
///
/// Bytes are `[0xA6, target, address, len_hi, len_lo]` (big-endian 16-bit
/// word count).
///
/// The start address is a single byte and `address + word_count` past 255
/// is deliberately **not** rejected: `SocProtocolFsm.sv` advances an
/// 8-bit `dump_addr` counter per word, so the range wraps modulo 256 and
/// reads the low addresses it crosses. Callers that want bank-linear
/// output keep `address + word_count <= 256`.
///
/// # Errors
///
/// [`CodecError::ZeroReadLength`] when `word_count` is 0: the FPGA consumes
/// such a request silently and no dump follows, so refusing here keeps a
/// host out of a guaranteed `read_exact` timeout.
pub fn encode_ram_read(
    target: RamReadTarget,
    address: u8,
    word_count: u16,
) -> Result<[u8; RAM_READ_REQ_LEN], CodecError> {
    if word_count == 0 {
        return Err(CodecError::ZeroReadLength);
    }
    Ok([
        RAM_READ_COMMAND,
        target.code(),
        address,
        (word_count >> 8) as u8,
        word_count as u8,
    ])
}

/// Total byte length of the dump frame a `word_count`-word request
/// returns: the five echoed header bytes plus two bytes per word.
pub const fn ram_dump_frame_len(word_count: u16) -> usize {
    RAM_READ_HEADER_LEN + 2 * (word_count as usize)
}

/// Decode a complete `0xA6` dump frame (echoed header + payload words).
///
/// `frame` must be exactly [`ram_dump_frame_len`]`(word_count)` bytes and
/// its header must equal the request's five bytes. Anything else means the
/// byte stream is desynchronized — there is no payload-level checksum —
/// so the error is [`CodecError::ReadEchoMismatch`], not a best-effort
/// decode.
///
/// # Errors
///
/// [`CodecError::ZeroReadLength`] when `word_count` is 0 — the FPGA emits
/// no dump for a zero-length request, so accepting a bare echoed header
/// would pass an impossible response off as a successful read;
/// [`CodecError::WrongFrameLength`] for a wrong byte count;
/// [`CodecError::ReadEchoMismatch`] for a header that does not echo the
/// request.
pub fn decode_ram_dump(
    target: RamReadTarget,
    start_address: u8,
    word_count: u16,
    frame: &[u8],
) -> Result<RamDump, CodecError> {
    if word_count == 0 {
        return Err(CodecError::ZeroReadLength);
    }
    let expected_len = ram_dump_frame_len(word_count);
    if frame.len() != expected_len {
        return Err(CodecError::WrongFrameLength {
            expected: expected_len,
            actual: frame.len(),
        });
    }
    let expected_header: [u8; RAM_READ_HEADER_LEN] = [
        RAM_READ_COMMAND,
        target.code(),
        start_address,
        (word_count >> 8) as u8,
        word_count as u8,
    ];
    let actual_header: [u8; RAM_READ_HEADER_LEN] = frame[..RAM_READ_HEADER_LEN]
        .try_into()
        .expect("length checked");
    if actual_header != expected_header {
        return Err(CodecError::ReadEchoMismatch {
            expected: expected_header,
            actual: actual_header,
        });
    }
    let mut words = Vec::with_capacity(usize::from(word_count));
    for i in 0..usize::from(word_count) {
        let b = RAM_READ_HEADER_LEN + i * 2;
        words.push(i16::from_be_bytes([frame[b], frame[b + 1]]));
    }
    Ok(RamDump {
        target,
        start_address,
        words,
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

    #[test]
    fn route_write_frame_is_five_bytes_big_endian() {
        let frame = encode_route_write(3, AerRouteEntry::from_word(0xC003, 3).unwrap()).unwrap();
        assert_eq!(frame, [0xA5, 0x03, 0x03, 0xC0, 0x03]);
    }

    #[test]
    fn route_write_word_validates_address_and_reserved_bits() {
        assert_eq!(
            encode_route_write_word(16, 0xC000),
            Err(AerRouteError::AddressOutOfRange { address: 16 })
        );
        assert_eq!(
            encode_route_write_word(2, 0xC010),
            Err(AerRouteError::ReservedBits {
                address: 2,
                word: 0xC010
            })
        );
    }

    #[test]
    fn route_table_encodes_sixteen_frames_in_address_order() {
        let frames = encode_route_table(&AerRouteTable::identity()).unwrap();
        assert_eq!(frames.len(), 16);
        for (address, frame) in frames.iter().enumerate() {
            let a = address as u8;
            assert_eq!(frame[0], ROUTE_WRITE_COMMAND);
            assert_eq!(frame[1], ROUTE_TABLE_TARGET);
            assert_eq!(frame[2], a);
            assert_eq!(&frame[3..5], &((0xC000 | a as u16).to_be_bytes()));
        }
    }

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

    #[test]
    fn ram_read_frame_is_five_bytes_big_endian() {
        let frame = encode_ram_read(RamReadTarget::Weight, 0x10, 0x0100).unwrap();
        assert_eq!(frame, [0xA6, 0x00, 0x10, 0x01, 0x00]);
        let frame = encode_ram_read(RamReadTarget::Leak, 0xFE, 3).unwrap();
        assert_eq!(frame, [0xA6, 0x02, 0xFE, 0x00, 0x03]);
    }

    #[test]
    fn ram_read_rejects_zero_word_count() {
        assert_eq!(
            encode_ram_read(RamReadTarget::Weight, 0, 0),
            Err(CodecError::ZeroReadLength)
        );
    }

    #[test]
    fn ram_read_target_codes_match_write_targets_and_explain_missing_three() {
        assert_eq!(RamReadTarget::Weight.code(), 0);
        assert_eq!(RamReadTarget::Threshold.code(), 1);
        assert_eq!(RamReadTarget::Leak.code(), 2);
        for code in 0u8..=2 {
            assert_eq!(RamReadTarget::from_code(code).unwrap().code(), code);
        }
        // The AER route table (write target 3) has no read port on the FPGA.
        assert_eq!(RamReadTarget::from_code(3), None);
        assert_eq!(RamReadTarget::from_code(0xFF), None);
    }

    #[test]
    fn ram_dump_frame_len_counts_header_plus_two_bytes_per_word() {
        assert_eq!(ram_dump_frame_len(1), 7);
        assert_eq!(ram_dump_frame_len(256), 517);
    }

    #[test]
    fn ram_dump_round_trip_decodes_echo_and_words() {
        let request = encode_ram_read(RamReadTarget::Threshold, 0x03, 3).unwrap();
        let mut frame = request.to_vec();
        for word in [0x0200i16, -256i16, 0x7FFFu16 as i16] {
            frame.extend_from_slice(&word.to_be_bytes());
        }
        let dump = decode_ram_dump(RamReadTarget::Threshold, 0x03, 3, &frame).unwrap();
        assert_eq!(dump.target, RamReadTarget::Threshold);
        assert_eq!(dump.start_address, 0x03);
        assert_eq!(dump.words, vec![0x0200, -256, 0x7FFF]);
    }

    #[test]
    fn ram_dump_rejects_wrong_length_and_bad_echo() {
        let request = encode_ram_read(RamReadTarget::Weight, 0x00, 2).unwrap();
        let mut frame = request.to_vec();
        frame.extend_from_slice(&[0xBE, 0xEF, 0xCA, 0xFE]);
        assert!(matches!(
            decode_ram_dump(RamReadTarget::Weight, 0x00, 2, &frame[..8]),
            Err(CodecError::WrongFrameLength {
                expected: 9,
                actual: 8
            })
        ));
        frame[1] = 0x01; // echoes the wrong target — the only sync check there is
        assert!(matches!(
            decode_ram_dump(RamReadTarget::Weight, 0x00, 2, &frame),
            Err(CodecError::ReadEchoMismatch { .. })
        ));
    }

    #[test]
    fn ram_dump_mem_lines_match_readmemh_format() {
        let dump = RamDump {
            target: RamReadTarget::Leak,
            start_address: 0x04,
            words: vec![0x0080, -256, 0x7FFF],
        };
        // a nonzero start carries an address directive so a re-flash lands
        // the words back at 0x04 instead of silently shifting them to 0
        let lines: Vec<String> = dump.mem_lines().collect();
        assert_eq!(lines, vec!["@04", "0080", "FF00", "7FFF"]);
    }

    #[test]
    fn ram_dump_mem_lines_zero_start_stays_plain() {
        let dump = RamDump {
            target: RamReadTarget::Weight,
            start_address: 0x00,
            words: vec![0x0080, -256],
        };
        let lines: Vec<String> = dump.mem_lines().collect();
        assert_eq!(lines, vec!["0080", "FF00"]);
    }

    #[test]
    fn ram_dump_mem_lines_resume_at_zero_after_wrap() {
        let dump = RamDump {
            target: RamReadTarget::Weight,
            start_address: 0xFE,
            words: vec![0x0001, 0x0002, 0x0003, 0x0004],
        };
        let lines: Vec<String> = dump.mem_lines().collect();
        // 0xFE..0xFF under @FE, then the wrapped words under @00
        assert_eq!(lines, vec!["@FE", "0001", "0002", "@00", "0003", "0004"]);
    }

    #[test]
    fn decode_ram_dump_rejects_zero_word_count() {
        let request = encode_ram_read(RamReadTarget::Weight, 0x00, 1).unwrap();
        let mut echo = request;
        echo[4] = 0; // fabricate a zero-length echoed header
        assert_eq!(
            decode_ram_dump(RamReadTarget::Weight, 0x00, 0, &echo),
            Err(CodecError::ZeroReadLength)
        );
    }
}
