//! Shared plumbing of the direct-venue swap-event decoders (ADR-013 section
//! 2b, P4.9): a bounds-checked little-endian reader and the outcome type
//! every venue event classifier returns.

use scout_core::SolanaPubkey;

/// Outcome of classifying one Anchor event payload (`event discriminator (8)
/// ++ Borsh event`) of a venue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueEventOutcome<T> {
    /// A swap event that decoded exactly.
    Event(T),
    /// An event of the pinned IDL that is not a swap (liquidity, fees,
    /// admin...): named, never a leg, never "unknown".
    KnownNonLeg { name: &'static str },
    /// A discriminator outside the pinned IDL's event set. COVERAGE GAP.
    UnknownEvent { discriminator: [u8; 8] },
    /// Right discriminator, broken structure (length or a non-0/1 bool).
    /// COVERAGE GAP.
    Malformed { reason: String },
}

/// Outcome of parsing one instruction of a venue program as a swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapInstructionParse<S> {
    /// An instruction of the venue program that is not a swap (liquidity,
    /// admin...). Not a coverage gap: events decide.
    NotSwap,
    /// A swap-shaped instruction of the IDL whose event semantics are not
    /// evidenced (two-hop, router) or not implemented. Its events are never
    /// legs.
    Unsupported {
        name: &'static str,
    },
    /// A swap discriminator with the wrong data length or too few accounts.
    Malformed {
        reason: String,
    },
    Swap(S),
}

/// Bounds-checked sequential reader.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    pub(crate) fn pubkey(&mut self) -> Option<SolanaPubkey> {
        self.take(32)?.try_into().ok()
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub(crate) fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }

    pub(crate) fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    /// Strict Borsh bool: only `0` and `1` are valid.
    pub(crate) fn bool(&mut self) -> Option<bool> {
        match self.take(1)? {
            [0] => Some(false),
            [1] => Some(true),
            _ => None,
        }
    }
}

/// Splits `payload` into its 8-byte event discriminator and the Borsh body.
pub(crate) fn split_event(payload: &[u8]) -> Option<([u8; 8], &[u8])> {
    let disc: [u8; 8] = payload.get(..8)?.try_into().ok()?;
    Some((disc, payload.get(8..)?))
}

/// Name of the table event with this discriminator.
pub(crate) fn event_name(
    table: &[(&'static str, [u8; 8])],
    disc: &[u8; 8],
) -> Option<&'static str> {
    table.iter().find(|(_, d)| d == disc).map(|(n, _)| *n)
}

pub(crate) fn malformed<T>(reason: String) -> VenueEventOutcome<T> {
    VenueEventOutcome::Malformed { reason }
}
