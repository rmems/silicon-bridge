// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `aer-route-v1` bounded routing table consumed by silicon-hdl.
//!
//! silicon-hdl `AerRouteTable` routes one selected ingress lane through a
//! fixed 16-entry, 16-bit table before the LIF weight-column selector and
//! STDP pre-event capture. The entry ABI is:
//!
//! | Bits | Field | Meaning |
//! |---|---|---|
//! | 15 | valid | `0` drops the event and pulses `route_fault` |
//! | 14 | terminal | `1` delivers `next_addr`; `0` performs another lookup |
//! | 13:4 | reserved | must be `0`; any set bit fails closed |
//! | 3:0 | next_addr | address delivered or used for the next lookup |
//!
//! The identity entry for address `i` is `0xC000 | i`. A lookup chain is
//! bounded by [`AER_ROUTE_MAX_HOPS`]: the initial access counts as lookup 1
//! and lookup 4 must be terminal, otherwise the event is dropped with a
//! route fault.
//!
//! Ordinary NIR edges never describe physical FPGA hops. An
//! [`AerRouteTable`] is either the identity derived from canonical input
//! ordering ([`AerRouteSource::CanonicalInputOrder`]) or an explicit
//! hardware routing plan supplied out of band
//! ([`AerRouteSource::ExplicitHardwarePlan`]).
//!
//! The runtime-write command is the existing five-byte `0xA5` frame with
//! target [`ROUTE_TABLE_TARGET`]; see [`crate::fpga_codec`].

use serde::{Deserialize, Serialize};
use std::fmt;

/// Schema identifier recorded in export manifests.
pub const AER_ROUTE_SCHEMA: &str = "aer-route-v1";

/// Number of table entries.
pub const AER_ROUTE_ENTRY_COUNT: usize = 16;

/// Bits per table entry.
pub const AER_ROUTE_ENTRY_WIDTH: u8 = 16;

/// Bits of address inside an entry (`next_addr[3:0]`) and table index.
pub const AER_ROUTE_ADDRESS_WIDTH: u8 = 4;

/// Maximum lookups per routed event. The initial access counts as lookup 1;
/// lookup [`AER_ROUTE_MAX_HOPS`] must be terminal.
pub const AER_ROUTE_MAX_HOPS: usize = 4;

/// `$readmemh` image filename emitted by the `silicon-hdl-v3 + aer-route-v1`
/// profile.
pub const AER_ROUTE_FILENAME: &str = "aer_routes.mem";

/// Valid bit (15) of a table entry.
pub const AER_ENTRY_VALID: u16 = 0x8000;

/// Terminal bit (14) of a table entry.
pub const AER_ENTRY_TERMINAL: u16 = 0x4000;

/// Reserved bits (13:4) of a table entry. Must be zero.
pub const AER_ENTRY_RESERVED_MASK: u16 = 0x3FF0;

/// `next_addr` field mask (bits 3:0).
pub const AER_ENTRY_NEXT_MASK: u16 = 0x000F;

/// One 16-bit `aer-route-v1` table entry.
///
/// Construct through [`AerRouteEntry::terminal`],
/// [`AerRouteEntry::forwarding`], [`AerRouteEntry::invalid`], or
/// [`AerRouteEntry::from_word`]. Reserved bits can never be set on a stored
/// entry: `from_word` rejects them with [`AerRouteError::ReservedBits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AerRouteEntry {
    word: u16,
}

impl AerRouteEntry {
    /// Parse a raw 16-bit word. Reserved bits 13:4 must be zero.
    ///
    /// `address` is the entry's position in the table, used only for error
    /// reporting.
    ///
    /// # Errors
    ///
    /// [`AerRouteError::ReservedBits`] when any reserved bit is set.
    pub fn from_word(word: u16, address: u8) -> Result<Self, AerRouteError> {
        if word & AER_ENTRY_RESERVED_MASK != 0 {
            return Err(AerRouteError::ReservedBits { address, word });
        }
        Ok(Self { word })
    }

    /// Valid terminal entry delivering `next_addr` (`0xC000 | next_addr`).
    ///
    /// # Errors
    ///
    /// [`AerRouteError::AddressOutOfRange`] when `next_addr` exceeds the
    /// 4-bit field.
    pub fn terminal(next_addr: u8) -> Result<Self, AerRouteError> {
        Self::compose(AER_ENTRY_VALID | AER_ENTRY_TERMINAL, next_addr)
    }

    /// Valid forwarding entry continuing the lookup at `next_addr` (`0x8000 | next_addr`).
    ///
    /// # Errors
    ///
    /// [`AerRouteError::AddressOutOfRange`] when `next_addr` exceeds the
    /// 4-bit field.
    pub fn forwarding(next_addr: u8) -> Result<Self, AerRouteError> {
        Self::compose(AER_ENTRY_VALID, next_addr)
    }

    /// Invalid entry (`0x0000`): drops the event and pulses `route_fault` at
    /// runtime. As a *start* entry this is a deliberate disabled route and
    /// passes [`AerRouteTable::validate`]; a chain that forwards into one
    /// fails as [`AerRouteError::DanglingRoute`].
    pub fn invalid() -> Self {
        Self { word: 0 }
    }

    fn compose(flags: u16, next_addr: u8) -> Result<Self, AerRouteError> {
        if next_addr >= AER_ROUTE_ENTRY_COUNT as u8 {
            return Err(AerRouteError::AddressOutOfRange { address: next_addr });
        }
        Ok(Self {
            word: flags | u16::from(next_addr),
        })
    }

    /// The raw 16-bit word (`{:04X}` in the `.mem` image).
    pub fn word(self) -> u16 {
        self.word
    }

    /// Bit 15: entry participates in routing.
    pub fn is_valid(self) -> bool {
        self.word & AER_ENTRY_VALID != 0
    }

    /// Bit 14: this lookup delivers `next_addr` instead of forwarding.
    pub fn is_terminal(self) -> bool {
        self.word & AER_ENTRY_TERMINAL != 0
    }

    /// Bits 3:0: delivered address (terminal) or next lookup index.
    pub fn next_address(self) -> u8 {
        (self.word & AER_ENTRY_NEXT_MASK) as u8
    }
}

/// How the route table contents were chosen. Recorded in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AerRouteSource {
    /// Identity table derived from the canonical input ordering. For a
    /// NIR-lowered graph this is the `Input` lane order; it is **not**
    /// inferred from NIR edges.
    CanonicalInputOrder,
    /// Caller-supplied hardware routing plan validated against the
    /// `aer-route-v1` bounds.
    ExplicitHardwarePlan,
}

/// A validated 16-entry `aer-route-v1` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AerRouteTable {
    entries: [AerRouteEntry; AER_ROUTE_ENTRY_COUNT],
}

impl AerRouteTable {
    /// Identity table: entry `i` is `0xC000 | i` and delivers lane `i`.
    pub fn identity() -> Self {
        let mut entries = [AerRouteEntry::invalid(); AER_ROUTE_ENTRY_COUNT];
        for (i, entry) in entries.iter_mut().enumerate() {
            *entry = AerRouteEntry {
                word: AER_ENTRY_VALID | AER_ENTRY_TERMINAL | i as u16,
            };
        }
        Self { entries }
    }

    /// Table from 16 raw words. Each word is checked with
    /// [`AerRouteEntry::from_word`].
    ///
    /// # Errors
    ///
    /// [`AerRouteError::WrongEntryCount`] when `words` is not 16 entries;
    /// [`AerRouteError::ReservedBits`] when an entry sets bits 13:4.
    pub fn from_words(words: &[u16]) -> Result<Self, AerRouteError> {
        if words.len() != AER_ROUTE_ENTRY_COUNT {
            return Err(AerRouteError::WrongEntryCount {
                actual: words.len(),
            });
        }
        let mut entries = [AerRouteEntry::invalid(); AER_ROUTE_ENTRY_COUNT];
        for (i, &word) in words.iter().enumerate() {
            entries[i] = AerRouteEntry::from_word(word, i as u8)?;
        }
        Ok(Self { entries })
    }

    /// Table entry at `address` (0..16).
    pub fn entry(&self, address: u8) -> Option<AerRouteEntry> {
        self.entries.get(address as usize).copied()
    }

    /// Copy of the table with entry `address` replaced.
    ///
    /// # Errors
    ///
    /// [`AerRouteError::AddressOutOfRange`] when `address` is 16 or above.
    pub fn with_entry(&self, address: u8, entry: AerRouteEntry) -> Result<Self, AerRouteError> {
        if address as usize >= AER_ROUTE_ENTRY_COUNT {
            return Err(AerRouteError::AddressOutOfRange { address });
        }
        let mut table = self.clone();
        table.entries[address as usize] = entry;
        Ok(table)
    }

    /// The 16 words in ascending address order, for the `.mem` image.
    pub fn words(&self) -> [u16; AER_ROUTE_ENTRY_COUNT] {
        self.entries.map(|entry| entry.word())
    }

    /// Whether every start address routes to a terminal delivery inside the
    /// hop budget.
    ///
    /// Rules, matching `AerRouteTable.sv`:
    ///
    /// - A start entry with `valid = 0` is allowed (a deliberately disabled
    ///   route; the hardware drops the event with `route_fault`).
    /// - The initial access is lookup 1; lookup [`AER_ROUTE_MAX_HOPS`] must
    ///   be terminal or the route fails as [`AerRouteError::OverBudget`].
    ///   Cycles fail the same way.
    /// - A valid non-terminal chain that reaches an invalid entry fails as
    ///   [`AerRouteError::DanglingRoute`].
    ///
    /// Reserved bits are already impossible on stored entries, so only
    /// `valid` and `terminal` participate here.
    ///
    /// # Errors
    ///
    /// [`AerRouteError::OverBudget`] / [`AerRouteError::DanglingRoute`].
    pub fn validate(&self) -> Result<(), AerRouteError> {
        for start in 0..AER_ROUTE_ENTRY_COUNT as u8 {
            self.validate_from(start)?;
        }
        Ok(())
    }

    /// [`Self::validate`] for a single start address.
    ///
    /// # Errors
    ///
    /// [`AerRouteError::AddressOutOfRange`], [`AerRouteError::OverBudget`],
    /// [`AerRouteError::DanglingRoute`].
    pub fn validate_from(&self, start: u8) -> Result<(), AerRouteError> {
        if start as usize >= AER_ROUTE_ENTRY_COUNT {
            return Err(AerRouteError::AddressOutOfRange { address: start });
        }
        let mut at = start;
        if !self.entries[at as usize].is_valid() {
            return Ok(());
        }
        for lookup in 1..=AER_ROUTE_MAX_HOPS {
            let entry = self.entries[at as usize];
            if entry.is_terminal() {
                return Ok(());
            }
            if lookup == AER_ROUTE_MAX_HOPS {
                return Err(AerRouteError::OverBudget { start });
            }
            at = entry.next_address();
            if !self.entries[at as usize].is_valid() {
                return Err(AerRouteError::DanglingRoute { start, at });
            }
        }
        Ok(())
    }
}

/// `aer-route-v1` table or plan validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AerRouteError {
    /// An entry word set reserved bits 13:4.
    ReservedBits {
        /// Table index of the offending entry (or the supplied address).
        address: u8,
        /// The rejected word.
        word: u16,
    },
    /// A table index or `next_addr` outside 0..16.
    AddressOutOfRange {
        /// The rejected address.
        address: u8,
    },
    /// [`AerRouteTable::from_words`] got a slice that is not 16 entries.
    WrongEntryCount {
        /// Slice length supplied.
        actual: usize,
    },
    /// Route from `start` still forwards at lookup [`AER_ROUTE_MAX_HOPS`];
    /// includes cycles.
    OverBudget {
        /// Start address of the offending route.
        start: u8,
    },
    /// A valid non-terminal chain reached an invalid entry.
    DanglingRoute {
        /// Start address of the offending route.
        start: u8,
        /// The invalid entry the chain forwarded into.
        at: u8,
    },
}

impl fmt::Display for AerRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReservedBits { address, word } => write!(
                f,
                "aer-route entry {address} sets reserved bits 13:4 (word 0x{word:04X})"
            ),
            Self::AddressOutOfRange { address } => write!(
                f,
                "aer-route address {address} is outside 0..{AER_ROUTE_ENTRY_COUNT}"
            ),
            Self::WrongEntryCount { actual } => write!(
                f,
                "aer-route table needs exactly {AER_ROUTE_ENTRY_COUNT} entries, got {actual}"
            ),
            Self::OverBudget { start } => write!(
                f,
                "route from {start} does not terminate within {AER_ROUTE_MAX_HOPS} lookups"
            ),
            Self::DanglingRoute { start, at } => {
                write!(f, "route from {start} forwards into invalid entry {at}")
            }
        }
    }
}

impl std::error::Error for AerRouteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_words_are_c000_or_address() {
        let words = AerRouteTable::identity().words();
        for (i, word) in words.iter().enumerate() {
            assert_eq!(*word, 0xC000 | i as u16);
        }
    }

    #[test]
    fn identity_table_validates() {
        assert_eq!(AerRouteTable::identity().validate(), Ok(()));
    }

    #[test]
    fn from_word_rejects_reserved_bits() {
        for word in [0xC010, 0x8010, 0xFFF0, 0x0010] {
            assert_eq!(
                AerRouteEntry::from_word(word, 3),
                Err(AerRouteError::ReservedBits { address: 3, word })
            );
        }
    }

    #[test]
    fn constructors_reject_out_of_range_next_addr() {
        assert_eq!(
            AerRouteEntry::terminal(16),
            Err(AerRouteError::AddressOutOfRange { address: 16 })
        );
        assert_eq!(
            AerRouteEntry::forwarding(200),
            Err(AerRouteError::AddressOutOfRange { address: 200 })
        );
    }

    #[test]
    fn entry_accessors_decompose_the_word() {
        let e = AerRouteEntry::from_word(0xC005, 0).unwrap();
        assert!(e.is_valid());
        assert!(e.is_terminal());
        assert_eq!(e.next_address(), 5);
        let f = AerRouteEntry::from_word(0x8007, 0).unwrap();
        assert!(f.is_valid());
        assert!(!f.is_terminal());
        assert_eq!(f.next_address(), 7);
    }

    #[test]
    fn from_words_checks_count_then_entries() {
        assert_eq!(
            AerRouteTable::from_words(&[0xC000; 15]),
            Err(AerRouteError::WrongEntryCount { actual: 15 })
        );
        let mut words = [0xC000u16; 16];
        words[4] = 0xC010;
        assert_eq!(
            AerRouteTable::from_words(&words),
            Err(AerRouteError::ReservedBits {
                address: 4,
                word: 0xC010
            })
        );
    }

    #[test]
    fn multi_hop_chain_within_budget_validates() {
        // 0 -> 1 -> 2 delivers address 2 (synthetic multi-hop case).
        let mut words = [0xC000u16; 16];
        words[0] = 0x8001; // forward to entry 1
        words[1] = 0x8002; // forward to entry 2
        words[2] = 0xC002; // terminal, deliver 2
        assert_eq!(
            AerRouteTable::from_words(&words).unwrap().validate(),
            Ok(())
        );
    }

    #[test]
    fn four_lookup_chain_validates_five_fails() {
        // 0 -> 1 -> 2 -> 3 terminal at lookup 4: allowed.
        let mut words = [0x0000u16; 16];
        words[0] = 0x8001;
        words[1] = 0x8002;
        words[2] = 0x8003;
        words[3] = 0xC003;
        let ok = AerRouteTable::from_words(&words).unwrap();
        assert_eq!(ok.validate_from(0), Ok(()));

        // One extra hop: the fifth lookup would be needed.
        words[3] = 0x8004;
        words[4] = 0xC004;
        let over = AerRouteTable::from_words(&words).unwrap();
        assert_eq!(
            over.validate_from(0),
            Err(AerRouteError::OverBudget { start: 0 })
        );
    }

    #[test]
    fn cycle_fails_as_over_budget() {
        let mut words = [0xC000u16; 16];
        words[0] = 0x8001;
        words[1] = 0x8000; // 0 <-> 1
        let table = AerRouteTable::from_words(&words).unwrap();
        assert_eq!(
            table.validate_from(0),
            Err(AerRouteError::OverBudget { start: 0 })
        );
        assert_eq!(
            table.validate_from(1),
            Err(AerRouteError::OverBudget { start: 1 })
        );
    }

    #[test]
    fn chain_into_invalid_entry_is_dangling() {
        let mut words = [0xC000u16; 16];
        words[0] = 0x8005;
        words[5] = 0x0000;
        let table = AerRouteTable::from_words(&words).unwrap();
        assert_eq!(
            table.validate_from(0),
            Err(AerRouteError::DanglingRoute { start: 0, at: 5 })
        );
    }

    #[test]
    fn invalid_start_entry_is_allowed() {
        let mut words = [0xC000u16; 16];
        words[7] = 0x0000;
        let table = AerRouteTable::from_words(&words).unwrap();
        assert_eq!(table.validate_from(7), Ok(()));
        assert_eq!(table.validate(), Ok(()));
    }

    #[test]
    fn with_entry_replaces_and_bounds_check() {
        let table = AerRouteTable::identity();
        let updated = table
            .with_entry(3, AerRouteEntry::forwarding(4).unwrap())
            .unwrap();
        assert_eq!(updated.entry(3).unwrap().word(), 0x8004);
        assert_eq!(table.entry(3).unwrap().word(), 0xC003);
        assert_eq!(
            table.with_entry(16, AerRouteEntry::invalid()),
            Err(AerRouteError::AddressOutOfRange { address: 16 })
        );
        assert_eq!(
            table.validate_from(16),
            Err(AerRouteError::AddressOutOfRange { address: 16 })
        );
    }
}
