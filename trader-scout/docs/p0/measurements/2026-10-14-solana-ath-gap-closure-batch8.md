# Solana ATH follow-up: terminal signature pagination, batch 8

Eight further native-SOL Mayhem curve marks were independently verified and
written with exact row/provider guards. The frozen 679-token cohort reaches
482 numeric / 197 unknown: 48 closures since the 434/245 checkpoint.
This is not annual all-market ATH completeness.

| Mint prefix | Birth-to-peak successful transactions | Stored FDV, USD |
|---|---:|---:|
| `G3VL3JPu…` | 14 | 5,650.21 |
| `3UzVm7cf…` | 202 | 24,354.30 |
| `bNf6AiLn…` | 65 | 8,511.71 |
| `3a1aqd97…` | 22 | 6,091.37 |
| `3nhRZuke…` | 62 | 11,007.00 |
| `3jrcVETa…` | 84 | 12,089.86 |
| `4rENUvCF…` | 25 | 11,502.18 |
| `EzqFMWBu…` | 229 | 10,522.06 |

All 140 positive neighboring TradeEvents match actual Pump program log
context and event-CPI bytes. The global neighborhood maxima occur at the
provider timestamp's second or one second earlier. Each passes the exact
2% price gate at both dated SOL/USD endpoints; every captured neighboring
event is in that actual minute. Two birth mints issue 2B six-decimal tokens,
authority is revoked at birth, no burn occurs through the price peak, and
current extensions are metadata only. Stored FDV is the exact saved provider
price times independently verified supply. SDK × FX endpoint bands are price
corroboration, not supply uncertainty or an all-market maximum bound.

Ezq's first finalized signature page contains 1,000 rows and is nonterminal.
Two cursor-chained continuations contain 564 and zero rows. All 1,564 unique
signatures have known times: 228 successful and 1,336 failed. Successful
birth-to-peak signatures plus the explicitly excluded birth match all 229
TFA transactions. Failed/outside rows and raw captures remain retained;
failed execution is not a supply effect. The same scoped-set comparison
holds for the other seven histories. A nonterminal or unknown-time query
would remain a gap (ADR-021 amendment 37).

Standard finalized recaptures match entire TFA messages, balances, inner
instructions and logs. System transfers within each actual Pump buy match
curve and fee/creator legs and ceiling fee rounding. Fee-payer balances are
not used as quote amounts. Eight atomic guarded updates preserve known
positive rows, holders/liquidity and `ath_at=NULL`. The source remains the
explicit provider-window Mayhem curve-mark reference; full Mayhem transitions
and annual/all-market history are not reconstructed.

The ApRm negative control's three neighboring marks have a global maximum
about 8.7% from the provider price. It remains unknown. The one AMM input
is not evaluated with the curve formula. No selection threshold changed.

Capture accounting: 57 initial Helius methods, two signature continuations
and eight standard finalized recaptures: 67 of 85 methods. Ten Coinbase GETs;
no Solana Tracker/Codex calls, retries or new provider keys. Root proof hashes:

- price: `d037e37306bac71c38f626b2b7a4f672c28096ed37aa7f048d4ab69b81b8ba15`;
- supply: `26f6ee8acea9d10cec2ccaf18636e0d989c6778f9dca6f555918312f1a98e63c`;
- native: `c32571e54b60cadfaa03dcefd98fcd529d6f79951915224c8e5d6a69b754fd94`.

The final separate worker sidecar is
`c2544e49bc8dd8932062603fdb33950cf9ac7ef09caa12b445d8b00e101fed6b`;
earlier revisions and all raw captures remain separately retained under
`.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch8/`.

The shared captured-byte regression includes the 140 positive events,
terminal pagination, retained failures and the negative price control.
Full fmt, workspace Clippy and all workspace/all-feature tests pass: 1,230
tests, with DB tests isolated in `scout_test`. Production adapters and live
binary are unchanged. The verified 26-closure state package predates this
48-closure checkpoint; no newer full restore has been performed.
