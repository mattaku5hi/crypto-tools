# RH 48c AAPL/USDG bracket follow-up

Read-only follow-up captured 2026-10-14. It revisits token
`0x48c01e934d149b54af69398a3f767b7e399fb4aa`, for which the Oct 13 note said
that its USD quote bracket was incomplete. The additional one-hour pre-event
query found a successful AAPL/USDG trade 148 seconds before the token-pool
maximum. Together with the already captured same-pool trade 8 seconds after,
the maximum now has a two-sided market-price bracket. This corrects the earlier
negative-control finding; it does not establish a lifetime ATH.

## Evidence and calculation

The target’s initial v4 `Initialize` in its successful creation receipt binds
pool `0x591948ec42765aa9f190e62fd3cec3fa8a5ea13d593c9e8d58858debe960a1ab`
to currency0 = the target and currency1 = Robinhood AAPL
(`0xaf3d76f1834a1d425780943c99ea8a608f8a93f9`). The saved v4 Swap scan covers
that pool from creation through the captured provider head and contains 15
post-creation swaps: 10 at `2026-07-29T21:53:17Z` and five at
`2026-07-29T21:53:32Z`. The first group’s maximum is log `0x81` in block
22,791,875. Its receipt succeeded and includes the matching pool Swap log.

All 10 swaps in the winning time group were compared using exact raw amounts.
Currency0 and currency1 both have 18 decimals. The maximum is
`551870417754093356 / 3126448373066587552731649` AAPL per target token. In the
second time group, the maximum is log `0x58`,
`1.784025260842861082171641026584273232821945406526764314698484790178130804818583649163118817871098652e-7`
AAPL per token, below the winning group even at the upper quote and FX bounds.

The AAPL/USDG v3 market pair is verified at the event block. USDG is token0
(6 decimals) and AAPL token1 (18 decimals). The winning group is bracketed by
successful trades in the same fee-3000 pool:

| Side | UTC time | Block | Pool fee | Executed USDG/AAPL |
|---|---|---:|---:|---:|
| Before, 148s | 21:50:49 | 22,790,408 | 3000 | 339.5219522393341981204724753217162028116868382933228544263501106243126580995244… |
| After, 8s | 21:53:25 | 22,791,960 | 3000 | 339.5343573021331534303666261966405568544452457541155591005989128473739443822451… |

The second token-pool time group is also bracketed within 30 seconds. Its
before-side is the 21:53:25 fee-3000 trade; its after-side is a 21:54:02
fee-500 trade in another pool for the same verified USDG/AAPL pair. No
additional fee-3000 trade appeared in the subsequent 3,666-block query. The
fee-tier difference is retained as a scope qualification, not merged away.

The saved Kraken public OHLC response has the July 29 UTC USDG/USD daily
candle: low `0.9999`, high `1.0002`, close `1.0000`, volume `586296.75513`,
683 trades. This uses the observed USDG market range; it does not assume a
perfect dollar peg. Combining the winning target-pool execution with the two
bracket executions and that same-day FX range gives:

| Quantity | Range / reference |
|---|---:|
| Winning event price | `$0.0000614278153804219910244931816195`–`$0.0000614484906203409093334760763110` per target token |
| 1B-supply FDV at that event | `$61,427.8153804219910244931816195158`–`$61,448.4906203409093334760763109936` |
| ADR-021 27a point: last prior USDG/AAPL execution × daily USDG/USD close | `$0.00006143395877629962098659184080359620861752976460715109648281904341230564204312303690737935743595634104` per token; `$61,433.95877629962098659184080359620861752976460715109648281904341230564204312303690737935743595634104` FDV; floor `$61,433.95` |

The point reference follows ADR-021 amendment 27a: for each successful target
swap, use the last successful USDG/AAPL execution before that timestamp group,
then multiply by the Kraken USDG/USD daily close. Convert all 15 target swaps
individually and choose the maximum after conversion; no midpoint is used. The
winning event is log `0x81` in the first group. Its selected reference is
`339.5219522393341981204724753217162028116868…` USDG/AAPL, giving
`$0.00006143395877629962098659184080359620861752976460715109648281904341230564204312303690737935743595634104`
per token and `$61,433.95877629962098659184080359620861752976460715109648281904341230564204312303690737935743595634104`
FDV for the captured 1B supply. The exact floor is **6,143,395 cents
($61,433.95)**.

The sensitivity band is separate from the point: it combines the lower/higher
successful USDG/AAPL bracket executions with the Kraken daily USDG/USD low/high.
It is deliberately wider than a same-minute quote. The second group’s
per-event close-reference maximum is about `$60,573.7870` FDV, below the first
group’s band; it does not change the configured-pool winner.

At the maximum event, `decimals()` returned 18 and `totalSupply()` returned
`1,000,000,000e18`. The creation receipt records the single initial mint of
`1e27` raw units. A Transfer-from-zero query after creation and a Transfer-to-zero
query from creation through the captured head both returned no logs. This
supports the historical 1B denominator for the captured interval; it is not a
claim that supply is immutable in the future.

## Scope and status

The maximum is an executed price in one configured initial v4 pool, converted
through observed AAPL/USDG v3 executions and a same-day USDG/USD daily candle.
It is not a market-wide or lifetime maximum, nor a wallet’s net acquisition
cost. Other pool venues and price history before the initial pool’s first
market swaps remain outside this certificate. The second group’s upper bound is
below the first group’s lower bound, so it does not change this pool-window
winner. The candidate and its exact rational inputs are in
`.scout-data/diagnostics/rh-48c-aapl-usdg-certificate-2026-10-14/candidate.json`
(schema v2; SHA-256 `988799f1fd6b5a105eacbad67e5698109378467c3519f617fa08147989c080c4`).
No database row or `ath_at` value was written; root review is still required
before any guarded promotion.

Raw evidence is kept in the private diagnostic directories referenced by the
candidate manifest. Key hashes include the original v4 Swap scan
`461abd2ba866628e493b6fb83d35ea0ee78a98af4ef5bf3d698e8d91f94723dc`,
the winning receipt `bec69c6bd188e72cdde7c3db19c329494d729d96e89aafec2aa7c3e6bdb87807`,
the before-event AAPL/USDG scan `bcdd2adf0f2c655f245aea8654b9ff25d6d52a6f18525412c050c8adca1c4171`,
the after-event AAPL/USDG scan
`7d7bdeebd874be8474b15eb88ff38372b22a2b7f007f175ff5b497292d1328da`,
and the daily Kraken OHLC response
`2c3222370fb29cf678eb33bc08fd0112fc1a175a2e5ec78e38c069987b460517`.

The Kraken response came from the public [OHLC endpoint](https://api.kraken.com/0/public/OHLC)
with `pair=USDGUSD`, `interval=1440`, `since=1785283200`. The exact API
response and request provenance are in the Oct 13 capture manifest; the
bracketing query, receipts, decimals, supply and mint scan are in the new Oct 14
diagnostic manifests. All new RPC requests were read-only, response-capped at
2 MiB and made without retries; no Codex, Birdeye or database request was made.
