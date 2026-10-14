# Dune as a free historical DEX-trade source — 2026-10-14

## Finding

Dune is a plausible one-off, exact-address **trade-history reconstruction** source for Solana and EVM networks, but it is not currently a sustainable Free-plan API alternative. Its current Free plan is view-only: no query executions and no API access. New accounts receive a trial with Plus features and 2,500 total credits for up to 14 days; it ends when either the time or credits run out. API access requires an active trial or paid plan, and API calls require a Dune API key. No account, hosted query, or API request was created or used for this review. [Dune credit system](https://docs.dune.com/learning/how-tos/credit-system), [API billing](https://docs.dune.com/api-reference/overview/billing), [API authentication](https://docs.dune.com/api-reference/overview/authentication)

That trial could fund a tightly scoped coverage experiment, but it is not a recurring monthly free allowance. The official credit documentation says the Free tier has zero credits and that trial credit consumption depends on query compute; exporting results through the API is a separate credit-consuming operation. Cost is not predictable from a fixed per-query formula. [Credit system and current plan table](https://docs.dune.com/learning/how-tos/credit-system), [API billing details](https://docs.dune.com/api-reference/overview/billing)

## Data and network scope

Dune documents two useful curated tables:

- EVM `dex.trades` records decoded trade events with chain, project/version, block timestamp, bought/sold contract addresses, decimal-adjusted amounts, raw `UINT256` amounts, `amount_usd`, emitting contract, transaction hash, and event index. Its documentation describes rows per swap leg in routed trades. The `amount_usd` field is the trade’s USD value at execution; it is a `DOUBLE`, not an exact fixed-point price field. [EVM DEX table](https://docs.dune.com/data-catalog/curated/dex-trades/evm/dex-trades)
- Solana `dex_solana.trades` has corresponding mint-address, raw/display amount, USD execution value, program, pool/project, signature, outer/inner instruction index, and transaction-order fields. Dune’s docs describe it as a curated set of DEX trades and example protocols include Raydium, Orca, and Serum. The table refresh is documented at about three hours. [Solana DEX table](https://docs.dune.com/data-catalog/curated/dex-trades/solana/solana-dex-trades), [Solana DEX overview](https://docs.dune.com/data-catalog/curated/dex-trades/solana/overview)

The DEX catalog overview says the curated DEX dataset covers 50 EVM chains plus Solana and lists Base, BNB, and Robinhood in its EVM-network navigation. This is network-level documentation, not proof that every factory, launchpad, migration, or token has decoded rows. The Robinhood chain page confirms Dune indexes the `robinhood` EVM schema and raw/decoded chain data, but does not name a Robinhood DEX-trades model or establish support for every Robinhood DEX. Dune’s chain page uses the `robinhood` schema name; it does not state chain ID 4663 there. [DEX coverage overview](https://docs.dune.com/data-catalog/curated/dex-trades/overview), [Robinhood Chain overview](https://docs.dune.com/data-catalog/evm/robinhood/overview), [Base overview](https://docs.dune.com/data-catalog/evm/base/overview), [BNB overview](https://docs.dune.com/data-catalog/evm/bnb/overview)

The documented schemas are suitable for reconstructing **observed trade execution prices** for rows actually decoded and returned. They do not directly provide an ATH or FDV field. A maximum of `amount_usd / token_amount` is a maximum observed execution average, not a pool’s marginal spot, candle high, or Codex-compatible “highest price ever.” Raw trade amount fields can help verify units, but the table’s USD price/value is floating point. It must not be treated as exact monetary accounting or as complete market coverage without token/pool/protocol validation.

Dune’s separate `prices.minute/hour/day` datasets are not a substitute for trade extrema: they return one `DOUBLE` price per time bucket, combine external Coinpaprika and DEX-derived sources, filter outliers, and derive long-tail prices from hourly volume-weighted averages subject to a $10k volume threshold. Documented forward-fill caps are 2 days for minute, 7 days for hourly, and 30 days for daily. Coverage is added after a token crosses the volume threshold. These prices therefore can omit low-volume launches and do not expose OHLC highs. [Prices methodology and schema](https://docs.dune.com/data-catalog/curated/prices/overview)

Dune’s DEX docs show queries over 365-day intervals, but do not promise a retention boundary or completeness from a particular token’s launch. Table refresh cadence and an example time filter are not proof of per-token history. Coverage of Pump.fun curves, PumpSwap versions, Base launchpad contracts, BNB launchpads, Robinhood AMMs, and migrated pools remains unverified in this documentation-only pass.

## Why it may help—and what it cannot establish alone

For a token and chain already represented by a supported model, a trial-funded query could find the maximum decoded execution average over the available history and return transaction/pool identifiers for receipt-level checking. It can complement a provider ATH by providing independent indexed trade evidence and may help identify missing pool/transaction candidates.

It does not independently establish:

- lifetime coverage of every pool, factory, bonding curve, or migration for a token;
- a marginal pool-state peak or within-candle high;
- historic circulating/total supply or FDV extrema;
- exact USD arithmetic, because the curated `amount_usd` and displayed amounts are floating point;
- completeness when a query returns no rows, or when a project/factory has no documented model.

## Unexecuted PoC outline

If a trial is intentionally activated, the smallest useful test is a few already-known Base and Solana addresses with local transaction/pool anchors, including one old token and one recent token per network. Query DEX tables by chain and exact contract/mint address, return row count, first/last event time, projects/versions/pools, null USD values, and top execution-average observations with transaction/event identifiers. Compare those rows to the project’s cached receipt/pool evidence. Do not query an unbounded year of minute prices; that table is explicitly a smoothed, thresholded price product.

Illustrative EVM trade-side expression (not validated against a live query):

```sql
WITH token_legs AS (
  SELECT blockchain, block_time, block_number, project, version,
         project_contract_address AS pool, tx_hash, evt_index,
         amount_usd / NULLIF(token_bought_amount, 0) AS usd_per_token
  FROM dex.trades
  WHERE blockchain = 'base'
    AND token_bought_address = 0x0000000000000000000000000000000000000000 -- replace
    AND block_month >= DATE '2026-01-01'
  UNION ALL
  SELECT blockchain, block_time, block_number, project, version,
         project_contract_address, tx_hash, evt_index,
         amount_usd / NULLIF(token_sold_amount, 0)
  FROM dex.trades
  WHERE blockchain = 'base'
    AND token_sold_address = 0x0000000000000000000000000000000000000000 -- replace
    AND block_month >= DATE '2026-01-01'
)
SELECT * FROM token_legs ORDER BY usd_per_token DESC LIMIT 100;
```

For Solana, use `dex_solana.trades`, compare the `VARCHAR` `token_bought_mint_address` / `token_sold_mint_address` columns directly to the exact base58 mint string, and retain signature plus outer/inner instruction indexes. `from_base58()` is for the separate Dune price-table `VARBINARY` address schema, not this trade table. These drafts return execution averages only. Validate amount semantics, USD valuation, and protocol coverage before using results; this is not an ATH query.

## Recommendation

Classify Dune as a **conditional diagnostic/research complement**, not as a new always-free production provider. If the owner chooses to spend the trial, first use it to test exact historical row coverage on a small, source-anchored set across Base and Solana. Include BNB and Robinhood only after checking each chain slug and actual protocol rows; the docs establish network indexing more clearly than model coverage. Stop if the model omits the relevant protocol or if 2,500 trial credits cannot cover both execution and result export. No change to ATH policy follows from documentation alone.

## Primary references

- [DEX Data overview: networks, refresh, method](https://docs.dune.com/data-catalog/curated/dex-trades/overview)
- [EVM `dex.trades` schema](https://docs.dune.com/data-catalog/curated/dex-trades/evm/dex-trades)
- [Solana `dex_solana.trades` schema](https://docs.dune.com/data-catalog/curated/dex-trades/solana/solana-dex-trades)
- [Curated price methodology](https://docs.dune.com/data-catalog/curated/prices/overview)
- [Current plan and trial credits](https://docs.dune.com/learning/how-tos/credit-system)
- [API billing/access requirement](https://docs.dune.com/api-reference/overview/billing)
- [API request/authentication reference](https://docs.dune.com/api-reference/executions/endpoint/execute-query)
- [Rate limits and Free API status](https://docs.dune.com/api-reference/overview/rate-limits)

## Evidence boundary

This is a primary-document review only. No Dune account/trial was activated, no SQL was hosted or executed, and no API key or data response was obtained. Protocol-by-protocol row coverage, first-history dates, API result pagination behavior for a candidate query, credits consumed, and whether each contract appears remain unverified.
