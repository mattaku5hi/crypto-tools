# Solana ATH follow-up: Mayhem native fee legs, batch 6

Ten currently unknown keys were selected from the same frozen 679-token
candidate cohort. A bounded capture used 51 Helius methods and ten Coinbase
GETs; seven additional standard finalized transaction recaptures brought the
Helius total to 58 of the 70-method cap. All raw requests/responses are retained
without retries, new Solana Tracker/Codex requests, or replacement of older
captures. Bodies were bounded at 2 MiB.

Seven Mayhem curve cases pass the maximum-over-all-captured-states check at
both dated SOL/USD endpoints. Their entire ±3-second neighborhoods contain
36 TradeEvents; maximum states precede the provider timestamp by one or two
seconds. All event-CPI payloads match actual Pump execution context. Complete
birth-to-peak TFA histories match finalized signature-index sets plus explicit
birth. Each mint issued 2B six-decimal tokens, revoked mint authority at birth
and had no burn through the price peak; current mint extensions are metadata
only, with no close authority or supply-scaling feature.

| Mint prefix | History transactions | Stored FDV, USD |
|---|---:|---:|
| `3JK9Yd82…` | 2 | 5,005.86 |
| `5N9Zfj9t…` | 2 | 4,870.04 |
| `D4ahEpG9…` | 1 | 5,225.41 |
| `5WrHHbJw…` | 2 | 7,034.46 |
| `9Lk9hLDc…` | 4 | 4,991.52 |
| `HorJc4Xz…` | 2 | 5,465.71 |
| `EdQwQPEj…` | 5 | 6,427.11 |

Most selected trades have nonzero protocol/creator fees. System transfers are
scoped to the actual Pump buy invocation, including a nested router case,
rather than conflating every transaction-wide native movement. Curve-account
transfers equal event SOL amounts; fee-recipient transfers and ceiling-rounded
fee fields reconcile. The `5N9` creator vault has an additional 890,880-lamport
transfer, retained separately from its creator fee. Fee-payer balance changes
include other funding/transaction flows and are not used as the trade amount.

All seven writes were guarded atomically against exact current zero-sentinel
rows and saved provider price/timestamp/mint/raw hashes. Known positive rows
were not overwritten; holders/liquidity were preserved. The source remains
`solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1`, with
`ath_at=NULL` and unknown provider recording window. Provider market caps with
a nominal 1B denominator were not copied into the FDV ledger. This is a
curve-mark reference, not execution average or a proven market-wide annual ATH.

Two negative controls (`5ioCXEqE…`, `EEnuxT5Q…`) have earlier matching states,
but their **global** captured maxima differ by about 16.7% and 6.6%. They remain
unknown; choosing a convenient lower matching observation would hide that
conflict. The PumpSwap candidate `9Jek81…` also remains pending. Its pilot
matched several pre-swap marks, but pool-owned vault post-states show a larger
nearby maximum; the curve formula was never applied to its AMM account.

Root independently rechecked price, supply and fee/native evidence:

- `root-independent-curve-price-proof-before3s.json`, SHA-256
  `f3b4ad42d145575ccdb25c425333a69a24908e866b2b86a1858b216493326b2b`;
- `root-independent-supply-proof.json`, SHA-256
  `4bde343662cc8e4693b9fff64a3ee72bb4718bf39d9476713f724909ab03a31a`;
- `root-independent-native-proof.json`, SHA-256
  `5b1f92f85e35462704a2671f6657549fb9598a41c2213497f48ed1396a93ceee`;
- the separate worker supply/native audit, SHA-256
  `db9b6230f5b41ad3608c9411f2d6867bbcaa4971c53da35839befb20dc840cf6`.

These are under `.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch6/`.
The exploratory stricter one-second comparison is retained separately; the
final proof explicitly records the observed -1/-2-second offsets within the
same captured window. ADR-021 amendment 35 and a captured-byte golden cover
these cases and the negative controls. Full fmt, workspace Clippy and all
workspace/all-features tests passed: 1,230 tests, with DB checks isolated in
`scout_test`. Production adapters and runner thresholds are unchanged.
