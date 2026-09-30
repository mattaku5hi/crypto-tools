# Cohort definitions — wallet discovery categories

Formalizes the categories discussed 2026-09-27. Three categories, not
four — "insider" (large entry right before a pump on an already
high-cap token) is merged into the smart-money scoring criteria as one
input signal, not a standalone pipeline. This removes a hard
dependency on the not-yet-built Solana AMM decoder (P4.1) and pump-leg
detection (P4.2) that a standalone insider pipeline would have needed.

Per AGENTS.md's honest-analytics-boundaries section: discovery and
validation datasets/periods must be kept separate. Every category
below is selected on one window and must be re-checked on a disjoint
window before being trusted — a wallet that only looks good on the
window it was found on has not been validated, it has been curve-fit.

## Category 1: top devs by migration rate

**Definition:** wallets that deployed tokens where a high fraction of
their total launches reached graduation/migration.

**The denominator problem:** this requires `migrated_count /
total_launched_count` per creator wallet, not just a list of
successful launches. A creator with 1 migrated token out of 1 launch
looks identical to one with 50/50 unless the full launch history is
tracked. This is why storage must record both numbers, not a boolean
verdict (see Storage requirements below).

**Volatility:** this list is the more volatile of the two dev
categories — the denominator grows every day a creator launches
anything, migrated or not. A cached verdict for this category has a
shorter useful lifetime than category 2's.

**Discovery path:** per migration/graduation event (cheap — a discrete
on-chain event, not continuous polling), resolve the migrated token's
creator wallet, then backfill that creator's full launch history to
compute the ratio. Cache the ratio with its two raw counts, not just
the computed percentage — the percentage alone can't be updated
incrementally.

## Category 2: top devs by runner concentration in their portfolio

**Definition:** wallets whose deployed-token portfolio has a
disproportionately high share of actual runners (MC >= threshold, see
P0.8 for the measured runners/day baseline this threshold interacts
with).

**Volatility:** more static than category 1 — a runner, once it is a
runner, doesn't stop being one. A verdict here can be cached
aggressively; incremental updates only need to append newly-seen
launches, not recompute history.

**Static MC threshold usage:** MC >= $1M (or whatever P0.8 settles on)
is a cheap pre-filter to shrink the search space, not a selection
criterion by itself — it decides which tokens are worth resolving a
creator for, not which creators qualify.

## Category 3: smart money (insiders merged in as a scoring signal)

**Definition:** wallets with a durable, statistically real trading
edge — not a single lucky trade, not concentrated survivorship.

The screenshot review from earlier in this session is the concrete
anti-pattern to avoid: an 84/100 score built on 18 closed trades, no
out-of-sample check, PFactor=100 despite the tool's own text warning
"depends on a few big winners" (Top1=42.7% of all PnL). Every criterion
below exists specifically to not reproduce that failure mode.

### Selection criteria, in order of how much they actually discriminate skill from luck

1. **Multi-runner intersection.** Appearing among early buyers of >=3
   independent runners is far more selective than any single-trade
   size threshold, and costs nothing beyond the per-token candidate
   lists already being collected for categories 1/2. This is the
   `buyer-intersect` binary's exact job — no new mechanism needed.

2. **Out-of-sample survival.** Select on window A, re-verify on a
   disjoint window B. A wallet that only looks good on the window it
   was found on is not validated — this is the single differentiator
   no public leaderboard tool in this space does, and it's free (two
   date ranges, not two data sources).

3. **Realized PnL only, never paper gains.** `scout-ledger`'s FIFO
   already produces `realized_trade_pnl: Option<Money>` structurally —
   a held, unsold position contributes nothing to the score regardless
   of its current mark. This is not new work, it's using what P1-P5
   already built correctly instead of reaching for a shortcut.

4. **PnL concentration cap.** Reject or down-weight wallets where a
   single trade contributes more than ~40-50% of total realized PnL
   (Top1 concentration). The screenshot case (42.7%) sits right at
   this line and should not score 84/100 under this scheme.

5. **Co-occurrence dedup.** Wallets that buy in the same block/slot as
   another already-counted wallet are the same entity for scoring
   purposes (bundler/sniper cluster, or literally the same actor across
   fresh wallets) — count once, not N times. Without this, a single
   bot cluster can look like N independent smart-money signals.

6. **Weight by absolute realized PnL, not ROI%.** A 300% return on a
   $50 position is noise; the ranking attribute should be dollar PnL
   with a minimum trade-size floor, not percentage return.

7. **Recency decay.** A wallet whose entire edge is 8 months stale
   ranks below one with the same total historical PnL but recent
   activity. Exact decay function is a later tuning detail, not a P0
   blocker — the requirement is that *some* decay exists, not none.

8. **Fresh-wallet bucket, not a fabricated score.** A wallet with 1-2
   trades total cannot be scored with the same confidence as one with
   20+ — per AGENTS.md invariant #6/#10 ("Unknown does not serialize as
   zero"), such wallets get an explicit `unverifiable` /
   `insufficient-sample` status, never a numeric score that implies
   confidence the data doesn't support. This directly extends
   `scout-analytics`'s existing `RatioStatus`/quality-gate pattern
   (`config/scout.example.toml`'s `min_closed_episodes`) — see P4.3 for
   why the *global* threshold (20) is wrong for concentrated
   profiles and needs to become per-cohort.

### Insider signal (merged, not a category)

"Entered with a large size right before a pump on an already
high-cap/graduated token" (not the launch segment — that's bot/sniper
noise per the 2026-09-27 discussion) becomes one input feature to the
above scoring, not a separate pipeline:

- **Static $ threshold (e.g. $700) is an acceptable cheap pre-filter**
  to shrink the candidate set before scoring, exactly like the MC
  threshold in category 2.
- **For actual scoring weight, a relative measure is more honest** — %
  of the pool's liquidity at time of trade, not an absolute dollar
  figure. $700 into a $5k-liquidity pool and $700 into a $5M-liquidity
  pool are not comparable events.
- Detecting "before the pump" requires a price/liquidity time series
  derived from our own decoded AMM swap reserves (P4.1/P4.2 — not yet
  built). Until then, this specific signal cannot be computed; the
  other 8 criteria above do not depend on it and can proceed without
  it.

## Storage requirements (for `scout-storage`, before the schema freezes)

1. **Dev verdicts are not booleans.** Category 1 needs
   `(migrated_count: u32, total_launched: u32, last_scanned_slot: u64)`
   per creator wallet, updatable incrementally (append new launches,
   don't recompute from scratch). A single cached "good/bad" bit loses
   the information needed to update the ratio later.
2. **Negative cache for rejected devs.** Without one, a creator who
   fails the bar gets re-evaluated (and re-billed against provider
   quota) every time their name resurfaces via a new migration event.
   Cache the rejection with the same denominator fields as an
   acceptance, so a rejected-today creator can still be promoted later
   if their ratio improves.
3. **Category 2 (runner concentration) can use a simpler, longer-TTL
   cache** than category 1 — the portfolio-runner-count only grows, it
   doesn't need the same incremental-recompute machinery.

## Explicitly not solved here

- The exact numeric thresholds for criteria 2, 4, 6, 7 above (how wide
  a window for out-of-sample, exact concentration cap, decay half-life)
  — these are tuning parameters to set once real data exists, not
  architectural decisions to lock in now.
- Cross-network normalization (is a $10k realized PnL on Solana
  "equivalent" to $10k on Base, given different typical trade sizes
  and gas costs) — flagged, not resolved.
