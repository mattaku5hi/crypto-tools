# H4/9sEF historical supply in observed price windows — 2026-10-14

Two previously unknown FDV references have already corroborated selected-pool
price windows but wide birth/current supply bounds. Raw supply reconstruction
now resolves the denominator for all46observed PRE/POST marks.

| Case | Successful TFA set | Observed marks | Historical raw supply@6 | Lower–upper reference USD |
| --- | --- | --- | --- | --- |
| H4knW |254|12 across6swaps|1136099869009195|17846.71–17853.48|
|9sEF|391|34 across17swaps|1712502929816770|29075.31–29092.91|

Each birth has two exact MintTo instructions totaling2Btokens, with authority
revoked. Three burns occur before the entire selected window; none occurs
inside its swap transactions. Current mint supplies are lower and later,
so they are not valid substitutes. Exact requested indexes18/14, Token-2022
ownership,6decimals, no mint authority and metadata extensions18/19 are
independently checked against the saved raw account batch.

H4's terminal census is286signatures,38failed,248successful; adding six
terminal window signatures exactly covers254successful full transactions.
Its final TFA response still has a cursor. The successful set is verified by
census/window equality, not an invented empty TFA page.9sEF's census has399
signatures,25failed,374successful;17window rows bring the exact set to391.
The continuation fetched91rows and an empty terminal page, resolving all
previously missing transactions.

The initial9sEF reconciliation misreported its two birth MintTo parent
indexes as outer0. Raw bytes locate them under outer2, inner19/20. A failing
independent audit caught this before any write. New v2 artifacts correct
metadata; amounts, supply and FDV intervals stay unchanged. The golden guard
rejects the original tuple and pins the corrected raw sources.

All46reserve ratios, exact Coinbase minute FX and historical supplies are
recomputed independently. Observed maxima precede cent floor/ceiling.
Both price endpoints agree within2% with the saved Solana Tracker peak.
No execution-average price or provider-high×current-supply shortcut is used.

Source: `onchain-pool-window-supply-cex-reference-v1`. This is a conditional
vault-ratio reference in two selected provider windows. Legacy event layout
has no virtual-quote field, and current pool tails are not extrapolated as
historical zero reserves. Unknown deployed-model error, alternate venues
and lifetime/annual coverage remain unbounded by the small reference band.
ATH date stays null; holder/liquidity metadata and runner gates remain intact.

Private evidence SHA-256:

- Root all46-mark certificate: `02ad7707bac026e066c1495d992cd18805c52776e6e0b0f262757eadecea3e34`.
- Independent raw audit v2: `523ea53ab2d50702f99afca6d3cf7de2fa6480c156ff27d692971dd062f7b388`.
- H4 state envelope: `b6e0c010070df64480b5b94a0253cffae3788f8c42f2630d0d73cdb23eb88544`.
-9sEF corrected state envelope: `c0e527cac3c896cb7d34744bc1cc37fdf1604590a97a6314ba3accdd1ac962ad`.
-9sEF corrected supply reconciliation: `09018eedbe90f2b8d51957a4f4f7d17acf986734a184d0a7ce087ec057ff3ad5`.

The new RPC work used10Helius methods: eight in the two-token pilot, then
91-row and terminal-empty9sEF continuation calls. State/candle math reused
cached evidence. No new Solana Tracker, Codex or CEX requests were needed.
Next task B8.4: remaining scoped historical supply/price/model gaps.

Golden v2: `solana_tracker_h4_9sef_historical_supply_fdv_window_cases_v2.json`,
SHA-256 `a9a2f0f3a2a83b086a5eb54c71a3afe96622c563e7bd5153ab226223a8c3b086`.
The first fixture used a different source-kind label; an initial pre-write
guard rejected it without changing the database. A new version preserves the
old fixture bytes and explicitly asserts source-kind equality with the root
certificate. Corrected-parent regression and all46exact U512 marks pass.
Full workspace fmt/Clippy/1237tests passed against scout_test in57.49seconds,
with production database tests disabled and incremental compilation off.

The atomic guarded two-row write stores1784671cents for H4 and2907531cents
for9sEF, keeps dates null and preserves holder/liquidity values. The fixed
679-key cohort is now517numeric/162unknown:83new references since434
(SOL69/Base2/RH12). Annual/all-market ATH remains incomplete. The two sealed
packages79/80 predate these two writes and JC. New packages are reserved for
completed milestones/cutover, under the approved two-complete-package policy.
