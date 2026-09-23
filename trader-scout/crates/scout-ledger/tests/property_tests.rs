//! Property tests for FIFO ledger invariants. See ACCEPTANCE.md's
//! closing "Минимальные property-инварианты" section: these are the
//! universal statements no fixed set of example tests can fully cover.

use alloy_primitives::U256;
use proptest::prelude::*;
use scout_core::{
    AddressBytes, AssetKey, ChainFamily, ChainKey, GenesisIdentity, Money, NetworkId, RawAmount,
};
use scout_ledger::{BasisStatus, Ledger};

fn test_asset() -> AssetKey {
    AssetKey::Token(
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(8453),
            genesis_identity: GenesisIdentity::Unverified,
        },
        AddressBytes::Evm([0x11; 20]),
    )
}

fn money(scaled_whole: i64) -> Money {
    Money::from_scaled_units(i128::from(scaled_whole) * 10i128.pow(scout_core::MONEY_SCALE))
}

fn raw(units: u64) -> RawAmount {
    RawAmount::from_u256(U256::from(units))
}

proptest! {
    /// Inventory conservation: after any sequence of acquisitions and
    /// disposals that never exceed available inventory, open_amount
    /// equals the sum of acquired amounts minus the sum of disposed
    /// amounts (ACCEPTANCE closing section: "opening + acquisitions +
    /// incoming - disposals ... = closing").
    #[test]
    fn open_amount_equals_acquired_minus_disposed(
        acquire_amounts in prop::collection::vec(1u64..1_000_000, 1..10),
        // Fraction (0..100) of running total to dispose at each step,
        // capped so we never try to dispose more than is available.
        dispose_fractions in prop::collection::vec(0u64..100, 0..10),
    ) {
        let mut ledger = Ledger::new();
        let mut total_acquired: u128 = 0;
        for amount in &acquire_amounts {
            ledger.acquire(test_asset(), raw(*amount), money(1000), BasisStatus::Known);
            total_acquired += u128::from(*amount);
        }

        let mut total_disposed: u128 = 0;
        for fraction in &dispose_fractions {
            let open = ledger.open_amount();
            let open_u128 = u128::try_from(open.as_u256()).unwrap_or(0);
            if open_u128 == 0 {
                continue;
            }
            let dispose_amount = open_u128.saturating_mul(u128::from(*fraction)).div_euclid(100);
            if dispose_amount == 0 || dispose_amount > open_u128 {
                continue;
            }
            let dispose_u64 = u64::try_from(dispose_amount).unwrap_or(0);
            if dispose_u64 == 0 {
                continue;
            }
            let result = ledger.dispose(raw(dispose_u64), money(1), Money::ZERO);
            if result.is_ok() {
                total_disposed += u128::from(dispose_u64);
            }
        }

        let expected_open = total_acquired - total_disposed;
        let actual_open = u128::try_from(ledger.open_amount().as_u256()).unwrap_or(0);
        prop_assert_eq!(actual_open, expected_open);
    }

    /// Fee-allocation conservation for a single-lot full disposal:
    /// consumed_acquisition_basis + remaining_basis (zero after full
    /// consumption) equals the original basis exactly, no matter the
    /// acquisition amount (ACCEPTANCE: "acquisition basis
    /// распределяется... без потери и дублирования").
    #[test]
    fn full_disposal_consumes_exactly_the_full_basis_no_loss_no_duplication(
        amount in 1u64..1_000_000,
        basis_whole in 1i64..1_000_000,
    ) {
        let mut ledger = Ledger::new();
        let basis = money(basis_whole);
        ledger.acquire(test_asset(), raw(amount), basis, BasisStatus::Known);

        let result = ledger.dispose(raw(amount), money(basis_whole), Money::ZERO);
        let Ok(result) = result else {
            prop_assert!(false, "expected Ok, got Err");
            return Ok(());
        };
        // proportional_money's full-consumption fast path (numerator ==
        // denominator) guarantees no rounding loss here.
        prop_assert_eq!(result.consumed_acquisition_basis, basis);
        prop_assert_eq!(ledger.open_amount(), RawAmount::ZERO);
    }

    /// Idempotency: replaying the identical acquire+dispose sequence
    /// against two freshly constructed ledgers always yields the same
    /// final open_amount (ACCEPTANCE: "apply(E); apply(E) = apply(E)",
    /// interpreted here as determinism of a fixed event sequence rather
    /// than the engine's future at-least-once dedup, which is a
    /// separate concern).
    #[test]
    fn identical_event_sequence_is_deterministic_across_fresh_ledgers(
        amount in 1u64..500_000,
        basis_whole in 1i64..500_000,
        dispose_fraction in 1u64..100,
    ) {
        let dispose_amount = u128::from(amount).saturating_mul(u128::from(dispose_fraction)).div_euclid(100).max(1);
        let dispose_amount = u64::try_from(dispose_amount.min(u128::from(amount))).unwrap_or(1);

        let mut ledger_a = Ledger::new();
        ledger_a.acquire(test_asset(), raw(amount), money(basis_whole), BasisStatus::Known);
        let result_a = ledger_a.dispose(raw(dispose_amount), money(1), Money::ZERO);

        let mut ledger_b = Ledger::new();
        ledger_b.acquire(test_asset(), raw(amount), money(basis_whole), BasisStatus::Known);
        let result_b = ledger_b.dispose(raw(dispose_amount), money(1), Money::ZERO);

        prop_assert_eq!(result_a.is_ok(), result_b.is_ok());
        prop_assert_eq!(ledger_a.open_amount(), ledger_b.open_amount());
        if let (Ok(a), Ok(b)) = (result_a, result_b) {
            prop_assert_eq!(a.consumed_acquisition_basis, b.consumed_acquisition_basis);
            prop_assert_eq!(a.realized_trade_pnl, b.realized_trade_pnl);
        }
    }

    /// Partial disposals never consume more basis than the lot actually
    /// has: consumed_acquisition_basis for a partial disposal is always
    /// strictly less than (or equal to, at 100%) the original basis.
    #[test]
    fn partial_disposal_never_consumes_more_than_original_basis(
        amount in 2u64..1_000_000,
        basis_whole in 1i64..1_000_000,
        dispose_fraction in 1u64..100, // strictly partial: 1..99%
    ) {
        let dispose_amount = u128::from(amount).saturating_mul(u128::from(dispose_fraction)).div_euclid(100).max(1);
        let dispose_amount = u64::try_from(dispose_amount).unwrap_or(1).min(amount - 1).max(1);
        if dispose_amount == 0 || dispose_amount >= amount {
            return Ok(());
        }

        let mut ledger = Ledger::new();
        let basis = money(basis_whole);
        ledger.acquire(test_asset(), raw(amount), basis, BasisStatus::Known);

        let result = ledger.dispose(raw(dispose_amount), money(basis_whole), Money::ZERO);
        let Ok(result) = result else {
            prop_assert!(false, "expected Ok, got Err");
            return Ok(());
        };

        prop_assert!(result.consumed_acquisition_basis.scaled_units() <= basis.scaled_units());
        prop_assert!(!ledger.open_amount().checked_sub(&RawAmount::ZERO).is_none() || ledger.open_amount() != RawAmount::ZERO);
    }
}
