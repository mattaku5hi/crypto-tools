# B8.4 historical ATH closures — 2026-10-13

Seventy-eight guarded writes extend the immutable 679-token diagnostic cohort from
434 numeric / 245 unknown to **512 numeric / 167 unknown** (Oct 14 Moscow). This is a
candidate-cohort checkpoint, not a coverage percentage for all annual network
launches. Numeric records retain source-specific price kinds and historical
scope; annual all-market ATH is still incomplete.

| Network | Numeric | Unknown |
|---|---:|---:|
| Solana | 78 | 32 |
| BSC | 13 | 0 |
| Base | 403 | 100 |
| Robinhood | 18 | 35 |

## Guarded closures

| Network / token prefix | Stored FDV, USD | Evidence and limits |
|---|---:|---|
| SOL `CCWdy8KN…` | 3.00 | Fresh exact-token primary Codex observation. |
| SOL `EojcZx3r…` | 2,024.00 | Fresh exact-token primary Codex observation. |
| SOL `H6xsCh2s…` | 2,070.00 | Fresh exact-token primary Codex observation. |
| SOL `6HfzZY1f…` | 68,044.67 | ST maximum and exact-mint GT daily high agree within 0.13%; complete mint/burn history proves peak supply equals birth supply. |
| SOL `6L9EyzaF…` | 68,048.89 | ST/GT same-day highs agree within 0.09%; complete history likewise determines the provider-window maximum FDV cents. |
| SOL `BRkf1C38…` | 127,031.38 | ST maximum matched by time-bracketing legacy PumpSwap reserve states and dated SOL/USD; 764 matching history transactions prove birth supply at the maximum. |
| SOL `Avx71ncf…` | 73,720.03 | ST maximum matched by pre/post PumpSwap states and dated SOL/USD; complete birth-to-peak history and a one-raw-unit burn produce identical upper/lower FDV cent floors. |
| SOL `88pqbGiX…` | 19,212.49 | ST maximum matched by fully decoded historical cashback-layout PumpSwap states and dated SOL/USD; complete 191-transaction birth-to-peak window proves the issued 2B supply. |
| SOL `8ihYkBBP…` | 104,354.67 | Legacy PumpSwap mark/FX validation and complete 838-transaction mint history; conservative maximum FDV lower bound with less than one cent uncertainty. |
| SOL `DwzNzHsH…` | 14,303.09 | Native-SOL Mayhem curve mark, all three adjacent states, dated FX and complete 17-transaction supply window; actual 2B supply. |
| SOL `CemXM4du…` | 15,670.48 | Same curve-mark model over all fourteen adjacent states and a complete 25-transaction supply window. |
| SOL `AkKQKnhP…` | 56,436.77 | Same curve-mark model over all ten adjacent states and a complete 181-transaction supply window. |
| Base `0x3607f8f1…` | 38,897,428.63 | GT daily high corroborated by a same-day active V4 marginal state and dated USDC/USD at both FX endpoints; fixed 100B supply. |
| Base `0xe46504da…` | 112.60 | One captured target-pool state composed through an earlier same-transaction quote/ZORA state, two active ZORA/WETH parents and dated ETH/USD minute close. |
| RH `0x4016fa67…` | 20,355.54 | Maximum over six pool executions valued through contemporaneous USDG/AAPL trades and the closed UTC-day USDG/USD reference. |
| RH `0x01193b12…` | 10,063.53 | Maximum over three post-creation pool executions, each valued with its own Coinbase ETH/USD minute close. |
| RH `0x3d5733d2…` | 32,486.30 | All 60 post-creation executions individually valued with dated ETH/USD minute closes. |
| RH `0xcc799821…` | 12,262.65 | Same reference model over all ten post-creation executions. |
| RH `0xd34b6aa3…` | 34,760.92 | Same reference model over all ten post-creation executions. |
| RH `0x48c01e93…` | 61,433.95 | All fifteen post-creation executions weighted with successful time-bracketing USDG/AAPL trades and dated USDG/USD. |
| RH `0xaa539cf7…` | 10,373.04 | All three post-creation executions weighted with their own ETH/USD minute closes; supply observed at the last event and independently proven unchanged from birth. |
| RH `0xc3a0e1a6…` | 10,211.69 | Same reference model over both post-creation executions. |
| RH `0x1e6489ef…` | 20,203.70 | All three executions weighted with preceding USDG/NVDA trades and dated USDG/USD; the winning quote leg precedes the target leg in the same receipt. |
| RH `0x52df349d…` | 20,295.18 | All three executions weighted with the block-pinned AMD/USD oracle round; historical UI multiplier is one, and adjacent-round bounds are within 0.71%. |
| RH `0x60d63f32…` | 30,007.29 | All 37 executions priced through the exact quote-contract/USDG market and dated USDG/USD. |
| RH `0x9aadb292…` | 20,278.89 | All six executions priced through actual quote/USDG trades; selected 600-second exception, 0.037% neighbor spread, stale oracle rejected. |

Three valid Codex refresh requests covered 96 SOL, 102 Base and 47 RH unknown
keys. They found the three SOL observations above; Base/RH returned no new
numeric ATH. Three earlier requests were rejected because a manual query put
`includeScams` outside `filters`. All six attempts were reserved before HTTP;
the shared counter fell from 527 to 521, with the 450 daemon reserve intact.
No counter was reset and no automatic retry was used.

The two new ST records have verified issuance of 2B units, rather than an
assumed standard Pump supply. Signature history reaches birth; successful full
transaction history has the same signature set and a terminal page. Supply
instructions, metadata discriminators, birth-time mint-authority revocation
and the finalized supply snapshot were independently rechecked from raw data.
Burns occurred after both maximum-price timestamps. Thus the price-times-peak
supply and price-times-birth upper-bound cent floors agree. They support a
maximum within the unknown ST recording window, not lifetime market coverage.

The `BQJF…` control has exact peak-time supply but a larger birth-supply upper
bound: 834,066 versus 1,621,320 cents. Its maximum-price-time FDV cannot be
declared the maximum FDV over history. It remains unpromoted; the independently
matched pool/day price is retained separately.

The Base `3607` case compares marginal state with a provider high. The same Swap's
execution-average price is about 7.2 times higher and is preserved as a
different price kind. All 15 May-4 raw swaps in the four observed USDC pools
were checked, including the one-sided state with positive liquidity; no higher
same-day marginal mark was omitted. The GT empty-pool history and limited
market coverage remain explicit. This state price is not wallet acquisition
cost or PnL.

The first two RH values are **reference-model FDVs**. All nine enumerated events were
priced before selecting a maximum. Their FX sensitivity is retained: about
$20,351.47–$20,358.21 for AAPL and $10,052.29–$10,064.06 for WETH. Neither is
an exact second-level fiat execution or wallet-net cost. The AAPL route uses
successful before/after market trades, not a stale equity oracle or assumed
USDG parity. Authoritative raw-header UTC times correct the earlier draft's
date rendering. The previously unbracketed `0x48c01e93…` observation was
subsequently recovered with successful actual-market brackets; its earlier
negative evidence is retained.

The three additional RH models cover all 80 post-creation events in their
initial pools through block 87,808,917. Every event was converted before maximum
selection; root verification rechecked raw response hashes, all 37 block
headers, receipt identities, asset decimals, supply and mint/burn evidence.
Selected FX sensitivity bands are $32,476.89–$32,515.49,
$12,261.16–$12,265.04 and $34,757.51–$34,793.95. These are reference-model
execution maxima with the same partial market scope, rather than provider
price highs or wallet-net acquisition costs.

The Base `e465` reference composes one target-pool event through a preceding
same-transaction quote/ZORA state and two end-of-event-block ZORA/WETH parent
states. Both parents have positive liquidity and differ by 0.32%; dated
ETH/USD minute sensitivity is retained. The old incorrect scan metadata and
failed quote-initialization response are excluded. A changed old receipt file
failed its stored hash check and was excluded; one new bounded receipt capture
re-established the canonical transaction/log/header binding. No old evidence
was rewritten to conceal the integrity gap.

The SOL `BRkf` check decodes all four nearby Buy/Sell events in the official
program log context. Event reserve fields match both pool-owned vault
pre-balances; curve amounts and all ceiling-rounded fee fields reproduce
exactly. Three dated official IDLs support the selected legacy reserve model;
this is not a general rule that a missing virtual-reserve field means zero,
and no deployed-program binary was captured. Both time-bracketing reserve
states and both dated SOL/USD endpoints agree with ST within 2%. The complete
764-transaction mint history reconciles one birth mint and all 27 burns to the
finalized supply; no burn precedes or overlaps the maximum-price second.

Every write required the current zero-sentinel row to remain `codex-missing`
with the expected observation timestamp. The ST writes additionally checked
the stored provider price, timestamp, exact pool and raw-response hash. Known
positive values were preserved. Holders/liquidity were preserved on all thirty-seven
complementary-source writes; the three primary Codex updates followed existing
metadata-refresh behavior. Runner thresholds and membership rules are unchanged.
Complementary records keep `ath_at=NULL`; the primary Codex timestamps retain
their provider meaning.

## Evidence, verification and next work

Private raw responses, exact rational calculations, SQL guards and before/after
rows are retained under `.scout-data/diagnostics/`:

- `b84-current-codex-refresh-2026-10-13/validated-query/`;
- `solana-gap-closure-2026-10-13/`;
- `base-3607-spot-window-certificate-2026-10-13/`;
- `rh-ath-gap-proof-2026-10-13/capture-201717/`.
- `rh-doppler-no-v4-13-2026-10-13/capture-204732/`.
- `base-e465-manual-reference-candidate-2026-10-14/`.
- `solana-gap-closure-2026-10-14/timefiltered/`.
- `rh-followup-independent-2026-10-14/`.
- `rh-nvda-amd-guarded-promotions-2026-10-14/`.

The original frozen snapshot and previous sealed deployment archive were not
rewritten. The live checkpoint is `b84-live-progress-2026-10-13.json`; the old
deployment dump predates these forty writes and is not claimed to include them.

Golden fixtures and ADR-021 amendments 25–36 cover accepted and
pending reviewed price kinds, supply-bound negative controls, per-event FX
weighting and provenance.
Full `cargo fmt --check`, workspace/all-targets/all-features Clippy with denied
warnings, and workspace/all-features tests passed: **1,231 tests**, with DB
tests isolated in `scout_test`. The release daemon remains active without
restart; its production logic did not change in this checkpoint.

Per-network details:
[SOL price/supply checks](2026-10-13-solana-ath-gap-closure.md),
[Base price-kind checks](2026-10-13-base-ath-gap-closure.md),
[RH pool/FX checks](2026-10-13-robinhood-ath-gap-closure.md), and
[ten post-launch Base trades missing GT candles](2026-10-13-base-zora-empty-gt-candles.md).

The earlier audit's claim of 13 RH tokens without an initial V4 Initialize
was unsupported: their creation receipts contain all 13 initializations, and
the retained discovery filters omitted every one of these token addresses.
Only two births precede the old lower block bound, so the cause cannot be
reduced to that bound alone. Nine of these newly discovered pools supplied numeric reference maxima.
The remaining four have zero observed Swaps or only creation-transaction
bootstrap Swaps. A fresh broader discovery/reconciliation for all 35 current
RH unknowns finds 32 empty pools and three with creation-transaction-only
Swaps at the configured manager. This does not prove absence of other venues
or justify an ATH of zero.

**B8.4 remains active.** Next batches query timestamp-adjacent SOL pool states
and missing Base price history, while preserving
explicit gaps for empty provider history, unsupported quote conversions and
unproven market-wide coverage. No unknown value is treated as zero or promoted
just to make the counter complete.

The 88pqb provenance correction fully decodes the historical 16-byte suffix
as two cashback `u64` fields, both zero, using the dated February IDL. The
previous signed-virtual-reserve interpretation is preserved as superseded
evidence; its numeric FDV does not change. Captured event-CPI bytes now pass
through the production parser in regression tests. The 5b2Bs check strengthens
an already-known Oct 11 value and is **not** counted as a new closure.

The batch-5 follow-up closes four additional SOL records. The 8ih maximum
FDV remains bounded to less than one cent, with the conservative lower value
accepted under amendment 33. The three Mayhem values use actual 2B issuance,
rather than the provider market-cap field's approximate 1B denominator;
complete native execution, dated FX and all captured curve states corroborate
the selected mark (amendment 34). See [batch 5](2026-10-14-solana-ath-gap-closure-batch5.md).
The Base 00b0 execution maximum has a 90% fee and differs from the marginal
state maximum; its independent follow-up remains unpromoted.

[Batch 6](2026-10-14-solana-ath-gap-closure-batch6.md) adds seven selected
Mayhem curve-mark records, with complete mint histories and separately
reconciled native fee legs. All two negative price controls and the AMM
pilot remain unpromoted. The protected 26-closure package predates these
seven writes; it must not be described as including the 33-closure checkpoint.

[Batch 7](2026-10-14-solana-ath-gap-closure-batch7.md) adds seven further
curve-mark records. Its Fg proof distinguishes the raw signature cursor
interval from successful transactions inside the supply timestamp scope;
outside/failed rows and the original mismatch status remain explicit.

[Batch 8](2026-10-14-solana-ath-gap-closure-batch8.md) adds eight further
curve marks. Ezq supply verification explicitly continues the 1,000-row
signature page to 564 and zero rows, retaining 1,336 failed signatures and
matching all 229 successful transactions including birth. The 140 positive
neighboring events pass actual-minute FX and native execution validation;
the negative curve control and AMM remain unpromoted. The sealed package
still contains the earlier 26-closure checkpoint, not these 48 closures.

[Batch 9](2026-10-14-solana-ath-gap-closure-batch9.md) adds seven further
curve marks. Fresh exact-key selection and scoped signature reconciliation
avoid both already-known candidates and post-peak successful effects; all
prior requests remain in cumulative budget accounting. The three AMM inputs
have separate price diagnostics and capped nonterminal supply histories.

[Doy legacy PumpSwap follow-up](2026-10-14-solana-doy-legacy-neighborhood.md)
adds one exact-cent record, with every captured pre/post state checked and a
complete 921-transaction supply window. The burn occurs before the peak;
maximum-FDV endpoint floors still agree. Other AMM price passes have their
supply evidence recorded separately and are not silently treated as exact.

## Conservative lower bounds (Oct 14)

Twelve additional SOL observations were guarded-written with the explicit
`solana-tracker-window-supply-lower-bound-2pct-pool-fx-validated-v1` source.
These are lower reference estimates within the owner's accepted 2% combined
price/supply/rounding uncertainty, not exact maximum FDV. See
[case table and limits](2026-10-14-solana-conservative-fdv-bounds.md) and
ADR-021 amendment 40. Holders/liquidity are preserved; `ath_at=NULL`. Two
roughly 44% supply-bound controls remain unknown. Total closures since the
immutable 434 baseline are 68: SOL54/Base2/RH12. Workspace fmt, all-target
all-feature Clippy and 1232 tests passed against the isolated test database.

## Transaction-local vault recovery (Oct14)

CtN5 and G3Hy add two explicit conservative lower estimates, $75,473.02
and $58,546.39. Exact scoped initialization/funding/buy transfers recover
missing intra-transaction pre states; G3Hy retains its intact event CPI despite
truncated logs. All318 neighborhood states participate in maxima. The69KX
control has a6.8–6.9% higher global state and remains unknown. See
[transaction-local vault evidence](2026-10-14-solana-transaction-local-vaults.md),
ADR-021 amendment41. Guarded writes and read-back passed after full
fmt/Clippy/1233tests. Current70closures are SOL56/Base2/RH12; the fixed
cohort is504numeric/175unknown. The separately dated68-closure state package
is retained; it does not contain these last two writes.

## Four more first-buy recoveries (Oct14)

D8uy/gLTR/A5Ewn/NDt3 add four explicit conservative lower estimates after
all966 terminal neighborhood pre/post states and eight canonical transaction
recaptures were checked. Full workspace fmt/Clippy/1233tests passed after
adding winning-time assertions. Atomic guarded writes preserve holder/
liquidity metadata and null ATH dates. Total74closures: SOL60/Base2/RH12;
fixedcohort508numeric/171unknown. The separate70-closure bundle predates
these last four updates. See [batch14 evidence](2026-10-14-solana-first-buy-batch14.md)
and ADR-021 amendment42. Bd556 remains rejected by its5.9% price conflict.

## Chronological FDV windows (Oct14)

EWbab/FLk/2b4 add three bounded chronological FDV references after1715 states,
684 actual scoped token transfers, direction-specific quote intervals and
six finalized birth/winner recaptures reconcile. Their combined uncertainties
are below0.12%, including cent rounding. FLk's largest FDV precedes its price
peak, so peak-price supply alone is rejected as the maximum. Full workspace
fmt/Clippy/1234tests passed before atomic guarded writes; null ATH dates and
holder/liquidity metadata are preserved. Current77closures are SOL63/Base2/RH12,
fixedcohort511numeric/168unknown. The74-closure archive predates these writes.
See [chronological FDV evidence](2026-10-14-solana-chronological-fdv-window.md),
ADR-021 amendment43. Conditional SDK-model PRE references and missing annual/
all-market coverage remain explicit; no automatic production adapter is added.

## Fht global-window lower reference (Oct14)

Fht adds$1,339,443.48 under the existing combined2% conservative policy.
All four actual vault states in the terminal window and both dated trades
reconcile; the maximum is708ms before the provider timestamp. Birth/current
raw mint checks and two finalized transaction confirmations pass. Stored
holders12 remain below50; no list-membership change is inferred. After full
fmt/Clippy/1234tests, the guarded write preserves metadata and null ATH date.
Current78closures are SOL64/Base2/RH12, fixedcohort512numeric/167unknown.
See [Fht evidence](2026-10-14-solana-fht-global-window.md), amendment44.
6SFB/7rWY remain unpromoted despite acceptable prices because47 instruction
wire variants remain unresolved; the bounded official-source search is
[documented separately](2026-10-14-pumpswap-legacy-instruction-wire-research.md).
The chronological three-case golden now verifies corrected certificate-v4
and original current-mint capture hashes; numeric values are unchanged.

Latest78-closure state package includes all current writes, exact dump-row
reconciliation and corrected capture provenance; see the [archive checkpoint](2026-10-13-local-daemon-and-year-coverage.md). Its own full restore is not claimed.

## vC95 chronological supply-weighted reference (Oct14)

After616 successful transactions, six exact supply instructions, all1661
price/FX/supply endpoints and three canonical recaptures reconcile, vC95 adds
$497,125.96 as a scoped conditional SDK-model lower reference. The upper
endpoint is$497,905.92; combined uncertainty is below0.157%. All334 selected-
pool vault marks and442 curve transfers are independently bound to raw
captures. One incomplete log stack is recovered by a unique exact event CPI;
one additional curve transfer remains an unexplained route flow. Dated SDK
argument interpretation is corrected:25-byte buys include track_volume;
52 shorter buys and appended account roles remain gaps. A retroactive audit
confirms unchanged main3 arithmetic while recording28 shorter buy variants.
See [vC95 evidence](2026-10-14-solana-vc95-chronological-fdv.md), amendment45.

Full fmt/Clippy/1234tests passed before the atomic guarded write; holder/
liquidity metadata and null ATH date are preserved. Current79closures are
SOL65/Base2/RH12; fixedcohort513numeric/166unknown. The78-closure archive above
predates this latest write. Annual/all-market ATH remains incomplete.
