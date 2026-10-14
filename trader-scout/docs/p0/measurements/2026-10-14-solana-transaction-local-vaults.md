# PumpSwap transaction-local vault states — 2026-10-14

Two manually verified provider-window candidates use the conservative2%
lower-bound policy in ADR-021 amendment40, after the missing transaction-
local states are recovered under amendment41.

| Token | Provider × current supply, lower FDV USD | Full pre/post states | Price maxima offset |
|---|---:|---:|---:|
| `CtN5TQw6…` | 75,473.02 | 202 | −3s |
| `G3HyEDPZ…` | 58,546.39 | 84 | −1s |
| `69KXHdb7…` negative | Not accepted | 32 | +1s |

The transaction creates the pool and vault ATAs before its first buy.
Consequently the vaults are absent from transaction-start preTokenBalances.
Ordered, identity-checked SPL funding transfers determine the actual swap-pre
state. The buy's exact scoped transfers and dated BuyEvent reproduce the
canonical post balances. G3Hy has truncated Program-data logs but retains
the intact event-CPI payload. Explicit event-authority and parent-stack-scope
binding permit this individual proof. Unknown CreatePool event bytes remain
retained; no generic production fallback is added.

All318 complete selected-pool neighborhood pre/post states are retained for
maximum selection. The 69KX full-window maximum is 6.8–6.9% above the saved
provider price and remains unaccepted. Its winning transaction was already
among the earlier verified states; recovering the missing first buy closes
the evidence gap rather than creating that maximum. For the other two,
stored lower scalars are within2% of both independent price/supply endpoints
and unrounded provider/birth-supply upper bounds, including cent rounding.
These are reference lower estimates; exact peak supply and full-year/all-
market ATH remain unknown. Holders/liquidity and runner thresholds are kept.

Root interval proof SHA-256:
`80e48c14b032a1b0d36f0956ba8384295ebdc8b61efd66e637e93d3e52ea1c57`.
Strict transaction/full-window proof SHA-256:
`127348ce325353679112d28958e7009dba29d9c25ce8b41a5047979274074976`.
Five standard finalized transaction recaptures match messages and selected
metadata; no Tracker/Codex/Coinbase calls are added in that phase.

Public regression: `solana_tracker_pumpswap_prebalance_window_cases.json`
and the shared Birdeye test module. Workspace fmt, all-target/all-feature Clippy with warnings denied and1233
tests passed in50.21seconds against isolated scout_test. The subsequent
atomic guarded transaction wrote only CtN5 and G3Hy; exact read-back confirms
source, lower floors, null ATH dates and preserved holder/liquidity metadata.
Fixed679cohort:504numeric/175unknown;70closures since434baseline.

Changed files: shared Birdeye test, three-case golden, ADR-021, this and central
measurement, tickets; SDK schema research and state checkpoint documentation
are recorded alongside them. Next task: B8.4, remaining historical maxima.
