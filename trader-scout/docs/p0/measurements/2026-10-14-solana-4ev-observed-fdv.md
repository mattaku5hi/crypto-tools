# 4ev observed reserve FDV reference — 2026-10-14

The 4evPrwwv token's saved price is corroborated by its selected PumpSwap
pool, but birth/current supply differs by almost44%. A terminal transaction
replay resolves historical supply: two birth mints issue2B tokens; authority
is revoked; one in-window burn removes878228625954594raw. Supply at the
peak and through+3s is1121771374045406raw at6decimals, greater than today's
1121758849983755raw. The current value is not used for historical pricing.

The captured history has111 successful transactions and20 failed signatures.
There are96 curve trade events and11 selected-pool swaps. All96 token amounts
have raw Token-2022 transfer matches. Buys have scoped System inputs, while
sell evidence is transaction-level. One bundled transaction balances two
trades only in aggregate; this does not prove its individual sale output.
The96 SDK-model PRE intervals are retained as unproven diagnostics. They
are not silently converted into observations or serialized as zero.

The new source evaluates exactly119 observed reserve marks: one Create,
96 curve POST and22 actual selected-pool PRE/POST vault states. Every mark
uses canonical instruction-ordered supply and exact raw Coinbase minute FX.
Five new public candle GETs plus two cached event-minute candles cover all15
minutes; all responses areHTTP200, with no retries or redirects. The raw
inventory's price-times-raw-supply field needs division by10^6 for normalized
FDV. This is checked independently rather than inferred from its field name.

The global observed-reference FDV is$7,197.80 lower floor to$7,205.30 upper
ceiling. Both winners are the pool POST state at2026-04-19 15:00:58UTC, one
second before the provider's peak second. Both global observed price bounds
agree within2% with the saved price. Reference FX/cent interval width is
below0.105%; it does not bound unknown PRE/market/model errors. Multiplying
the provider price by exact historical peak supply gives a different point
reference,$7,202.66 floored, and is not substituted for the observed maximum.

Source:
`onchain-mint-history-observed-curve-post-selected-pool-fdv-window-cex-v1`.
Scope is birth through provider-peak+3s for observed curve marks and one pool,
not lifetime/all-market ATH. Short buy argument variants,55 extra curve
account roles, wrapper semantics and40 unparsed current-pool bytes remain
explicit gaps. No historical virtual-quote reserve is inferred from today's
snapshot. No generic production adapter or runner thresholds change.

Private evidence SHA-256:

- Root observed certificate: `cb9f46719c24fa0190159b38da678485ffd32edd4d6422abc670ffe3161cd254`.
- Observed replay: `ec357d365bf7123aedb4bebfda79a4fb7ae03e0c563c8e07ed12233a965a1929`.
- Raw event/vault/flow audit: `c9e2f88f65fbb31bf7349dd8d6fce149b8075e887c8d541279ca030ef7ab33aa`.
- Root raw supply binding: `d0e8afeb94e1cc95b238ebed5823035202ea86a6ec7b0a05a50df8ec28c4d7ef`.
- Exact15-candle audit: `48901d0120580a0fe2b8326733d02f526a049859b7dd17e870a8f064892bbef5`.
- Two canonical confirmations: `cbd88a9b8d431a9131597c1b2dce10581d2e65b68a273d86627e2c847dd43c0b`.

The price/supply pipeline used six Helius methods (two pilot, two continuation,
two standard confirmations), five public Coinbase requests, and no new
Solana Tracker/Codex calls. Canonical birth and winner responses exactly match
the full captured transaction objects. Next task B8.4: remaining historical
price/supply/schema gaps, with annual coverage still incomplete.

Golden: `solana_tracker_4ev_observed_curve_post_pool_fdv_window_case.json`,
SHA-256 `2aea861177d4712b14cba2729d5730c2ed6a8772d28fed44e5c8a13b72be749e`.
A separate exact119-mark regression verifies native/raw-supply normalization,
historical supply, all FX bounds, source hashes and typed PRE/bundled-sale
gaps. Full workspace fmt, all-target/all-feature Clippy and1235tests passed
in48.12seconds against isolated scout_test. The atomic guarded write then
stored719780cents, with holder/liquidity unchanged and null ATH date.
Fixed679-key cohort is514numeric/165unknown:80new references since434
(SOL66/Base2/RH12). Annual/all-market ATH remains incomplete.

Changed files: birdeye.rs captured-case test and new observed golden,
ADR-021 amendment46, this measurement and central status. No production
binary/adapter or runner gate change.
