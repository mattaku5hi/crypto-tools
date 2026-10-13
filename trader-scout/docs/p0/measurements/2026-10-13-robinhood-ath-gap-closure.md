# Robinhood ATH gap closure: frozen 47-token cohort

Read-only audit captured through `2026-10-13T20:06:11Z`. The input is the RH subset of
`.scout-data/diagnostics/current-2026-10-13-top-runner-unknown-priority.json`
(snapshot time `2026-10-13T18:43:02.653522Z`, SHA-256
`1368a3473eeeac14c1f367b5695159617bf3250537624159298dbe74d19bae50`). It
contains 47 Doppler tokens with missing ATH. This pass ignored holder counts:
the objective was to find price evidence, not to decide runner eligibility.

Scope is each frozen token's canonical initial Doppler/v4 pool, from its
Initialize block through provider `latest`; this is not all-market/lifetime
coverage. Provider chain ID was 4663; the observed initial head was block
`87782416`. Requests used the configured RH RPC and `latest` block tags, so
they are not a finalized-head certificate. The pool manager is the gate
registry's Robinhood v4 manager; event topics and decoding follow
`crates/scout-dex-evm/src/uniswap.rs`. Price policy is exact raw Swap deltas,
token/quote decimals, and a block-pinned Chainlink USD answer, with no float
arithmetic. Current checkout HEAD at capture: `b1885c87d47a5b0bcd35e874dd44bce52bf247d6`.

## Results

Three tokens have a measured swap-price and same-block supply candidate from
their canonical initial Uniswap v4 pool. They are **scoped trade-price/FDV
observations, not lifetime ATHs**. The price is the executed token/quote ratio
multiplied by the Robinhood Chainlink USD quote at that block. Exact decimals,
total supply, block timestamp and block hash were read at the same block as
each selected swap.

| Token | Pool quote | Pool Swap events / blocks | Max observed price (USD/token) | Max event (UTC, block) | Oracle `updatedAt` | `totalSupply` at event block | Event-block FDV |
|---|---|---:|---:|---|---|---:|---:|
| `0x48c01e934d149b54af69398a3f767b7e399fb4aa` | AAPL token, 18 decimals | 15 / 2 | `0.000061526211916646941494161902837243273676442342308345733130752843662542607575906014` | `2026-07-29T17:53:17Z`, block `22791875` | `2026-07-29T17:17:07Z` (36m10s before) | `1000000000000000000000000000` (1B, 18 decimals) | `$61,526.211916646941494161902837243273676442342308345733130752843662542607575906014` |
| `0x4016fa67c597bf5e1991c852a6dfbb3eb9801e18` | AAPL token, 18 decimals | 6 / 2 | `0.000020345433945396543753380630510177430179642444598451401593671927692914700876574735` | `2026-08-03T21:06:27Z`, block `27216237` | `2026-08-03T20:38:58Z` (27m29s before) | `1000000000000000000000000000` (1B, 18 decimals) | `$20,345.433945396543753380630510177430179642444598451401593671927692914700876574735` |
| `0x01193b12f91826c2bf313d4b9620e83280a3fee1` | WETH, 18 decimals | 5 / 3 | `0.000010025653671760101336907967546616946769641584902294980036425470239717259846935917` | `2026-08-26T08:26:58Z`, block `46586316` | `2026-08-26T07:38:16Z` (48m42s before) | `1000000000000000000000000000` (1B, 18 decimals) | `$10,025.653671760101336907967546616946769641584902294980036425470239717259846935917` |

The AAPL quote is Robinhood's tokenized AAPL asset at
`0xaf3d76f1834a1d425780943c99ea8a608f8a93f9`; the WETH quote is
`0x0bd7d308f8e1639fab988df18a8011f41eacad73`. The saved Robinhood asset
registry binds the AAPL contract to AAPL, and the saved Chainlink feed
directory identifies the AAPL/USD proxy as
`0x6b22a786baa607d76728168703a39ea9c99f2cd0` and ETH/USD proxy as
`0x78f3556b67e17df817d51ef5a990cdaF09e8d3a9`. Both directory rows use 8
decimals. The Chainlink answer is a timestamped oracle mark, not a same-second
USD execution; its age is shown above. The AAPL and ETH quotes are also not
interchangeable with USDC/USD.

The candidate computation used exact integer swap deltas and decimal
arithmetic:

```text
price_usd_per_token = abs(quote_delta_raw) / 10^quote_decimals
                    / (abs(token_delta_raw) / 10^token_decimals)
                    * (Chainlink answer_raw / 10^8)
event_block_fdv = price_usd_per_token * totalSupply_raw / 10^token_decimals
```

All three winning swaps are from the initial Doppler v4 pools. The result is a
maximum over the complete observed Swap logs in those pools through the
captured provider head, not a market-wide maximum. The 1B supply is the
on-chain `totalSupply` observed at the winning event block; it is not a claim
that supply can never change. These values are evidence for a later scoped
price observation, but do not justify setting `ath_at` or filling a lifetime
ATH field.

Receipt checks add an important qualification. The checked AAPL winning
transaction has a successful receipt with two Swap logs from the selected
pool; it is a separate transaction after pool creation. The
WETH maximum is in the successful pool-creation receipt itself: its log order
is v4 `Initialize` at 17, Doppler initializer/Create logs at 31 and 36, then
two pool Swap logs at 39 and 41. Those are real v4 Swap events, but they are
not independent post-launch market trades; keep this row as a launch-transaction
price marker unless a product policy explicitly accepts that class of
execution. The frozen cohort's `created_at` differs from the corresponding
Initialize-block timestamp by +10s, +14s and -18s for these three tokens,
respectively. Use the block header/event time for the evidence above and audit
that small source-timestamp discrepancy before applying time-window gates.

## Remaining 44 keys

The 47 keys divide as follows:

| Evidence class | Keys | Finding |
|---|---:|---|
| Initial v4 pool present in the Oct 11 exact-token scan | 30 | All 30 had zero Swap logs from initialization through anchor block `85,914,253`. A tail query from `85,914,254` through provider `latest` also returned zero logs for all 30 pools. |
| Newly found initial v4 pool, but no Swap | 1 | The pool is quoted in PLTR token (`0x894e1ec2d74ffe5aef8dc8a9e84686accb964f2a`); zero Swap events were returned through provider `latest`. |
| No initial Robinhood v4 PoolManager `Initialize` found | 13 | These tokens were absent from both indexed-currency sides of the exact-address Initialize filters. This excludes only the configured initial v4 PoolManager route; it does not exclude v3, migrated pools, or other venues. |
| Initial v4 pool with swaps | 3 | Scoped price observations above; lifetime market coverage remains open. |

The 13 addresses with no initial v4 Initialize were:

```text
0x1e6489ef6d75c6f785843669b01262a7562b1e18
0x3d5733d2ac4d8f572aa22531e84bf620cc98fee1
0x41dbe23760ab7776e2615edeed946ef1225ffee1
0x52df349d941a790762a7474244a276ac1f371e18
0x60d63f32f0520178eabf4045cfb6e78941901e18
0x72edf2374f8605bca909f5b83f1a410c3416fee1
0x92ec1bfe5ed721faeab229ccde9414e22f729916
0x9aadb292cd6e17cdf98ad16d1a1caf1704041e18
0xa6134d94a96cf9ec60c075584b9cea73c6fafee1
0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1
0xc3a0e1a6c10b97c3e124ff8a2c291e5677a6fee1
0xcc799821eba30921d0f387f846d36cb6a0cafee1
0xd34b6aa31674b581ac37766198ce3a9350a3fee1
```

The 30 zero-Swap token/pool pairs are enumerated in
`.scout-data/diagnostics/rh-doppler-empty-window-protocol-audit-2026-10-11/initialize-hook-crosscheck.json`.
That file maps each token to its pool ID; the underlying complete query
manifest is
`.scout-data/diagnostics/empty-cohort-v4-scope-2026-10-11/manifest.json`
(SHA-256 `2fe8a1e067245eed1560bd503a86c5a93f49a558bc413a4e74fcbc3ee5e06548`).
It contains all 30 successful Swap queries, with zero returned logs. The
current 47-token cohort overlaps all 30 exactly.

## RPC evidence and limits

The read-only pass used the configured Robinhood RPC only. `eth_chainId`
returned `0x1237` (4663), and the initial `eth_blockNumber` was `87782416`.
Every request had a 20-second timeout, a 2 MiB response cap and no retries;
URLs, headers and credentials were not saved or printed. Total RPC calls: 30
of the 30-call budget. No database writes, provider price API calls, paid API
calls, or process changes were made.

For the 17 tokens outside the Oct 11 frozen v4 audit, two `eth_getLogs`
filters covered both indexed currency positions from block `18,411,837` through
`latest`. They returned zero currency0 Initialize events and four currency1
Initialize events. One filtered Swap request for those four pools returned 26
events across seven distinct blocks: 15 events in two blocks, 6 in two blocks,
and 5 in three blocks; one pool had zero swaps. Seven block-pinned
`latestRoundData()` calls used the saved official Chainlink AAPL/USD and
ETH/USD proxies. Three block-pinned reads each of `decimals()` and
`totalSupply()`, plus a winning-block header read per traded token, produced
the table above. A separate one-call tail query for the original 30 pools
returned an empty list.

Two `eth_getTransactionReceipt` reads confirmed the AAPL follow-up swap receipt
and the WETH creation-transaction log ordering described above.

The captured call summaries are not a retained raw-response fixture; exact
filters are described here, and the frozen cohort, prior scan, feed directory
and RH asset-registry bodies have local hashes/paths above. The new call bodies
were size-capped and discarded after bounded parsing. This is a reproducible
summary, not a claim that the raw live responses are archived.

## Next batch

The next useful batch is not another generic price-provider lookup. For the
44 still-unpriced tokens, resolve each Doppler `Create` receipt's exact
numeraire, then scan the canonical Doppler `Migrate`/pool route and bounded
Uniswap v2/v3/v4 pool swaps from that creation block to a pinned head. For
stock-token quotes, use the matching Chainlink USD feed at swap blocks; for
WETH, use the ETH/USD feed. Keep quote conversion age and event-vs-oracle
timing provenance. Promote only if route coverage supports the claimed
completeness; otherwise retain a named scoped-window price and leave lifetime
ATH unknown.

This pass changes no stored ATH values. It does not establish that the 44
remaining tokens had no trades on alternate routes, or that the three scoped
maxima are lifetime ATHs.



## Follow-up: captured raw, exact-UTC quote proof (2026-10-13)

This follow-up supersedes the earlier Chainlink-point draft above. The earlier
AAPL and WETH prices are **not promotion evidence**: the AAPL feed observations
were 27–36 minutes stale, and the previous WETH maximum was in its pool-creation
transaction. The ISO timestamps below are independently derived from the raw
block header epoch seconds in UTC. This corrects an earlier rendering that
showed the second event as Aug 3 21:06Z; its actual UTC time is **Aug 4 01:06:27Z**.

A new private raw capture is retained under
`.scout-data/diagnostics/rh-ath-gap-proof-2026-10-13/capture-201717/`.
`candidate-review.json` is the bounded review index. It records exact request
methods and public params, raw response paths and SHA-256, with URLs, headers
and credentials omitted. The capture contains 45 successful RH method calls (the approved cap),
all responses below 2 MiB, and two bounded keyless market-data responses.
All raw files are mode 0600. The Robinhood head was block 87,790,837, hash
`0x2827281fc77f2798b5a6a28650c99f01e667a9dddb2d55d07a3cf7b1dfaad69b`.

Two tokens now have independently priceable **initial-pool scoped maxima**
from exact successful swap receipts and contemporaneous traded-market FX.
These are evidence candidates only; no database rows were written, and neither
value claims lifetime ATH or alternate-pool completeness.

| Token | Maximum successful pool event | Exact quote conversion | Scoped price / 1B-supply FDV range |
|---|---|---|---|
| `0x4016fa67c597bf5e1991c852a6dfbb3eb9801e18` | v4 pool `0x2aec506e…aab50e`; block 27,216,237, log 22; `2026-08-04T01:06:27Z`; receipt status 1. Its `AAPL/token` ratio is `6.672165397106399420647568462e-8`. This is the max of all six pool swaps through the captured head. | Official Robinhood v3 factory returned the USDG/AAPL fee-3000 pool at the target block. Successful receipt-backed executions bracket the target at `01:06:23Z` (4 seconds before; `305.1120173979994371650550324 USDG/AAPL`) and `01:06:41Z` (14 seconds after; `305.1214082368890032291383663 USDG/AAPL`). Kraken's closed Aug 4 USDG/USD candle had low `0.9997`, high `1.0000`, volume `821672.53056` and 586 trades. | `$0.00002035147117370840286292845360–$0.00002035820501954546327338853483` per token; `$20,351.47–$20,358.21` FDV. |
| `0x01193b12f91826c2bf313d4b9620e83280a3fee1` | v4 pool `0x3f7a4a3c…c377490`; block 46,586,365, log 11; `2026-08-26T12:27:03Z`; receipt status 1. The `WETH/token` ratio is `4.073810825100821925439307761e-9`, the highest of three post-creation pool swaps. All creation-transaction swaps were excluded. | Coinbase Exchange ETH/USD closed one-minute candle for `12:27Z`: low `$2,467.54`, high `$2,470.43`, volume `128.24108893 ETH`. The minute contains the target block timestamp. | `$0.00001005229116336928213389850947–$0.00001006406447665382350926302907` per token; `$10,052.29–$10,064.06` FDV. |

The selected AAPL and WETH receipts match the raw pool Swap log index and block
hash. The two AAPL/USDG neighbor receipts are also successful and match their
queried pool logs and header hashes. The target token decimals are 18 and
`totalSupply()` at each selected maximum block is exactly `1,000,000,000e18`.
Each creation receipt records the single `1e27` raw-unit mint; the three clones
have the same 44-byte runtime and embed the source-verified Doppler
implementation. A bounded Transfer scan from all three token contracts through
the pinned head returned zero burns. The implementation exposes caller burn,
so the statement is scoped to this captured history rather than immutable
supply for all time.

The third token, `0x48c01e934d149b54af69398a3f767b7e399fb4aa`, remains without a
USD candidate. Its two AAPL/USDG executions found in the queried exact fee-tier
pools are both after its target swap at `2026-07-29T21:53:17Z`; there is no
before/after market-price bracket within five minutes. No USD price was
invented from the stale AAPL oracle.

These captures establish exact source-scoped execution prices and support
bounded FDV calculations, not a provider-window high. Existing ADR-021 source
labels describe provider-window observations and cross-checks; keep these two
candidates unpromoted until the project confirms the accepted source kind and
storage semantics for a direct on-chain scoped maximum. `ath_at` remains null.


## Deterministic reference-close model across every observed swap

The previous table's bands are sensitivity ranges. This follow-up also evaluates
each event under a deterministic reference-close model, choosing the maximum
only after FX weighting. Exact integer ratios and event-level results are in
`candidate-review.json`; no binary floats are used.

For the WETH pool, each of the three post-creation swaps is priced using the
Coinbase ETH/USD close in its own UTC minute: both block-46,586,365 events use
`$2,470.30` at `12:27Z`; the block-46,587,186 event uses `$2,469.04` at `12:28Z`.
The winning event remains block 46,586,365 log 11, with reference-close price
`$0.00001006353488124656040241272196` and 1B-supply reference FDV
`$10,063.53488124656040241272196`. Its same-candle low/high sensitivity is
`$0.00001005229116336928213389850947–$0.00001006406447665382350926302907`
(`$10,052.29–$10,064.06` FDV). The price is a minute-close reference model,
not a second-level fiat execution or a wallet's realized price.

The AAPL pool's six swaps occur in two timestamp groups, blocks 27,216,237
(`01:06:27Z`) and 27,216,411 (`01:06:44Z`). Each group has its own actual
USDG/AAPL executions on both sides. The before/after ratios differ by only
`0.00307784%` for the first group and `0.00229727%` for the second; all four
bracket positions are verified in three unique successful v3 receipts, and the
trades are 3–14 seconds from their corresponding v4 event blocks. For each swap, the reference price uses the last successful
v3 execution before its group and Kraken's Aug 4 USDG/USD daily close `0.9999`.
The daily low/high (`0.9997–1.0000`) and both neighboring market executions
are retained as the uncertainty band. Across all six swaps, the maximum stays
at block 27,216,237 log 22: reference-close price
`$0.00002035554268939785137805557743`, reference FDV
`$20,355.54268939785137805557743`; its bracket plus daily FX sensitivity is
`$0.00002035147117370840286292845360–$0.00002035820501954546327338853483`
(`$20,351.47–$20,358.21` FDV). This uses traded USDG conversion, not an assumed
peg or a stale stock-token oracle.

These two model outputs are the proposed direct on-chain source-specific
reference-priced maxima for root's ADR/golden review. They cover all events
observed in the captured canonical initial pools and preserve explicit market
uncertainty. They remain proposals: no source-kind, policy, database or ATH
record was changed, and neither number claims complete lifetime or alternate-
pool coverage.

## Root integration and guarded persistence (2026-10-13)

After independent verification, root applied two missing-only guarded writes
under ADR-021 amendment 27a. The stored reference-priced scoped maxima are
`$20,355.54` (`2,035,554` cents) for `0x4016fa67c597bf5e1991c852a6dfbb3eb9801e18`
with source `onchain-pool-window-supply-stock-trade-fx-reference-v1`, and
`$10,063.53` (`1,006,353` cents) for
`0x01193b12f91826c2bf313d4b9620e83280a3fee1` with source
`onchain-pool-window-supply-cex-reference-v1`. Both keep `ath_at=NULL` and
`market_lifetime_complete=false`; the guard preserved existing positive values
and metadata. `observed_at` is `2026-10-13T20:42:30Z`. The root's private
[`promotion-manifest.json`](../../../.scout-data/diagnostics/rh-ath-gap-proof-2026-10-13/capture-201717/promotion-manifest.json)
records both exact fractions, sensitivity bounds, sources and event keys.

Root independently checked 47 captured response hashes and all enumerated
reference-priced events against chain ID 4663. The immutable
`candidate-review.json` SHA-256 remains
`06e56f774c800cb8e7539159ddc1b6cdc9a2e82d6b33ca495e98a001abed3399`; the
private [`root-independent-verification.json`](../../../.scout-data/diagnostics/rh-ath-gap-proof-2026-10-13/capture-201717/root-independent-verification.json)
contains the independent result. The post-write audit reports 442 numeric / 237
unknown ATH values overall, with Robinhood at 8 numeric / 45 unknown. Annual ATH
coverage remains incomplete.

One private candidate-index field is historically misnamed:
`burns_from_zero_through_head` counts the raw `Transfer` scan with `to=0x0`,
which is a burn scan. The retained raw query and zero result are correct; do not
interpret the field name as a query for mint events or rewrite the sealed
candidate index.


## Follow-up: receipt-grounded audit of the 13 apparent missing pools (2026-10-13)

The prior statement that the 13 listed tokens were absent from both exact-address `Initialize` filters was incorrect. The six Robinhood filters retained in the Oct 11 manifest contain 30 distinct queried token addresses; their intersection with these 13 is zero. The retained filter inputs therefore do not substantiate the earlier claim that these 13 were tested; this is an input-scope mismatch, not evidence of a provider index gap. Separately, two of the 13 creation blocks predate the old filter's `fromBlock=18,411,818`, which would exclude them even if their addresses had been supplied.

A fresh indexed lookup used `launches_pkey(chain, token)` for exactly the 13 addresses. For every indexed creation transaction, the receipt has status `1` and its block equals `created_block`. Each of the 13 receipts contains exactly one `Initialize` from Robinhood's configured v4 PoolManager (`0x8366a39cc670b4001a1121b8f6a443a643e40951`), one matching `Create` from the official Doppler Robinhood hook (`0x4e3468951d49f2eea976ed0d6e75ffcb44a9a544`), and one Airlock `Create` from `0xeb7c034704ef8dcd2d32324c1545f62fb4ad0862`. The v4 event currencies match the token and the Airlock numeraire; the hook event agrees on PoolManager, asset, and numeraire. The known official deployments and event semantics are documented in the [protocol audit](2026-10-11-rh-doppler-empty-window-protocol-audit.md) and its linked first-party deployment/source references. No v2 `PairCreated` or v3 `PoolCreated` topic appears in these creation receipts. That receipt-local observation does not exclude later migration or alternate venues.

One bounded `eth_getLogs` query filtered the 13 exact PoolIds at the configured PoolManager, from the minimum indexed creation block `17,403,260` through pinned head `87,808,917`. It returned 150 Swap logs (143,136-byte response): 12 pools have at least one observed swap, and one has none. Results by token:

| Token | Indexed creation block | PoolId | Initial quote | Swap logs / distinct blocks |
|---|---:|---|---|---:|
| `0x1e6489ef6d75c6f785843669b01262a7562b1e18` | 17,403,260 | `0x7cdf69ad…dd55acc` | NVIDIA token | 3 / 2 |
| `0x3d5733d2ac4d8f572aa22531e84bf620cc98fee1` | 46,091,348 | `0x9ce24e14…ee1b101` | WETH | 62 / 26 |
| `0x41dbe23760ab7776e2615edeed946ef1225ffee1` | 46,817,004 | `0x23378db6…5936c628` | WETH | 2 / 1 |
| `0x52df349d941a790762a7474244a276ac1f371e18` | 17,438,422 | `0x5cb4bc87…919ba5815` | `0x86923f96…ad2023fdc` | 3 / 2 |
| `0x60d63f32f0520178eabf4045cfb6e78941901e18` | 65,505,396 | `0x9cebf11e…253d58a9` | `0x1937cad4…887c46c33` | 37 / 8 |
| `0x72edf2374f8605bca909f5b83f1a410c3416fee1` | 53,450,437 | `0x555dfc60…cd7efc05` | `0x1b0e319c…911c153e` | 2 / 1 |
| `0x92ec1bfe5ed721faeab229ccde9414e22f729916` | 18,747,375 | `0x7a4ea230…e4209ac5` | `0x86923f96…ad2023fdc` | 0 / 0 |
| `0x9aadb292cd6e17cdf98ad16d1a1caf1704041e18` | 21,054,304 | `0x2423ed08…a887306a4` | `0x322f0929…1c003b2d` | 6 / 2 |
| `0xa6134d94a96cf9ec60c075584b9cea73c6fafee1` | 41,615,942 | `0xc54510d6…dfedc76` | WETH | 2 / 1 |
| `0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1` | 44,528,731 | `0x9d540ddc…4037f374` | WETH | 5 / 3 |
| `0xc3a0e1a6c10b97c3e124ff8a2c291e5677a6fee1` | 49,434,763 | `0x97b03341…6eadc0a2` | WETH | 4 / 2 |
| `0xcc799821eba30921d0f387f846d36cb6a0cafee1` | 46,870,851 | `0x4c1a9a9a…ad9b8c69` | WETH | 12 / 6 |
| `0xd34b6aa31674b581ac37766198ce3a9350a3fee1` | 46,140,507 | `0x76dffd69…012db13d` | WETH | 12 / 8 |

For the three strongest pools with an established WETH-to-USD reference path, all 80 post-creation Swap events (60, 10, and 10) were evaluated: each event was mapped to its block-header UTC minute, matched to the corresponding Coinbase candle close, and compared by reference USD price. Their exact maxima were selected after excluding swaps in the creation transaction. The 37 distinct event blocks all have captured headers. Each selected maximum has a successful receipt, matching log index, and block header. Token `decimals()` is 18, WETH `decimals()` is 18, and `totalSupply()` at each winning event block is `1,000,000,000` tokens. From creation through that winning block each token has one mint-from-zero Transfer (in its creation receipt) and zero burn-to-zero Transfers. The resulting supply is therefore supported for the event-block FDV; this does not establish later supply or complete market lifetime.

| Token | Maximum event (UTC, block, log) | WETH/token raw ratio | Coinbase ETH/USD minute close (low–high) | Reference price / 1B-supply event-block FDV |
|---|---|---:|---:|---:|
| `0x3d5733d2ac4d8f572aa22531e84bf620cc98fee1` | 2026-08-25 22:36:12Z, 46,091,504, 34 | `712988969068359 / 53742746125043824306235` | `$2,448.71` (`$2,448.00–$2,450.91`) | `$0.0000324863045588546891` / `$32,486.30` (range `$32,476.89–$32,515.49`) |
| `0xcc799821eba30921d0f387f846d36cb6a0cafee1` | 2026-08-26 20:27:17Z, 46,871,356, 32 | `1468474551458379 / 295379769482336433421184` | `$2,466.60` (`$2,466.30–$2,467.08`) | `$0.0000122626520258146515` / `$12,262.65` (range `$12,261.16–$12,265.04`) |
| `0xd34b6aa31674b581ac37766198ce3a9350a3fee1` | 2026-08-25 23:58:32Z, 46,140,551, 94 | `156774035930379 / 11013308902860520896662` | `$2,441.94` (`$2,441.70–$2,444.26`) | `$0.0000347609235949420566` / `$34,760.92` (range `$34,757.51–$34,793.95`) |

These are minute-close reference prices, not second-level USD executions or wallet PnL. The reference maximum is selected across all event-level quote/token ratios scaled by each event's matching UTC Coinbase ETH/USD minute close; for all three pools the highest quote/token ratio also happens to be the highest USD reference value. The minute low/high is retained as sensitivity. The event candles cover one UTC minute for the first pool, three for the second, and two for the third. The aggregate `eth_getLogs` scope reaches the pinned head, but discovery still covers only each canonical initial Doppler/v4 pool; migration pools and other venues remain open. No ATH/DB rows were written.

Private, mode-0600 request/response captures and hashes are under
`.scout-data/diagnostics/rh-doppler-no-v4-13-2026-10-13/capture-204732/`;
[`followup-13-results.json`](../../../.scout-data/diagnostics/rh-doppler-no-v4-13-2026-10-13/capture-204732/followup-13-results.json)
is the compact review index. The receipt and all-13-pool scan used 16 Robinhood RPC methods; exact post-launch event/header, receipt, token and WETH decimals, total-supply and bounded mint/burn checks brought the total to 71 Robinhood RPC methods. Four public Coinbase candle GETs cover the six distinct event minutes. No retries, DB writes or process changes were made. This follow-up supersedes the earlier 13-token “no initial Initialize found” classification above; the historical 30-token, zero-Swap findings and their bounded scope are unaffected.
