# ADR-018: USD execution pricing for the Solana wallet ledger (P5.2, first slice)

Status: Accepted (2026-10-03)
Date: 2026-10-03
Extends: ADR-001 (`Money` in USD), ADR-010/013 (quote-unit ledgers). Opens the USD view required by
CLI.md §4 ("cross-chain/quote comparison only in USD") and ARCHITECTURE §10.

## Context

Ledgers are exact in their quote units (lamports, USDC, USDT; ADR-013). Wallets that mix SOL- and
USDC-quoted trades cannot be compared in one column, and cross-quote episodes (USDC buy → SOL sell)
are `Unknown { CrossQuoteUnit }`. A USD execution price per trade is needed, with provenance and
without inventing values (invariant #6).

Sources checked live on 2026-10-03: Pyth Benchmarks and Hermes historical endpoints now return
HTTP 401 without credentials; Kraken public OHLC returns only the last ~720 one-minute candles;
**Coinbase Exchange public candles** (`GET /products/{SOL-USD|USDT-USD}/candles?granularity=60&start&end`,
≤300 candles per request, no key) return historical one-minute candles `[time, low, high, open,
close, volume]` for SOL-USD and USDT-USD (no USDC-USD product: Coinbase treats USDC as USD).

## Decision

1. **Price source (v1).** `PriceSource` port in `scout-pricing` with a Coinbase adapter for SOL-USD
   and USDT-USD one-minute candles. The execution price of a trade at block time *t* is the candle
   `[m, m+60)` containing *t*: value = `close` (decimal string parsed exactly into an integer
   rational; never `f64`), with `low`/`high` carried as the uncertainty band and `volume` as a
   liquidity hint. Quality label `cex_reference_1m` (a reference price, not the on-chain execution
   price).
2. **Missing data.** No candle for minute *m* (no trades on Coinbase) → previous candle within
   ≤ 5 minutes, labelled `stale_{k}m`; otherwise `PRICE_UNKNOWN` (never zero, never interpolated
   across a larger gap). Provider failure → `PRICE_UNKNOWN` with the error class, coverage partial.
3. **Stablecoins.** USDT → USD via USDT-USD candles (actual, depeg-visible). USDC → USD at par with
   explicit label `usdc_par_assumed` (no keyless historical USDC/USD source verified); the report
   shows the count of USDC-valued legs so a depeg assumption is visible. A future verified source
   replaces the assumption without changing ledger semantics.
4. **USD ledger view.** Each acquisition lot also records `usd_basis = quote_basis × price(t_acq)`;
   each disposal `usd_proceeds = quote_proceeds × price(t_disp)`; USD realized PnL per episode =
   Σ usd_proceeds − Σ usd_basis of consumed lots. Conversions are checked integer arithmetic to
   `Money` at `MONEY_SCALE`, rounding **half-even once per conversion**, documented in one function.
   Cross-quote episodes become `ClosedKnown` in USD when every leg is priced; any `PRICE_UNKNOWN` leg
   → the episode is unknown in USD (never zero). Native quote-unit ledgers are unchanged and remain
   the exact primary record.
5. **Scope of v1.** Realized USD PnL, USD consumed basis, USD ROI/PF/win rate (ADR-016 lower bounds
   apply), per-wallet price coverage (`priced legs / legs`, labels histogram). Open positions stay
   unvalued (end-of-window valuation needs token prices → later slice). `wallet-rank --quote usd`,
   wallet-stats USD block. Budget: candle requests are bounded and cached per (product, 300-minute
   page) per run; counted in `requests_made_prices`.
6. **Reproducibility.** `run_meta` records the price source, products, policy version, staleness
   limit and the USDC assumption.

## Consequences

- SOL- and USDC-quoted wallets become comparable in USD with explicit price provenance.
- USD PnL mixes trading skill with SOL/USD drift by construction (standard USD accounting); the
  native-unit PnL stays available beside it.
- Coinbase is a single reference venue; an independent cross-check source can be added later
  (ARCHITECTURE "independence"), and the labels make the current dependence explicit.
