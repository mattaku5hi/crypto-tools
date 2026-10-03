//! ERC-20 `Transfer` and WETH9 `Deposit`/`Withdrawal` log decoding
//! (ADR-020 §2). Protocol-agnostic: no venue knowledge here.
//!
//! The topic0 constants are keccak256 of the canonical signatures; the unit
//! tests recompute them so a typo cannot survive.

use alloy_primitives::{Address, B256, U256, b256};
use scout_api::DecodeOutcome;
use scout_core::RawEvmLog;

/// `Transfer(address,address,uint256)`.
pub const TRANSFER_TOPIC0: B256 =
    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");
/// WETH9 `Deposit(address,uint256)`.
pub const WETH_DEPOSIT_TOPIC0: B256 =
    b256!("e1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c");
/// WETH9 `Withdrawal(address,uint256)`.
pub const WETH_WITHDRAWAL_TOPIC0: B256 =
    b256!("7fcf532c15f0a6db0bd6d0e038bea71d30d808c7d98cb3bf7268a95bf5081b65");

/// A decoded ERC-20 `Transfer`. `token` is the emitting contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Erc20Transfer {
    pub token: Address,
    pub from: Address,
    pub to: Address,
    pub amount: U256,
    pub log_index: u64,
}

/// Which side of the wrapped-native contract moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrappedNativeKind {
    /// `Deposit(dst, wad)`: `account` was credited `amount` WETH.
    Deposit,
    /// `Withdrawal(src, wad)`: `account` was debited `amount` WETH.
    Withdrawal,
}

/// A decoded WETH9 `Deposit`/`Withdrawal`. WETH9 emits no `Transfer` for
/// these, so they are the only log evidence of wrap/unwrap balance changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrappedNativeEvent {
    /// Emitting contract (caller must compare with the chain's WETH/WBNB).
    pub contract: Address,
    pub kind: WrappedNativeKind,
    pub account: Address,
    pub amount: U256,
    pub log_index: u64,
}

/// Address stored in the low 20 bytes of a 32-byte topic.
#[must_use]
pub fn address_from_topic(topic: &B256) -> Address {
    Address::from_slice(topic.as_slice().get(12..32).unwrap_or(&[0u8; 20]))
}

fn single_word(data: &[u8]) -> Option<U256> {
    if data.len() != 32 {
        return None;
    }
    Some(U256::from_be_slice(data))
}

/// Decode an ERC-20 `Transfer`. An ERC-721-shaped log (same topic0, 4
/// topics, empty data) is `Malformed`, never silently treated as ERC-20
/// (invariant #18).
#[must_use]
pub fn decode_erc20_transfer(log: &RawEvmLog) -> DecodeOutcome<Erc20Transfer> {
    if log.topics.first() != Some(&TRANSFER_TOPIC0) {
        return DecodeOutcome::NotMine;
    }
    let (Some(from), Some(to), 3) = (log.topics.get(1), log.topics.get(2), log.topics.len()) else {
        return DecodeOutcome::Malformed(format!(
            "Transfer-shaped log with {} topics (ERC-721 or non-standard), expected 3",
            log.topics.len()
        ));
    };
    let Some(amount) = single_word(&log.data) else {
        return DecodeOutcome::Malformed(format!(
            "ERC-20 Transfer data has {} bytes, expected 32",
            log.data.len()
        ));
    };
    DecodeOutcome::Decoded(Erc20Transfer {
        token: log.address,
        from: address_from_topic(from),
        to: address_from_topic(to),
        amount,
        log_index: log.log_index,
    })
}

/// Decode a WETH9 `Deposit`/`Withdrawal`.
#[must_use]
pub fn decode_wrapped_native_event(log: &RawEvmLog) -> DecodeOutcome<WrappedNativeEvent> {
    let kind = match log.topics.first() {
        Some(t) if *t == WETH_DEPOSIT_TOPIC0 => WrappedNativeKind::Deposit,
        Some(t) if *t == WETH_WITHDRAWAL_TOPIC0 => WrappedNativeKind::Withdrawal,
        _ => return DecodeOutcome::NotMine,
    };
    let (Some(account), 2) = (log.topics.get(1), log.topics.len()) else {
        return DecodeOutcome::Malformed(format!(
            "WETH Deposit/Withdrawal with {} topics, expected 2",
            log.topics.len()
        ));
    };
    let Some(amount) = single_word(&log.data) else {
        return DecodeOutcome::Malformed(format!(
            "WETH Deposit/Withdrawal data has {} bytes, expected 32",
            log.data.len()
        ));
    };
    DecodeOutcome::Decoded(WrappedNativeEvent {
        contract: log.address,
        kind,
        account: address_from_topic(account),
        amount,
        log_index: log.log_index,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, address, keccak256};

    use super::*;

    fn log(address: Address, topics: Vec<B256>, data: Vec<u8>) -> RawEvmLog {
        RawEvmLog {
            address,
            topics,
            data: Bytes::from(data),
            block_number: 1,
            transaction_index: 0,
            log_index: 7,
        }
    }

    fn topic(addr: Address) -> B256 {
        addr.into_word()
    }

    #[test]
    fn topic0_constants_match_keccak_of_signatures() {
        assert_eq!(
            TRANSFER_TOPIC0,
            keccak256("Transfer(address,address,uint256)")
        );
        assert_eq!(WETH_DEPOSIT_TOPIC0, keccak256("Deposit(address,uint256)"));
        assert_eq!(
            WETH_WITHDRAWAL_TOPIC0,
            keccak256("Withdrawal(address,uint256)")
        );
    }

    #[test]
    fn decodes_erc20_transfer() {
        let token = address!("00000000000000000000000000000000000000aa");
        let a = address!("00000000000000000000000000000000000000a1");
        let b = address!("00000000000000000000000000000000000000b2");
        let l = log(
            token,
            vec![TRANSFER_TOPIC0, topic(a), topic(b)],
            U256::from(1234u64).to_be_bytes::<32>().to_vec(),
        );
        let t = decode_erc20_transfer(&l).decoded().unwrap();
        assert_eq!(
            (t.token, t.from, t.to, t.amount, t.log_index),
            (token, a, b, U256::from(1234u64), 7)
        );
    }

    #[test]
    fn erc721_shaped_transfer_is_malformed_not_erc20() {
        let a = Address::repeat_byte(1);
        let l = log(
            a,
            vec![TRANSFER_TOPIC0, topic(a), topic(a), B256::ZERO],
            vec![],
        );
        assert!(decode_erc20_transfer(&l).is_malformed());
    }

    #[test]
    fn other_topic_is_not_mine_and_short_data_is_malformed() {
        let a = Address::repeat_byte(1);
        assert!(decode_erc20_transfer(&log(a, vec![B256::ZERO], vec![])).is_not_mine());
        let l = log(a, vec![TRANSFER_TOPIC0, topic(a), topic(a)], vec![0; 31]);
        assert!(decode_erc20_transfer(&l).is_malformed());
    }

    #[test]
    fn decodes_weth_deposit_and_withdrawal() {
        let weth = Address::repeat_byte(9);
        let user = Address::repeat_byte(3);
        let data = U256::from(5u64).to_be_bytes::<32>().to_vec();
        let d = decode_wrapped_native_event(&log(
            weth,
            vec![WETH_DEPOSIT_TOPIC0, topic(user)],
            data.clone(),
        ))
        .decoded()
        .unwrap();
        assert_eq!(d.kind, WrappedNativeKind::Deposit);
        assert_eq!(
            (d.account, d.amount, d.contract),
            (user, U256::from(5u64), weth)
        );
        let w = decode_wrapped_native_event(&log(
            weth,
            vec![WETH_WITHDRAWAL_TOPIC0, topic(user)],
            data,
        ))
        .decoded()
        .unwrap();
        assert_eq!(w.kind, WrappedNativeKind::Withdrawal);
        assert!(
            decode_wrapped_native_event(&log(weth, vec![WETH_DEPOSIT_TOPIC0], vec![0; 32]))
                .is_malformed()
        );
    }
}
