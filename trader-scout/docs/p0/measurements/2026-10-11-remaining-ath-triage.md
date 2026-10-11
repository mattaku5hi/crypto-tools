# Remaining ATH gaps: saved price evidence and holder gates

## Current inventory — 19:30:49 UTC

A fresh read-only database join finds **249 unknown FDV rows**, including
three absent ATH rows, in the frozen 679-address cohort. It matches the
250-row checkpoint below except for Base 67d5, whose independently validated
provider-window value was written at 18:38 UTC. No other unknown row was
removed or treated as zero. There are five Base rows with known holders at
least 100, 86 rows with unknown holders, and 158 below 100; unknown holders
still do not pass the runner gate.

The current private [CSV gap register](../../../.scout-data/diagnostics/ath-remaining-triage-current-2026-10-11/remaining-249.csv)
and [JSON evidence inventory](../../../.scout-data/diagnostics/ath-remaining-triage-current-2026-10-11/remaining-249.json)
include each token's creator, holder status, saved price evidence, blocker,
and next required proof. Unknown FDV serializes as JSON null or an empty CSV
field, rather than the database's internal `codex-missing` sentinel. Holder
and ATH status are refreshed from the database; cached price evidence retains
its separate 18:18 UTC inventory timestamp. The JSON SHA-256 is
`cb24e70420d4f5afc60a6b10d34c11eb487df4cd3d32c30f1a38fdc78209a1ea`;
CSV SHA-256 is
`0be7d571d9644d1727fbbcf8f0e736c1dd5200efbb830818a1d7cb166021dbd4`.
Manifest: `.scout-data/diagnostics/ath-remaining-triage-current-2026-10-11/manifest.json`.
No provider requests or database writes were made.

Independent offline review of the five known-holder Base candidates finds
no additional promotion under the current price policy. The separate
[boundary-price audit](2026-10-11-base-prior10-boundary-price-recheck.md)
preserves higher known execution or pool-state values, unmatched peak days,
and unsupported quote routes. A lower matching provider price must not erase
that evidence. Current snapshot, bundle, and exports are the verified
18:39/18:42 checkpoint described in the
[progress report](2026-10-11-ath-completion-progress.md). Next task: B8.4.

## Historical checkpoint — 18:18:46 UTC

At 18:18:46 UTC, a bounded read-only join of the frozen 679-address cohort
still finds 250 unknown FDV rows, including three absent ATH rows. This
inventory classifies saved evidence; it does not promote prices to FDV,
replace missing values with zero, or establish annual/lifetime completeness.
The snapshot and transferable package remain at the separately recorded
17:51/18:05 checkpoints.

| Network | Holder metadata | Saved price evidence | Unknown FDV rows |
|---|---|---|---:|
| Base | At least 100 | Nonempty candle window | 6 |
| Base | Unknown | Nonempty candle window | 5 |
| Base | Unknown | Empty candle window | 48 |
| Base | Below 100 | No saved candle window/extremum | 48 |
| Robinhood | Unknown | Empty candle window | 30 |
| Robinhood | Below 100 | Nonempty candle window | 1 |
| Robinhood | Below 100 | Empty candle window | 1 |
| Robinhood | Below 100 | No saved candle window/extremum | 15 |
| Solana | Unknown | Provider price extremum | 3 |
| Solana | Below 100 | Provider price extremum | 93 |
| Total | | | 250 |

Six Base rows have known holders passing the threshold; 86 rows have
unknown holders; 158 have known holders below 100. Unknown holders do not
pass the gate. The 92 potentially relevant gaps therefore retain the earlier
network split: Base 59, Robinhood 30, Solana 3. Other eligibility, liquidity,
FDV and plausibility gates still apply. The four new Solana records reduced
the below-100 group from 162 to 158; they did not change runner membership.

The six Base contracts are `0x00b0a07ec58d87dc09606e3166e1312f679e3b07`,
`0x3607f8f1d16c5ae717b61d28eec1d624df763b07`,
`0x5b2a3ce2109b8d09c18b02a18a0d7fd886f8cb07`,
`0x67d5ab989378bfb2070954989a2f09ca6b19ab07`,
`0x92d95883e79a695ca4afa40d4885b4ef4ac38b07`, and
`0xfec3a80626377bce24c399ef9f4e124afef6cb07`.
Their saved Birdeye windows are nonempty, but independent price/pool-scope
checks remain unresolved. In particular, a contradictory GeckoTerminal
maximum cannot be averaged with Birdeye; a separately corroborated source
window requires its own proof and provenance. An empty pool history must be
distinguished from a pool with no observed swaps in the audited interval.
See [the price cross-check](2026-10-11-remaining-ath-crosscheck.md).

Robinhood's empty windows require actual protocol/pool discovery and price
evidence; an empty configured-v4 scan alone does not exclude other markets.
For the three higher-priority Solana rows, historical supply is available,
but price corroboration and/or a proven maximum of price times supply over
the stated source scope remain necessary. A 43-row JC chart yields a precise
returned-row maximum, while gaps leave the wider provider-window maximum
unknown: [supply/window audit](2026-10-11-solana-runner-potential-supply-window-audit.md).

Private inventory and manifest:
`.scout-data/diagnostics/ath-remaining-triage-2026-10-11/`.
The 250-row JSON SHA-256 is
`a1074b0facd29c6949d219c283432f5c1701726ec45bb3c7bb2f2de8d85ecda5`;
the cohort hash is
`3f86e65375e09470f33e9d261d2f3355fa1967753c0709fc1020c9cd69ac4f77`.
No provider calls or database writes were made for this inventory. Next task:
B8.4, with the six known-holder Base rows checked first.
