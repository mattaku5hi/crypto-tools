# Base: threshold witnesses and current dev-list membership

Two unresolved Base ATH records have independently replayed marginal-price
observations above the runner threshold. These are dated pool observations,
not resolved annual or lifetime maxima. No ATH, classification, source policy,
or provider-comparison gate was changed.

| Token | Observed UTC | Conservative reference FDV |
|---|---|---:|
| `0x00b0a07ec58d87dc09606e3166e1312f679e3b07` | 2026-05-02 04:14:31 | $1,529,626.80 |
| `0x5b2a3ce2109b8d09c18b02a18a0d7fd886f8cb07` | 2026-05-02 08:57:09 | $5,213,688.74 |

Each point binds the actual Uniswap v4 PoolKey and Swap log to a successful
receipt and canonical block header. Exact integer Q96-squared marginal price,
18/6 token/USDC decimals, historical 100B supply and the saved May 2 Kraken
USDC/USD daily low reproduce the reported floors. Daily FX is a reference,
not an event-minute executable exchange rate. Positive post-Swap liquidity
does not establish trade size, economic execution price or all-market coverage.
The 5b2a transaction contains multiple pool Swaps; an individual event ratio
must not be called the wallet's execution price.

Source permits bridge mint/burn and ordinary burns. The saved transfer scan
has one creation mint and no burns through its pinned block 52,456,236;
this is a historical supply conclusion through that anchor, not immutable
supply by contract design. Holder counts 1,428/1,434 are October checkpoints,
not May 2 historical counts. Global marginal maxima still disagree with
archived provider highs by more than 2%; selecting a better-matching event
would not resolve the disagreement. Both canonical ATH rows remain unknown.

Both launch rows record Clanker creator
`0x6fd22632c88e5247c1aedc203fd3c352a2e7515e`. Read-only replay of current
runtime gates finds 30 qualifying runners, including 29 big runners, without
these two tokens. The exact creator address already belongs to all three
current Base top-runners exports (Axiom, GMGN and Basedbot, 12 members each);
their membership hashes match database delivery records. No missing dev-list
membership is demonstrated by these two unknown ATHs. The database has no
address-kind row for this creator, so EOA versus contract remains unverified;
Clanker's implemented attribution preserves its recorded creator field.

Private immutable audit: `.scout-data/diagnostics/base-threshold-independent-witness-2026-10-14/audit-v4.json`,
SHA-256 `e5ae365848c56fdde6f8d17420635ae1eed2a91922b2e3b2e2ca0c2ad56e003d`.
It retains raw-input hashes, calculations, runtime gates, export membership
and read-only database evidence. This pass used no fresh provider requests.

The frozen diagnostic cohort remains **523/679 numeric references, 156 unknown**:
SOL 21, Base 100, Robinhood 35 and BSC 0 unknown. Numeric references include
explicitly scoped or conditional values; this does not measure complete annual
ATH coverage. Next task remains **B8.4**: resolve the remaining price/supply
and coverage gaps while preserving source and model boundaries.
