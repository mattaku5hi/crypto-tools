//! Generic Uniswap-v2-shape `Swap` event decoder.
//!
//! Scope note (AGENTS.md invariant #16): this decodes the well-known
//! `Swap(address,uint256,uint256,uint256,uint256,address)` event shape
//! documented across the Uniswap-v2 ABI family. It does **not** claim
//! support for any specific deployment — `docs/p0/deployment-registry.md`
//! has zero confirmed entries as of this writing. This module exists to
//! prove the decode *mechanism* end-to-end (raw log -> typed swap) against
//! a synthetic fixture, satisfying ARCHITECTURE.md §1's "первым рабочим
//! slice делаем один подтвержденный EVM DEX" groundwork before a real
//! deployment is confirmed and wired in.
//!
//! Implements `scout_api::TxDecoder` (ADR-008 S4). The `Ok(None)`/`Err`
//! split from the pre-S4 free-function version is now
//! `DecodeOutcome::NotMine`/`DecodeOutcome::Malformed` — a log with a
//! different event signature is `NotMine` (normal: a registry trying
//! several decoders against one log sees this from every non-matching
//! decoder), never conflated with "matched this decoder's signature but
//! its structure is broken" (`Malformed`, invariant #18: never silently
//! skipped).

use alloy_primitives::{Address, B256, U256};
use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::RawEvmLog;

/// The well-known Uniswap-v2-style `Swap` event signature hash:
/// `keccak256("Swap(address,uint256,uint256,uint256,uint256,address)")`.
/// This is a protocol-shape constant, not a deployment address — pinning
/// it here does not claim any specific contract is supported.
pub const V2_SWAP_EVENT_SIGNATURE: B256 = B256::new([
    0xd7, 0x8a, 0xd9, 0x5f, 0xa4, 0x6c, 0x99, 0x4b, 0x65, 0x51, 0xd0, 0xda, 0x85, 0xfc, 0x27, 0x5f,
    0xe6, 0x13, 0xce, 0x37, 0x65, 0x7f, 0xb8, 0xd5, 0xe4, 0x4f, 0x1f, 0xea, 0x53, 0x9d, 0x93, 0x24,
]);

/// A decoded v2-style swap: raw amounts in/out for each token side, and
/// the address that received the output (the `to` field of the event —
/// a candidate signal for owner attribution, never auto-promoted to
/// confident owner here; that classification lives in scout-normalize
/// per ADR-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSwap {
    pub pool_address: Address,
    pub sender: Address,
    pub amount0_in: U256,
    pub amount1_in: U256,
    pub amount0_out: U256,
    pub amount1_out: U256,
    pub to: Address,
    pub block_number: u64,
    pub transaction_index: u64,
    pub log_index: u64,
}

/// A `TxDecoder` for the v2-style `Swap` event shape, scoped to one
/// `DeploymentScope` (AGENTS.md invariant #16: mandatory at
/// registration, never a blanket claim across chains/addresses).
#[derive(Debug, Clone)]
pub struct V2SwapDecoder {
    scope: DeploymentScope,
}

impl V2SwapDecoder {
    #[must_use]
    pub fn new(scope: DeploymentScope) -> Self {
        Self { scope }
    }
}

impl TxDecoder<RawEvmLog, DecodedSwap> for V2SwapDecoder {
    fn scope(&self) -> &DeploymentScope {
        &self.scope
    }

    fn decode(&self, log: &RawEvmLog) -> DecodeOutcome<DecodedSwap> {
        decode_v2_style_swap(log)
    }
}

/// Decode a raw log as a Uniswap-v2-shape `Swap` event.
///
/// `DecodeOutcome::NotMine` for a log with a different event signature —
/// this is the expected, common case when scanning a block's mixed logs
/// and is never treated as an error. `DecodeOutcome::Malformed` for a log
/// that *does* match the signature but has a broken topic count or data
/// length — real evidence of an unfamiliar/corrupted format that must
/// surface (AGENTS.md invariant #18: "Незнакомый формат... не
/// пропускается молча").
#[must_use]
pub fn decode_v2_style_swap(log: &RawEvmLog) -> DecodeOutcome<DecodedSwap> {
    let signature = log.topics.first().copied();
    if signature != Some(V2_SWAP_EVENT_SIGNATURE) {
        return DecodeOutcome::NotMine;
    }
    if log.topics.len() != 3 {
        return DecodeOutcome::Malformed(format!(
            "log matches v2 Swap signature but has {} topics, expected 3 (signature + sender + to)",
            log.topics.len()
        ));
    }
    if log.data.len() != 128 {
        return DecodeOutcome::Malformed(format!(
            "log matches v2 Swap signature but data has {} bytes, expected 128 (4x uint256)",
            log.data.len()
        ));
    }

    let (Some(sender_topic), Some(to_topic)) = (log.topics.get(1), log.topics.get(2)) else {
        // Unreachable given the topics.len() == 3 check above, but
        // avoid indexing_slicing per workspace lint policy rather than
        // asserting an invariant that's already been checked.
        return DecodeOutcome::Malformed(
            "log matches v2 Swap signature but sender/to topics are missing".to_string(),
        );
    };
    let sender = address_from_topic(sender_topic);
    let to = address_from_topic(to_topic);

    let amount0_in = u256_from_data_slice(&log.data, 0);
    let amount1_in = u256_from_data_slice(&log.data, 32);
    let amount0_out = u256_from_data_slice(&log.data, 64);
    let amount1_out = u256_from_data_slice(&log.data, 96);

    DecodeOutcome::Decoded(DecodedSwap {
        pool_address: log.address,
        sender,
        amount0_in,
        amount1_in,
        amount0_out,
        amount1_out,
        to,
        block_number: log.block_number,
        transaction_index: log.transaction_index,
        log_index: log.log_index,
    })
}

/// An indexed `address` topic is a 32-byte value with the address
/// right-aligned in the low 20 bytes (EVM ABI encoding convention).
fn address_from_topic(topic: &B256) -> Address {
    let bytes = topic.as_slice();
    let mut addr_bytes = [0u8; 20];
    // bytes is always 32 long (B256); the last 20 bytes are the address.
    if let Some(tail) = bytes.get(12..32) {
        addr_bytes.copy_from_slice(tail);
    }
    Address::from(addr_bytes)
}

/// Read one 32-byte big-endian `uint256` word from `data` starting at
/// `offset`. Caller guarantees `data.len() == 128` (checked before this
/// is called), so `offset + 32 <= data.len()` always holds for the four
/// calls in `decode_v2_style_swap`.
fn u256_from_data_slice(data: &[u8], offset: usize) -> U256 {
    let mut word = [0u8; 32];
    if let Some(slice) = data.get(offset..offset + 32) {
        word.copy_from_slice(slice);
    }
    U256::from_be_bytes(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic (never claimed as mainnet) v2-shape Swap log.
    /// Provenance: hand-constructed fixture proving the decode mechanism,
    /// per ADR-006 / docs/p0/deployment-registry.md's empty-by-design
    /// state — this is not a real pool address or transaction.
    fn synthetic_v2_swap_log(
        amount0_in: u64,
        amount1_in: u64,
        amount0_out: u64,
        amount1_out: u64,
    ) -> RawEvmLog {
        let mut data = Vec::with_capacity(128);
        for amount in [amount0_in, amount1_in, amount0_out, amount1_out] {
            let mut word = [0u8; 32];
            word[24..32].copy_from_slice(&amount.to_be_bytes());
            data.extend_from_slice(&word);
        }

        let sender_topic = address_topic([0x11; 20]);
        let to_topic = address_topic([0x22; 20]);

        RawEvmLog {
            address: Address::from([0xAA; 20]),
            topics: vec![V2_SWAP_EVENT_SIGNATURE, sender_topic, to_topic],
            data: data.into(),
            block_number: 12_345,
            transaction_index: 7,
            log_index: 2,
        }
    }

    fn address_topic(addr_bytes: [u8; 20]) -> B256 {
        let mut topic = [0u8; 32];
        topic[12..32].copy_from_slice(&addr_bytes);
        B256::new(topic)
    }

    #[test]
    fn decodes_a_well_formed_v2_swap_log() {
        let log = synthetic_v2_swap_log(1_000, 0, 0, 950);
        let outcome = decode_v2_style_swap(&log);
        let decoded = outcome.decoded().unwrap();
        assert_eq!(decoded.amount0_in, U256::from(1_000_u64));
        assert_eq!(decoded.amount1_out, U256::from(950_u64));
        assert_eq!(decoded.pool_address, Address::from([0xAA; 20]));
        assert_eq!(decoded.sender, Address::from([0x11; 20]));
        assert_eq!(decoded.to, Address::from([0x22; 20]));
        assert_eq!(decoded.block_number, 12_345);
    }

    #[test]
    fn wrong_signature_is_not_mine_not_an_error() {
        // The actual bug this migration fixes: a log belonging to a
        // different event must be NotMine, not conflated with
        // Malformed. A registry scanning a block's mixed logs against
        // several decoders relies on this distinction to avoid
        // reporting every non-matching log as a decode failure.
        let mut log = synthetic_v2_swap_log(1, 0, 0, 1);
        log.topics[0] = B256::ZERO;
        let outcome = decode_v2_style_swap(&log);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn matching_signature_with_wrong_topic_count_is_malformed_not_not_mine() {
        // AGENTS.md invariant #18: a log that DOES match this decoder's
        // signature but has a broken structure must surface as
        // Malformed, never silently treated the same as "not my event."
        let mut log = synthetic_v2_swap_log(1, 0, 0, 1);
        log.topics.pop();
        let outcome = decode_v2_style_swap(&log);
        assert!(outcome.is_malformed());
        assert!(!outcome.is_not_mine());
    }

    #[test]
    fn matching_signature_with_wrong_data_length_is_malformed() {
        let mut log = synthetic_v2_swap_log(1, 0, 0, 1);
        log.data = alloy_primitives::Bytes::from(vec![0u8; 64]);
        let outcome = decode_v2_style_swap(&log);
        assert!(outcome.is_malformed());
    }

    #[test]
    fn preserves_canonical_ordering_fields_for_downstream_ledger_ordering() {
        // ADR-002/ADR-004: block_number + transaction_index (+ log_index
        // here as the finer-grained action path) must survive decoding
        // unmodified, since the ledger's FIFO ordering depends on them.
        let log = synthetic_v2_swap_log(1, 0, 0, 1);
        let decoded = decode_v2_style_swap(&log).decoded().unwrap();
        assert_eq!(decoded.block_number, 12_345);
        assert_eq!(decoded.transaction_index, 7);
        assert_eq!(decoded.log_index, 2);
    }

    #[test]
    fn max_u256_amount_does_not_overflow_or_truncate() {
        // ACCEPTANCE C13 spirit extended to decoding: extreme values must
        // decode exactly, not wrap or truncate.
        let mut data = vec![0xFFu8; 32]; // amount0_in = U256::MAX
        data.extend_from_slice(&[0u8; 32]); // amount1_in = 0
        data.extend_from_slice(&[0u8; 32]); // amount0_out = 0
        data.extend_from_slice(&[0u8; 32]); // amount1_out = 0
        let log = RawEvmLog {
            address: Address::from([0xAA; 20]),
            topics: vec![
                V2_SWAP_EVENT_SIGNATURE,
                address_topic([0x11; 20]),
                address_topic([0x22; 20]),
            ],
            data: data.into(),
            block_number: 1,
            transaction_index: 0,
            log_index: 0,
        };
        let decoded = decode_v2_style_swap(&log).decoded().unwrap();
        assert_eq!(decoded.amount0_in, U256::MAX);
    }

    #[test]
    fn v2_swap_decoder_implements_tx_decoder_trait() {
        // Proves the trait wiring, not just the free function.
        let scope = DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Evm([0xAA; 20])],
            active_from: 0,
            active_until: None,
        };
        let decoder = V2SwapDecoder::new(scope);
        let log = synthetic_v2_swap_log(1, 0, 0, 1);
        let outcome = decoder.decode(&log);
        assert!(matches!(outcome, DecodeOutcome::Decoded(_)));
    }
}
