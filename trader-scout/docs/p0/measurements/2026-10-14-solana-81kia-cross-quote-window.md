# 81kia: historical supply and cross-quote observed FDV

The guarded write stores **$91,170.56** for
`81kiaJSjaU9w6FVwRVT2K5TP1EhDghAXCm6Y4zmRpump`. The conditional reference
interval is **$91,170.56–$91,887.84**, covering PRE/POST transaction states
and two virtual-reserve interpretations. Source remains the existing
`onchain-selected-pool-cross-quote-observed-fdv-window-vq-model-cex-v1`.
This is a selected-pool observation window, not complete annual or lifetime ATH.

The quote asset is volatile87ea, priced through its CF3/WSOL pool, not a dollar
peg. Terminal histories of CF3 and both vaults contain the same two successful
transactions at22:45:32/22:45:54 UTC. Previous POST balances equal next PRE:
30845041082848 raw87ea and553584674096 raw WSOL. They bracket the target
transaction at22:45:46 and provider peak at22:45:47.033. Both canonical boundary
transactions match archived TFA rows. No future POST price is carried backward.

Full raw event parsing uses the October8 official IDL, signed-i128 schema
fields and consumed lengths. Offsets455/168 refer to Buy/Boost event bytes
including their eight-byte discriminators, not stripped bodies. The deployed
revision remains unverified. Physical state continuity and the primary retained
fee model select the PRE interpretation; event-as-POST remains a sensitivity.
Net retained fee is protocol minus externally paid buyback plus creator fee.
The prior boundary exactly validates its13512-raw adjustment; the next has
3711003129 raw retained. Target Buy/Boost fees are zero. The event is
BoostBuyAndBurn; it is not wallet acquisition or a demonstrated execution price.

All127 captured successful mint transactions are classified and ordered.
Fresh finalized census248 =83 successful +165 failed; exclusive birth/target
anchors produce85 full successes through target with exact set equality.
Birth issues1e15 raw and revokes mint authority;30 Burns total33588763050229
raw, reconciling to later current supply966411236949771. The target burn
809659974884 raw occurs before its BuyEvent CPI. PRE supply968626467018737
and POST967816807043853 are priced separately. Current supply is not substituted.

Four marks(two transaction states × two models), with their own-minute Kraken
SOL/USD110.20–110.35, give eight price endpoints inside2%. The lower rule is
minimum of each model's maximum lower FDV; the upper is maximum upper FDV.
The0.78060058% interval does not bound unknown market or deployed-model error.
ATH date remains null; unknown holders and liquidity remain unknown. The
reference is below the runner threshold and adds no dev-list member.

Actual new request count is **14 Helius methods**:6 first quote/canonical,
6 extra parallel calls(including4 repeated vault requests),2 mint/canonical.
The parallel duplicate is recorded in a versioned correction. No new Codex,
Solana Tracker or FX-provider requests were used. The original provider raw
body exists in the keyed-probe capture; a null pointer in the later joined
report is not absence of the raw body. Its SHA matches the DB observation.

Both new captured regressions pass with full workspace fmt, all-target/all-feature
Clippy and all-feature tests: **1,244 tests**, **57.06 seconds**. DB tests used
`scout_test`; production DB was not used by tests. Incremental compilation was
disabled. The one-row transaction guards source/certificate/fixture equality,
the original provider observation and current missing-row version.

The fee audit also corrects JC's diagnostic upper bound by one cent:
**$227,079.92–$227,224.07**. Stored lower remains22707992 cents; no JC database
write is needed. Consistent POST-field sensitivity adjusts all four CF3 PRE
pairings. A separate corrected golden preserves the historical original fixture.

Frozen679-key checkpoint: **525 numeric /154 unknown**,91 new scoped references
since434 (SOL77/Base2/RH12). Remaining unknown: SOL19, Base100, Robinhood35,
BSC0. Numeric counts do not imply complete annual ATH coverage. The sealed
80-value package predates11 writes; no new full restore/deployment is claimed.

| Evidence | SHA-256 |
|---|---|
| 81kia root certificate | `5d5de769d005faac30d0e46a80145c3a540064984a7a1b22da30d3eab8524ab0` |
| 81kia independent supply | `5a51d435dfde08dfe3a2734d50590c164a2f1a9e89b14eb9f47bfc95ea36cf01` |
| 81kia independent VQ envelope | `a502aa22830a6f19870031ba9cb0e89440c3069904f5748f099733e4054697ee` |
| 81kia golden fixture | `d5d0ab1ba8bdd66c0a173916dc2bae23d93b4cc755c688ac1691ef9abf4a4814` |
| JC corrected root certificate | `dd7dd7fb01560ec2510dcc8d2bb03e9b2f56feca1eacb424fbdde1cc46282571` |
| JC corrected golden fixture | `d7304f00319922db42071321b84546525cf0fb67ed9be1738c626d15cb378347` |

Private capture/certificate/guarded write:
`.scout-data/diagnostics/solana-81kia-mint-canonical-phase-2026-10-14/`.
JC correction: `.scout-data/diagnostics/solana-jc-vq-buyback-correction-2026-10-14/`.
Full checks: `.scout-data/checks/2026-10-14/b84-gap-closure-round27-validation-manifest.json`.
ADR-021 amendment54; next task **B8.4**, remaining historical price/supply gaps.
