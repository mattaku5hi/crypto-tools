//! Tier 1 decoder trait. See ADR-008 for the `DecodeOutcome` semantics
//! this fixes: `NotMine` (a payload simply isn't this decoder's event
//! shape — normal, expected when scanning a block's mixed logs against
//! several decoders) is a distinct case from `Malformed` (the payload
//! looked like a match but its structure is broken — AGENTS.md
//! invariant #18, never silently skipped).

/// Result of attempting to decode one raw payload against one protocol
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeOutcome<T> {
    /// This payload does not match the decoder's protocol shape at all
    /// (wrong event signature, wrong instruction discriminator, ...).
    /// Normal and expected — a registry trying several decoders against
    /// one payload sees this from every decoder except the right one.
    NotMine,
    /// Successfully decoded.
    Decoded(T),
    /// The payload matched this decoder's protocol shape (signature/
    /// discriminator) but its structure is broken in some other way
    /// (wrong topic count, wrong data length, ...). Per AGENTS.md
    /// invariant #18, this must never be silently treated the same as
    /// `NotMine` — it is real evidence of an unfamiliar/corrupted
    /// format that needs to surface, not disappear.
    Malformed(String),
}

impl<T> DecodeOutcome<T> {
    #[must_use]
    pub fn is_not_mine(&self) -> bool {
        matches!(self, DecodeOutcome::NotMine)
    }

    #[must_use]
    pub fn is_malformed(&self) -> bool {
        matches!(self, DecodeOutcome::Malformed(_))
    }

    #[must_use]
    pub fn decoded(self) -> Option<T> {
        match self {
            DecodeOutcome::Decoded(value) => Some(value),
            DecodeOutcome::NotMine | DecodeOutcome::Malformed(_) => None,
        }
    }
}

/// A chain-block-or-slot boundary, used by `DeploymentScope` to bound a
/// decoder's declared activation range. Left as a bare number here (EVM
/// block number or Solana slot — the caller's `ChainKey.family`
/// disambiguates which) rather than introducing a new type per family;
/// callers scope decoders per chain family already via `ChainKey`.
pub type BlockOrSlot = u64;

/// The deployment a decoder is registered against: which chain, which
/// contract/program addresses, and what block/slot range it is active
/// for. Mandatory at registration (AGENTS.md invariant #16: "Проверяются
/// deployment, диапазон блоков, ABI/IDL..." — a decoder without a scope
/// would implicitly claim global support for its protocol shape across
/// every chain and address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentScope {
    pub chain: scout_core::ChainKey,
    pub contract_addresses: Vec<scout_core::AddressBytes>,
    pub active_from: BlockOrSlot,
    /// `None` means still active as of the last check — not "forever."
    pub active_until: Option<BlockOrSlot>,
}

impl DeploymentScope {
    /// Whether `position` (block number or slot) falls within this
    /// scope's declared activation range.
    #[must_use]
    pub fn covers_position(&self, position: BlockOrSlot) -> bool {
        position >= self.active_from && self.active_until.is_none_or(|until| position <= until)
    }

    /// Whether `address` is one of this scope's declared contract
    /// addresses.
    #[must_use]
    pub fn covers_address(&self, address: &scout_core::AddressBytes) -> bool {
        self.contract_addresses.contains(address)
    }
}

/// Port every Tier 1 protocol decoder implements — ours and third
/// parties' alike, generic over the raw input shape (`Raw`, e.g.
/// `scout_core::RawEvmLog`) and the decoded output type (`Decoded`).
pub trait TxDecoder<Raw, Decoded>: Send + Sync {
    /// The deployment this decoder is scoped to. A registry only
    /// dispatches payloads whose chain/address/position this scope
    /// actually covers.
    fn scope(&self) -> &DeploymentScope;

    /// Attempt to decode `raw`. See `DecodeOutcome` for the
    /// `NotMine`/`Decoded`/`Malformed` contract.
    fn decode(&self, raw: &Raw) -> DecodeOutcome<Decoded>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_scope(from: u64, until: Option<u64>) -> DeploymentScope {
        DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Evm([0xAA; 20])],
            active_from: from,
            active_until: until,
        }
    }

    #[test]
    fn scope_with_no_upper_bound_covers_any_position_at_or_after_start() {
        let scope = test_scope(100, None);
        assert!(!scope.covers_position(99));
        assert!(scope.covers_position(100));
        assert!(scope.covers_position(1_000_000));
    }

    #[test]
    fn scope_with_upper_bound_excludes_positions_after_it() {
        let scope = test_scope(100, Some(200));
        assert!(scope.covers_position(100));
        assert!(scope.covers_position(200));
        assert!(!scope.covers_position(201));
    }

    #[test]
    fn scope_only_covers_its_declared_addresses() {
        let scope = test_scope(0, None);
        assert!(scope.covers_address(&scout_core::AddressBytes::Evm([0xAA; 20])));
        assert!(!scope.covers_address(&scout_core::AddressBytes::Evm([0xBB; 20])));
    }

    #[test]
    fn not_mine_and_malformed_are_distinct_outcomes() {
        // The whole point of this type vs a single Result: a registry
        // iterating decoders must be able to tell "wrong decoder" from
        // "right decoder, broken data" apart.
        let not_mine: DecodeOutcome<u32> = DecodeOutcome::NotMine;
        let malformed: DecodeOutcome<u32> = DecodeOutcome::Malformed("bad".to_string());
        assert!(not_mine.is_not_mine());
        assert!(!not_mine.is_malformed());
        assert!(malformed.is_malformed());
        assert!(!malformed.is_not_mine());
    }
}
