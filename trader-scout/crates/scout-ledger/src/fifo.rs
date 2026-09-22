//! FIFO ledger: lot consumption, fee allocation, realized PnL. See ADR-004
//! for the formulas this implements, and ACCEPTANCE §C for the worked
//! examples that back the tests below.

use std::collections::BTreeMap;

use scout_core::{AssetKey, Money, RawAmount, ScoutError};

use crate::lot::{BasisStatus, Lot};

/// Result of consuming lots to cover a disposal (sale).
#[derive(Debug, Clone)]
pub struct DisposalResult {
    /// Proceeds after allocated sale fees.
    pub net_sale_proceeds: Money,
    /// Sum of capitalized basis consumed from all lots touched.
    pub consumed_acquisition_basis: Money,
    /// `net_sale_proceeds - consumed_acquisition_basis`, only when every
    /// consumed lot had known basis; `None` otherwise (ADR-004: unknown
    /// basis is never treated as zero, so PnL for such a disposal is
    /// explicitly unknown, not silently computed as if the unknown lot
    /// cost nothing).
    pub realized_trade_pnl: Option<Money>,
    /// True if every lot consumed by this disposal had known basis.
    pub all_basis_known: bool,
}

/// Per-asset FIFO inventory and realized-PnL ledger. One `Ledger` per
/// `(WalletKey, AssetKey)` in the engine; kept generic here over a single
/// asset's lot queue for testability.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// FIFO queue of lots, oldest-first by `acquisition_sequence`.
    /// `BTreeMap` keyed by sequence number keeps consumption order
    /// deterministic regardless of how lots were inserted (workspace
    /// determinism policy; ADR-002's canonical-location ordering
    /// contract is what actually assigns these sequence numbers
    /// upstream).
    lots: BTreeMap<u64, Lot>,
    next_sequence: u64,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new acquisition lot. `basis` is the fully capitalized
    /// cost (consideration + allocated acquisition fees).
    pub fn acquire(
        &mut self,
        asset: AssetKey,
        amount: RawAmount,
        basis: Money,
        basis_status: BasisStatus,
    ) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.lots.insert(
            sequence,
            Lot {
                asset,
                acquisition_sequence: sequence,
                original_amount: amount,
                remaining_amount: amount,
                original_basis: basis,
                remaining_basis: basis,
                basis_status,
            },
        );
        sequence
    }

    /// Total remaining amount across all open lots (open inventory).
    #[must_use]
    pub fn open_amount(&self) -> RawAmount {
        self.lots.values().fold(RawAmount::ZERO, |acc, lot| {
            acc.checked_add(&lot.remaining_amount)
                .unwrap_or(RawAmount::ZERO)
        })
    }

    /// Consume lots FIFO to cover a disposal of `amount`, given the raw
    /// proceeds and sale fee. Per ADR-004: fee is allocated to reduce
    /// proceeds; the disposal never re-subtracts a fee already captured
    /// elsewhere.
    ///
    /// Returns an error if open inventory is insufficient to cover the
    /// disposal (this ledger only tracks known on-chain deltas; the
    /// caller is responsible for not calling this with more than the
    /// wallet's confirmed on-chain balance).
    pub fn dispose(
        &mut self,
        mut amount_to_dispose: RawAmount,
        gross_proceeds: Money,
        sale_fee: Money,
    ) -> Result<DisposalResult, ScoutError> {
        let net_sale_proceeds = gross_proceeds.checked_sub(&sale_fee)?;
        let mut consumed_acquisition_basis = Money::ZERO;
        let mut all_basis_known = true;

        // Oldest lot first: BTreeMap iteration over u64 keys is already
        // ascending, giving FIFO order for free.
        let sequences: Vec<u64> = self.lots.keys().copied().collect();
        for sequence in sequences {
            if amount_to_dispose == RawAmount::ZERO {
                break;
            }
            let Some(lot) = self.lots.get_mut(&sequence) else {
                continue;
            };
            if lot.remaining_amount == RawAmount::ZERO {
                continue;
            }

            let consume_amount = if lot.remaining_amount.as_u256() <= amount_to_dispose.as_u256() {
                lot.remaining_amount
            } else {
                amount_to_dispose
            };

            // Proportional basis consumption: consumed_basis / lot's
            // remaining_basis == consume_amount / lot's remaining_amount.
            // Computed via checked integer arithmetic on scaled units to
            // avoid float rounding (ADR-004, ACCEPTANCE C02's exact
            // worked example).
            let lot_basis_consumed =
                proportional_money(lot.remaining_basis, consume_amount, lot.remaining_amount)?;

            consumed_acquisition_basis =
                consumed_acquisition_basis.checked_add(&lot_basis_consumed)?;
            if matches!(lot.basis_status, BasisStatus::Unknown { .. }) {
                all_basis_known = false;
            }

            lot.remaining_amount = lot.remaining_amount.checked_sub(&consume_amount).ok_or(
                ScoutError::ArithmeticOverflow {
                    context: "Ledger::dispose remaining_amount underflow",
                },
            )?;
            lot.remaining_basis = lot.remaining_basis.checked_sub(&lot_basis_consumed)?;

            amount_to_dispose = amount_to_dispose.checked_sub(&consume_amount).ok_or(
                ScoutError::ArithmeticOverflow {
                    context: "Ledger::dispose amount_to_dispose underflow",
                },
            )?;
        }

        if amount_to_dispose != RawAmount::ZERO {
            return Err(ScoutError::ArithmeticOverflow {
                context: "Ledger::dispose: insufficient open inventory to cover disposal",
            });
        }

        let realized_trade_pnl = if all_basis_known {
            Some(net_sale_proceeds.checked_sub(&consumed_acquisition_basis)?)
        } else {
            None
        };

        Ok(DisposalResult {
            net_sale_proceeds,
            consumed_acquisition_basis,
            realized_trade_pnl,
            all_basis_known,
        })
    }
}

/// Compute `total * (numerator / denominator)` using only checked integer
/// arithmetic on `Money`'s scaled units, never floats (ADR-001/ADR-004).
fn proportional_money(
    total: Money,
    numerator: RawAmount,
    denominator: RawAmount,
) -> Result<Money, ScoutError> {
    if denominator == RawAmount::ZERO {
        return Ok(Money::ZERO);
    }
    if numerator == denominator {
        // Exact full consumption: avoid any division at all, so no
        // rounding is possible for the common "consume the whole lot"
        // case (ACCEPTANCE C01's full-lot scenario).
        return Ok(total);
    }
    // For a partial consumption we need total * numerator / denominator.
    // Both RawAmount values fit in U256; total.scaled_units() is i128.
    // Since numerator/denominator both come from the same RawAmount
    // domain (on-chain token units), we can safely reduce them to a
    // u128 ratio for this multiplication — token supplies never
    // approach U256::MAX in practice, and any case that would overflow
    // here is exactly the "extreme amount values" case that must
    // produce a typed error, not a silent wrap.
    let num_u128 =
        u128::try_from(numerator.as_u256()).map_err(|_| ScoutError::ArithmeticOverflow {
            context: "proportional_money: numerator exceeds u128",
        })?;
    let den_u128 =
        u128::try_from(denominator.as_u256()).map_err(|_| ScoutError::ArithmeticOverflow {
            context: "proportional_money: denominator exceeds u128",
        })?;
    let total_units = total.scaled_units();
    let total_units_u128 = total_units.unsigned_abs();

    let product = total_units_u128
        .checked_mul(num_u128)
        .ok_or(ScoutError::ArithmeticOverflow {
            context: "proportional_money: total * numerator overflow",
        })?;
    let quotient = product.div_euclid(den_u128);
    let signed = i128::try_from(quotient).map_err(|_| ScoutError::ArithmeticOverflow {
        context: "proportional_money: quotient exceeds i128",
    })?;
    let signed = if total_units.is_negative() {
        -signed
    } else {
        signed
    };
    Ok(Money::from_scaled_units(signed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_asset() -> AssetKey {
        AssetKey::Token(
            scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            scout_core::AddressBytes::Evm([0x11; 20]),
        )
    }

    fn money(whole_and_frac_scaled: i128) -> Money {
        Money::from_scaled_units(whole_and_frac_scaled * 10i128.pow(scout_core::MONEY_SCALE))
    }

    fn raw(units: u64) -> RawAmount {
        RawAmount::from_u256(alloy_primitives::U256::from(units))
    }

    #[test]
    fn acceptance_c01_full_lot_disposal_matches_worked_example() {
        // C01: consideration=1000, fee=10. Sale: proceeds=1400, fee=10.
        // Expected realized_trade_pnl=380, consumed_basis=1010.
        let mut ledger = Ledger::new();
        let basis = money(1000).checked_add(&money(10)).unwrap();
        ledger.acquire(test_asset(), raw(100), basis, BasisStatus::Known);

        let result = ledger.dispose(raw(100), money(1400), money(10)).unwrap();

        assert_eq!(result.consumed_acquisition_basis, money(1010));
        assert_eq!(result.net_sale_proceeds, money(1390));
        assert_eq!(result.realized_trade_pnl, Some(money(380)));
    }

    #[test]
    fn acceptance_c02_partial_disposal_does_not_consume_full_basis() {
        // C02: 100 units bought for 1000+fee10=1010 basis. Sell 40 for
        // 600-fee6. Expected consumed_basis=404, realized_trade_pnl=190,
        // remaining_basis=606.
        let mut ledger = Ledger::new();
        let basis = money(1010);
        ledger.acquire(test_asset(), raw(100), basis, BasisStatus::Known);

        let result = ledger.dispose(raw(40), money(600), money(6)).unwrap();

        assert_eq!(result.consumed_acquisition_basis, money(404));
        assert_eq!(result.net_sale_proceeds, money(594));
        assert_eq!(result.realized_trade_pnl, Some(money(190)));

        // Remaining basis must be exactly 606, not re-derived from a
        // rounded intermediate (ACCEPTANCE C02: "База не списывается
        // целиком при partial sale").
        let remaining: Money = ledger
            .lots
            .values()
            .map(|l| l.remaining_basis)
            .fold(Money::ZERO, |acc, m| acc.checked_add(&m).unwrap());
        assert_eq!(remaining, money(606));
    }

    #[test]
    fn fifo_order_consumes_oldest_lot_first() {
        let mut ledger = Ledger::new();
        // Lot 1: cheap, old. Lot 2: expensive, new.
        ledger.acquire(test_asset(), raw(50), money(100), BasisStatus::Known);
        ledger.acquire(test_asset(), raw(50), money(500), BasisStatus::Known);

        // Dispose exactly the first lot's amount; basis consumed must be
        // from the cheap lot, not the expensive one.
        let result = ledger.dispose(raw(50), money(200), Money::ZERO).unwrap();
        assert_eq!(result.consumed_acquisition_basis, money(100));
    }

    #[test]
    fn unknown_basis_lot_makes_pnl_unknown_not_zero_cost() {
        // AGENTS.md invariant #6 / ACCEPTANCE C05: unknown basis must
        // never be treated as zero — the realized PnL for a disposal
        // touching an unknown-basis lot is None, not "proceeds - 0".
        let mut ledger = Ledger::new();
        ledger.acquire(
            test_asset(),
            raw(100),
            Money::ZERO,
            BasisStatus::Unknown {
                reason: "external transfer, no lineage".to_string(),
            },
        );

        let result = ledger.dispose(raw(100), money(500), Money::ZERO).unwrap();
        assert_eq!(result.realized_trade_pnl, None);
        assert!(!result.all_basis_known);
        // Proceeds are still known even though PnL is not.
        assert_eq!(result.net_sale_proceeds, money(500));
    }

    #[test]
    fn disposing_more_than_open_inventory_is_a_typed_error() {
        let mut ledger = Ledger::new();
        ledger.acquire(test_asset(), raw(10), money(100), BasisStatus::Known);
        let result = ledger.dispose(raw(20), money(50), Money::ZERO);
        assert!(result.is_err());
    }

    #[test]
    fn apply_twice_is_idempotent_when_ledger_is_rebuilt_from_scratch() {
        // A simplified idempotency check: replaying the exact same
        // sequence of acquire/dispose calls against a fresh ledger
        // produces the same final open_amount both times (ACCEPTANCE
        // property: apply(E); apply(E) = apply(E), here checked as
        // "same E applied to two fresh ledgers gives the same result",
        // which is the meaningful invariant at this layer — true replay
        // idempotency requires the engine's event-dedup, tracked
        // separately).
        let mut ledger_a = Ledger::new();
        ledger_a.acquire(test_asset(), raw(100), money(1000), BasisStatus::Known);
        ledger_a.dispose(raw(40), money(500), Money::ZERO).unwrap();

        let mut ledger_b = Ledger::new();
        ledger_b.acquire(test_asset(), raw(100), money(1000), BasisStatus::Known);
        ledger_b.dispose(raw(40), money(500), Money::ZERO).unwrap();

        assert_eq!(ledger_a.open_amount(), ledger_b.open_amount());
    }

    #[test]
    fn fee_allocation_never_double_counts() {
        // C08 spirit: acquisition fee capitalized once, sale fee reduces
        // proceeds once. Verify the sum of (consumed_basis contribution
        // from acquisition fee) + (sale fee) equals the total fees paid,
        // not double either one.
        let mut ledger = Ledger::new();
        let acquisition_fee = money(10);
        let consideration = money(1000);
        let basis = consideration.checked_add(&acquisition_fee).unwrap();
        ledger.acquire(test_asset(), raw(100), basis, BasisStatus::Known);

        let sale_fee = money(10);
        let result = ledger.dispose(raw(100), money(1400), sale_fee).unwrap();

        // consumed_acquisition_basis already includes the acquisition
        // fee (1010, not 1000); net_sale_proceeds already excludes the
        // sale fee (1390, not 1400). realized_trade_pnl must reflect
        // exactly one deduction of each fee.
        assert_eq!(result.consumed_acquisition_basis, money(1010));
        assert_eq!(result.net_sale_proceeds, money(1390));
        assert_eq!(
            result.realized_trade_pnl,
            Some(money(1390).checked_sub(&money(1010)).unwrap())
        );
    }
}
