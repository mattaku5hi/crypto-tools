# Owner-accepted Solana references and historical fill scope — 2026-10-14

The owner ends the targeted fill for the remaining 100 Base keys and accepts
provider/on-chain price disagreements for the remaining 15 Solana keys. After
independent raw-input checks, a captured regression and the complete workspace
checks, all 15 references were written in one guarded transaction. The frozen
679-key cohort now has **544 numeric / 135 unknown** values: SOL 110/0,
BSC 13/0, Base 403/100 and Robinhood 18/35. The 100 Base gaps are parked;
normal facts ingestion and future observations continue. None is changed to zero.

These references use the retained exact Solana Tracker peak price multiplied
by checked monotone current-to-birth supply bounds. They carry the distinct
source `solana-tracker-owner-accepted-discordance-supply-bound-v1`, a null ATH
date and provider-recording-window-unknown scope. No annual, lifetime,
all-market or on-chain price agreement is asserted. Holders/liquidity are
preserved; runner/dev gates are unchanged. See ADR-021 amendment 57.

Nine supply intervals are within 2%; six are wider. Their full widths are
retained rather than presented as precise event-time supply. Even every upper
reference is below the unchanged $500k runner threshold; accepting these
15 values adds no token or developer to top-runners.

| Mint prefix | Stored lower FDV | Upper reference ceiling | Supply width / birth, truncated |
|---|---:|---:|---:|
| `27ES77p6…` | $48,931.17 | $49,544.70 | 1.23% |
| `48vcicPG…` | $125,954.78 | $132,428.79 | 4.88% |
| `4CX5a5gn…` | $64,810.30 | $64,830.91 | 0.03% |
| `4qo2GNND…` | $87,031.28 | $87,039.81 | 0.00% |
| `5ioCXEqE…` | $12,800.19 | $15,580.50 | 17.84% |
| `69KXHdb7…` | $79,632.21 | $79,632.38 | 0.00% |
| `6nV9kzU4…` | $90,726.28 | $90,726.33 | 0.00% |
| `9Jek81nw…` | $60,201.28 | $60,215.41 | 0.02% |
| `AiEpwMiG…` | $3,193.02 | $6,317.07 | 49.45% |
| `ApRm3VBA…` | $6,039.97 | $6,818.83 | 11.42% |
| `BNrJcMqh…` | $109,464.78 | $109,487.75 | 0.02% |
| `BQfp97ry…` | $50,422.59 | $58,893.32 | 14.38% |
| `Bd556n65…` | $56,402.51 | $56,465.26 | 0.11% |
| `EEnuxT5Q…` | $4,117.00 | $4,852.73 | 15.16% |
| `F2RwcxcL…` | $37,140.31 | $37,142.56 | 0.00% |

The independent audit checks 15 raw birth transactions, request signatures,
MintTo sums and mint-authority revocation; 15 raw mint accounts with supported
program owners, decimals, authority options and metadata-only extensions; and
all 15 original provider response hashes. The regression reuses the real ST
parser, checks exact monetary floor/ceil arithmetic and current account bytes,
invalid supply controls, the wide-interval upper guard and existing holder/
liquidity gates. Production adapter behavior remains unchanged.

Workspace validation: **1248 tests**, `cargo fmt --check`, and
`cargo clippy --workspace --all-targets --all-features -- -D warnings` passed
in isolated `scout_test` in 58.86 seconds. No production tests or new provider
requests were used for this Solana acceptance. SQL guards match the old
missing-row source/value/time/holder/liquidity and the exact saved ST price,
timestamp, pool and response hash. All 15 after-rows match, metadata preserved.

Private proof root:
`.scout-data/diagnostics/solana15-owner-discordance-acceptance-2026-10-14/`:
`root-final-certificate-v1.json` SHA
`b722441bd572ef8cff1d78559ca4df599239e5448f38b9ad07264cb7f3387874`,
`raw-supply-independent-audit-v2.json`, `accepted-values.csv`, and
`atomic-fifteen-row-write-v1/manifest.json`.

Changed: `crates/scout-devtracker/src/birdeye.rs` (tests only),
`crates/scout-devtracker/tests/fixtures/solana_tracker_owner_accepted_discordance_case.json`,
ADR-021 amendment 57, tickets and this record. No daemon restart was needed.

## Runner and Robinhood denominators

The separate live annual runner count at this session was 6,799 unique
network/token keys, including 4,006 at $1m or above. SOL 3,653 (2,257 big),
BSC 1,191 (738), Base 711 (354), Robinhood 1,244 (657). Counts apply the current
$500k–$10bn FDV, 100-holder and configured liquidity gates, annual token-birth
scope and the same shared-contract signer exclusions as `for_each_dev`.
They use recorded values and do not prove complete annual market histories.
The latest delivered top-runner developer lists contained SOL 42 and 12 each
for the other networks (78 total); the 15 accepted values do not alter them.

Robinhood's 35 remaining cases belong to a 53-token additional-check cohort,
not the full network. At 2026-10-14 19:54 UTC, the live DB had 1,296,096 RH
launches, 183,706 joined ATH-result rows and 123,057 positive non-missing
numeric references. The latter are not all independently verified annual ATHs.

The owner also requested an alternative-launchpad check. See
[venue discovery](2026-10-14-robinhood-alternative-launchpads.md) and the
[35-token v2/v3/Goo scan](2026-10-14-rh35-alternative-factory-scan.md).
Six successful token-position filters found no pool-creation logs for these
35 addresses at the three queried factories. One v3 code lookup timed out;
4.fun launch ABI and Pleiades remain unverified. These limits do not establish
zero ATH or complete venue coverage. Further RH work remains under B8.4;
B9.1 actual GitHub release execution and the stopped-source VPS cutover remain
separate pending tasks.
