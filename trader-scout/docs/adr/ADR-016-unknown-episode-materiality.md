# ADR-016: Materiality of unknown episodes in quality gates and ranking (worst-case bounds)

Status: Accepted (owner decision, 2026-10-03)
Date: 2026-10-03
Amends: ARCHITECTURE §10 "material unknown basis" (today: any `ClosedUnknown` excludes), ADR-011 §5,
TICKETS P4.10.

## Context

After ADR-013/015 most closed episodes of active wallets are `ClosedKnown`
(`2026-10-02-wallet-rank-live.md`: 48/2, 52/1, 28/4, 10/1, …), yet every wallet with a single
`ClosedUnknown` is excluded, so rankings are empty. Unknown episodes cannot simply be dropped: they
may be the losers (invariant #9, selection bias).

## Decision

1. **Share gate.** Under `quality` and `insider`, a wallet passes the unknown gate iff
   `closed_unknown ≤ max_unknown_episode_share × (closed_known + closed_unknown)`, compared exactly
   as integers; default **10 %** (owner decision), CLI `--max-unknown-episode-share <percent>`
   (0 = old strict rule). Left-censored episodes are not in numerator or denominator (ADR-011).
   Open positions with in-window unknown basis are reported (`has_unknown_basis_inventory`, raw
   amounts) and no longer exclude by themselves (open positions are unvalued anyway, P5.2);
   `--require-no-open` remains the strict variant. `none` profile: no gate, labels only.
2. **Worst-case metrics (lower bounds).** Every `ClosedUnknown` episode is treated as a loss:
   - `win_rate_lower_bound = wins / (closed_known + closed_unknown)`; the `min-…`/sample gates and
     any win-rate presentation in rankings use this bound.
   - Per unknown episode, a PnL lower bound exists iff its consumed acquisition basis is fully known
     (proceeds unknown but ≥ 0 ⇒ PnL ≥ −basis): bound = −known consumed basis. If any consumed lot
     has unknown basis, that episode's lower bound is **unbounded**.
   - `realized_pnl_lower_bound = Σ known episode PnL + Σ bounds of unknown episodes` (per quote unit);
     `profit_factor_lower_bound = gross_profit / (gross_loss + Σ |bounds|)`; both `unbounded` if any
     unknown episode is unbounded.
3. **Ranking tiers.** Rank keys use the lower bounds. Tier 1: wallets whose lower bounds are bounded
   (all-known wallets are tier 1 with bound = exact value). Tier 2: wallets with an unbounded unknown
   episode, ordered after all tier-1 wallets by their known-subset value, always labelled
   `pnl_status = known_subset_unbounded` and `rank_tier = 2`. `--exclude-unbounded` drops tier 2
   (reason `pnl_unbounded`). Ties and remaining chain per ARCHITECTURE §10.
4. **Reporting.** JSONL/table show known-subset PnL, the lower bound (or `unbounded`), unknown and
   left-censored counts, `win_rate` (known) and `win_rate_lower_bound`, the effective share and the
   policy values in `run_meta`. Unknown never serializes as zero.

## Consequences

- Rankings become non-empty for wallets with a small unknown share, ordered conservatively; a
  wallet cannot rise in rank because of unknown episodes.
- The 10 % default is a research starting policy, not a statistical guarantee; it is printed with
  every run.
