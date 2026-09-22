//! Minimal raw EVM log/transaction shapes. Deliberately narrow — this is
//! not a full receipt/block model, just what a decoder needs to classify
//! one event (ARCHITECTURE.md \$4's `TxDecoder` contract input).

use alloy_primitives::{Address, B256, Bytes, U256};

/// One decoded-from-JSON-RPC log entry, prior to any protocol-specific
/// interpretation. `topics[0]` is the event signature hash when present;
/// callers must not assume a fixed topic count without checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvmLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
    pub block_number: u64,
    pub transaction_index: u64,
    pub log_index: u64,
}

/// Minimal transaction context a decoder needs: who sent it, and its
/// canonical position (block_number, tx_index) per ADR-002's ordering
/// contract — never wall-clock/fetch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvmTransaction {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub block_number: u64,
    pub transaction_index: u64,
    pub value: U256,
}
