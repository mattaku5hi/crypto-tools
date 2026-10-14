# ADR-021: Dev tracker — facts, storage and derivation

Status: Accepted (2026-10-07)
Date: 2026-10-07
Builds on: `docs/DEV-TRACKER.md` (owner decisions 2026-10-06), B0 measurements
(`docs/p0/measurements/2026-10-06-dev-tracker-volumes.md` and its addenda), ADR-020 (EVM events),
ADR-012/015 (pump.fun programs).

## Decision

1. **Facts, not verdicts.** PostgreSQL stores immutable chain facts and refreshable observations;
   the three categories (`top-runners`, `top-migr`, `win-streak`) are derived on every cycle from
   those facts and the current TOML config. Changing a threshold never requires a rescan.
2. **Fact tables** (crate `scout-devdb`, schema migrations embedded and applied at start):
   - `launches(chain, token, launchpad, creator, created_block, created_at, tx_hash, source)` —
     one row per token; `creator` is the launchpad's own creator field (Flap `creator`, Pons
     `deployer`, four.meme `creator`, Zora `payoutRecipient`, Clanker `tokenAdmin`, pump.fun
     creator account).
   - `migrations(chain, token, launchpad, migrated_block, migrated_at, tx_hash, pool, source)` —
     the launchpad's own graduation event (Flap `LaunchedToDEX`, Pons `PoolGraduated`, pump.fun
     `Migrate`, four.meme graduation once identified). Launchpads without a curve (Zora, Clanker)
     have no rows here.
   - `ath(chain, token, ath_fdv_usd, ath_at, source, observed_at)` — refreshable observation
     (Codex `extrema.athFdv`, ADR B0.3); the latest observation per token wins.
   - `cursors(source, position, updated_at)` — per ingestion source (chain + launchpad + event),
     the last fully ingested block / slot / signature; ingestion is idempotent (upserts keyed by
     the natural key), so a replay after a crash is harmless.
   - `deliveries(category, format, content_hash, delivered_at)` — the last delivered export per
     category and terminal format (B7: send only on change).
3. **Derivation (B4)** is pure Rust over loaded facts (`DevFacts` per (chain, creator)):
   resolved launches = migrated + not migrated and older than `pending_window`; migration rate =
   migrated / resolved with both raw counts kept; current streak = consecutive migrated launches
   from the newest resolved launch backwards (pending launches skipped); runners = launches with
   `ath_fdv_usd` ≥ threshold. Launchpads without migration skip the migration-rate rules
   (owner: Zora/Clanker → `top-runners` only).
4. **Sources per chain** (B0): EVM launch/migration events by `eth_getLogs` polling on the keyed
   provider (Alchemy PAYG, fallback dRPC — ADR-020 amendments, A7), Solana pump.fun creates and
   migrations from on-chain data (standard RPC methods so Ankr / the public RPC can serve as
   fallback, A8), ATH from Codex in batches of 200. Codex counts are hints only (≈ 70 % migration
   coverage, no Flap/Pons).
5. **Tests** run against a real PostgreSQL when `SCOUT_TEST_DATABASE_URL` is set (local container);
   without it the database tests skip (CI stays offline). The derivation is tested without a
   database.

## Consequences

- Category membership is reproducible from the database at any time; history of membership is
  not stored as a fact (deliveries keep the content hash of what was sent).
- A launchpad whose creator field cannot be trusted (e.g. a factory that is the `msg.sender` for
  everyone) must be mapped to the real creator before its launches count; each launchpad's
  creator rule is documented next to its decoder.

## Amendment 1 (2026-10-07) — the dev behind a launch

Live (one day of EVM launches, 33k rows): most of the busiest "creators" on BSC and Robinhood are
contracts. Sampling 8 launch signers per contract shows two kinds: shared intermediaries (a different
`tx.from` per launch: Flap VaultPortal `0x90497450…`, launcher services `0xf2e2f0ae…` (BSC),
`0x0e1651ae…` (Robinhood, filling Pons' `originalDeployer`)) and single-operator bots (one signer for
every launch, e.g. `0x563b2841…` signed by `0x7485fbfd…` on both chains). On Base almost every Zora
creator is a smart wallet whose transaction signer is a shared ERC-4337 bundler.

Rule (`scout_devdb::SIGNER_RESOLVED_LAUNCHPADS` = flap, fourmeme, pons — launchpads whose creator field
is or can be the caller): a contract creator with several signers is a shared intermediary and each
launch's signer is the dev (`contract:<addr>` until resolved, never merged with real users). A
single-owner contract stays its own dev (its signer may be a relayer/bundler shared by many users, so it
is never merged into it). EOAs and every other launchpad (Zora `payoutRecipient`, Clanker `tokenAdmin`)
keep the creator field. Residual risk: a Pons/Flap dev on a smart wallet is resolved to its bundler.
Cost: one `eth_getCode` per new creator on those launchpads, ≤ 8 lookups per new contract creator, one
lookup per launch through a shared intermediary (live first pass: ≈ 13k requests for one day, all chains).

## Amendment 2 (2026-10-07) — pump.fun facts

- **Launch** = the self-CPI `CreateEvent` (IDL `pump_idl_e0687ae9.json`) in a successful transaction touching the
  `mint_authority` PDA `TSLvdd1p…` (`create` / `create_v2` only); the dev is `CreateEvent.creator` (normally the
  signer `user`; no signer resolution on Solana). `created_block` = slot, `tx_hash` = signature.
- **Migration** = `CompletePumpAmmMigrationEvent` (mint, pool) in a successful transaction signed by the migrator
  `39azUYFW…`. Its other transactions are not migrations: "Bonding curve already migrated" no-ops (no event) and
  buys. Live page (60 txs): every `migrate`/`migrate_v2` with `CreatePool` carried the event (44/44).
- **Cursor** = the next window's inclusive start in unix seconds (Helius `filters.blockTime.gte`); the window ends
  60 s before now. A pass cut by the page budget restarts at the newest block time it saw (re-read, idempotent).
- **Cost** (Helius full mode, 10 credits / 100 txs): ≈ 1.8k txs/hour → ≈ 4.5k credits/day steady state.
- **Backfill** is not done by default: one year of creates is ≈ 19M txs (≈ 1.9M credits), more than the free
  plan's month. Forward ingestion fills the window as time passes; a deeper first pass (90 days of migrations
  ≈ 20k credits, creates of qualifying creators only) waits for the owner's decision on credits.

## Amendment 3 (2026-10-07) — which launches get a signer lookup (owner decision: option B)

Measured on one day: BSC 3,243 launches through shared intermediaries (8 migrated), Robinhood 270 (2 migrated);
BSC 1,693 creators (807 with ≥ 2 launches), Robinhood 2,932 (224). Resolving every signer and classifying every
creator for a year would cost ≈ 6M Alchemy calls (≈ $30–60); dRPC free cannot serve it (it answers `null` for
launch transactions older than a few hours).

- A signer is looked up for every migrated launch through an intermediary (any age) and every launch younger than
  7 days (`identity::RECENT_SIGNER_WINDOW`), so new launches are always attributed; on Robinhood
  (`resolves_every_signer`) for all of them.
- A dev with a migrated launch through an intermediary is a candidate; its transactions to every shared
  intermediary of the chain are listed once (`alchemy_getAssetTransfers`, top-level calls, zero values included;
  6/6 sampled launches found) and matched to launches by hash (`launches_by_tx` index, table `dev_histories`).
  This runs in every cycle, so a dev who becomes a candidate later (an old launch migrating) gets its history then.
- A launch through an intermediary whose signer is still unknown belongs to no dev (excluded from the derivation,
  never one pseudo-dev per contract). Such a dev has no migration in the window, so it cannot qualify (top-migr and
  win-streak need migrations; a curve token reaches a runner's ATH only after graduating).
- Only creators with ≥ 2 launches are classified (a single launch is its creator's own); 4 owner samples, not 8.
- Expected one-year identity cost ≈ 0.5M calls ≈ $5; steady state cheaper than before.

## Amendment 4 (2026-10-08) — a runner's ATH must be real

Codex `athFdv` is the highest price ever times the total supply; one tiny trade in an empty pool makes it huge.
Live, Base: Zora coins with 1–8 holders at $1.4M–$99M; 400 Bankr tokens (Doppler v4, 10B supply, ~2,000 airdrop
holders each) at $380M–$6B with current liquidity of $0 (median) to $3.5k (max). Therefore:

- each ATH observation also stores Codex `holders` and `liquidity`; the store keeps the highest liquidity seen
  across observations (young tokens are observed every 12 h, so a real runner's peak liquidity is captured);
- a token is a runner only with `top_runners.min_holders` (default 100) holders — unknown is never assumed;
- on `top_runners.liquidity_checked_launchpads` (default Bankr, Noice) the highest liquidity seen must also be at
  least `min_liquidity_bp_of_ath` (default 100 bp = 1 %) of the ATH;
- migration 0005 made existing observations stale so they are re-observed with holders.

Live after the change (Base, 1,889 observations): ATH ≥ $500k — Bankr 347 → 0 runners, Zora 45 → 0, Clanker 5 → 1.

## Amendment 5 (2026-10-10) — bounded candidate-history work and incomplete paging

BSC backfill exposed a long identity step: the daemon checked 500 candidate devs
against 66 shared intermediaries sequentially, then reported 4,039 attributed
launches. Its 90-minute progress watchdog fired before that step completed and
recovered afterwards. Dev identity and transaction matching remain unchanged.

Candidate histories now run with at most the RPC client's configured concurrency
in flight; each dev checks its intermediaries sequentially and checkpoints only
after its own history finishes. Completed checkpoints survive cancellation or
another dev's failure. The same transport limiter still accounts for every call.

A remaining `pageKey` after the existing 50-page bound returns typed
`IdentityError::HistoryPageLimit` and leaves that dev uncheckpointed. A bounded
partial listing cannot prove a complete dev history. Existing completed records
are retained; this change does not retroactively prove that older listings reached
their natural end. The local PostgreSQL/mock-RPC callsite fixture verifies overlap,
per-dev checkpoints and the page-limit failure without paid RPC.

## Amendment 6 (2026-10-10) — seek ATH candidates before stale observations

On the live Base database, the old candidate query scanned millions of launches,
counted every creator's launches and sorted before its limit. A 4,000-token
diagnostic did not finish within 45 seconds; an abandoned daemon query remained
active for over 20 minutes after its client stopped.

Migration 0008 adds `(chain, created_at DESC, token)`. The query first selects a
bounded, ordered set without an ATH row using `NOT EXISTS`, then fills only the
remaining capacity with stale observations. Creator eligibility uses an indexed
third-row existence check instead of counting the whole history. Eligibility,
refresh ages and missing-before-stale priority are unchanged; token address is
the deterministic tie breaker. `observed_at` is non-null in the schema, so absence
of an ATH row and the old null-join predicate have the same meaning.

The null-join variant of the new query still timed out at 30 seconds because
PostgreSQL estimated just one missing row and chose a full scan. The anti-join
used the new index. A warm diagnostic for 20,000 Base candidates completed in
765 ms of server execution, with sampled backend peak RSS 232,268 KiB (20 ms
sampling). Environment: PostgreSQL 16 in local Docker, Intel i7-12700K, 16 logical
CPUs visible, 24,031,788 KiB RAM, over 10 million Base launches, concurrent forward
ingestion and Noice backfill. This measures one SQL query, with partially warmed
buffers and no provider calls; it is not a cold-cache or end-to-end throughput
claim. SQL, EXPLAIN/BUFFERS and environment metadata are saved locally under
`.scout-data/diagnostics/ath-base-candidates-antijoin-20000.*`.

## Amendment 7 (2026-10-10) — bounded enumeration of provider-known Base ATH

Directly submitting Base's 8.8 million unobserved eligible addresses would need
about 44,000 batches of 200, above the current Codex account's remaining quota.
The opt-in `backfill --chains base --phases ath --ath-from-index` instead
enumerates positive ATH from Codex's index and intersects each page with tracked
Base launches. It does not change the runner definition or fabricate ATH for
unindexed, untraded or unpriced contracts.

Existing `codex-missing` rows retain their stored zero sentinel for retry
scheduling, but the dev-derivation query projects their ATH as `None`. A real
provider-reported zero remains a numeric zero; missing is not evidence that the
token's ATH was zero.

Liquidity is converted to cents from an exact decimal string or integer USD;
fractional JSON numbers are rejected rather than rounded through a float. Null
holders/liquidity remain unknown. The public live-response subset fixture
`crates/scout-devtracker/tests/fixtures/codex_base_positive_ath_2026-10-10.json`
checks the actual provider shape without credentials or a completeness claim.

The filter fixes network 8453, mainnet, `includeScams:true`, `athFdv > 0` and a
half-open interval of provider first-pair `createdAt`, ordered ascending. Scams
remain visible. The requested scope is the last `--days` closed UTC days of
first-pair creation; it is distinct from contract launch time and from the ATH
observation timestamp. Observations retain the time they were fetched. A token
launched in the requested year whose first pair lies outside those intervals is
a coverage gap for this pass.

Pages contain at most 200 results. Codex rejects `limit + offset > 10,000`, so a
full interval at the offset boundary is divided into `[start, midpoint)` and
`[midpoint, end)`. A saturated one-second interval is an explicit error. Versioned
per-day cursors retain pending intervals and offset, and advance only after
idempotent ATH writes. Interrupted pages can be replayed. A completed cursor
means the pass reached the index's natural end for that interval, not that Codex
indexed every contract or offered an immutable snapshot: live index changes and
offset ordering remain provider limitations. Re-enumeration after later fact
ingestion requires a fresh pass; completed days are not silently reopened.

The pass bounds response bodies at 4 MiB and request attempts by
`--ath-requests`; retries count towards that bound. Authentication, quota,
malformed data and interval saturation preserve the unfinished cursor. A shared,
opt-in `cursors` counter for the Codex account reserves one allowance atomically
before every HTTP attempt, including the daemon and retries. Absence preserves
legacy behavior; zero stops requests. The application never resets or estimates
the monthly account quota automatically. Exhaustion is a Codex incident while
chain-fact ingestion continues. An operator sets the counter from the dashboard
with a conservative margin and explicitly updates it when a new period starts.

Local PostgreSQL/mock-HTTP tests cover paging, resume, subdivision, tracked-token
intersection, malformed/network mismatches, shared budget exhaustion and retry
accounting. Live probes and source limitations are recorded in
`docs/p0/measurements/2026-10-10-base-ath-sources.md`.

## Amendment 8 (2026-10-11) — provider date mismatch is explicit provenance

This corrects amendment 7's first-pair membership assumption. Codex's
[TokenFilters](https://docs.codex.io/api-reference/input-objects/tokenfilters) and
[TokenFilterResult](https://docs.codex.io/api-reference/types/tokenfilterresult)
describe `createdAt` as first-pair creation, while
[filterTokens troubleshooting](https://docs.codex.io/api-reference/queries/filtertokens)
describes the result field as current top-pair creation. The server predicate is
not established by these conflicting descriptions.

A live Base page filtered to `[1791072000, 1791158400)` (2026-10-04 UTC)
returned `0x3084d9ed7ebb5670a6192b4716b99256d52212c2` with integer
`createdAt=1775701583`, matching its tracked April launch. Result, nested token
and selected pair dates agreed. A separate
[listPairsForToken](https://docs.codex.io/api-reference/queries/listpairsfortoken)
response contained 60 pairs, including one created at `1791093681` inside the
requested day. This supports a multiple-pair explanation; it does not prove
which date the server actually filtered.

The scope is now explicitly **provider createdAt-filter enumeration** over
closed UTC intervals. A valid nonnegative integer result date outside its
requested interval records a quality gap instead of stopping the page. The
address/network and positive exact-decimal ATH remain validated. A tracked
observation retains its original ATH and ATH timestamp, with source
`codex-index-date-mismatch`; no replacement date is fabricated. Invalid,
fractional or negative date fields still fail. Only `codex-missing` is the
unknown scheduling sentinel; a date gap does not make a valid ATH unknown.

The cursor JSON contract stays version 1 with serde-default
`date_mismatch_rows` and `date_mismatch_examples`. At most four examples per
day retain token, returned date and requested interval. Counts describe
processed provider rows, not unique tokens: subdivision may replay rows. Raw
page length still controls offsets, and quality metadata is checkpointed with
the page only after idempotent ATH writes. Completed-day resume preserves
counts without additional requests. Existing cursors default to zero; earlier
completed pages are not retroactively revalidated.

`complete=true` means enumeration reached natural ends in the requested
provider-filter intervals. It does not certify first-pair date membership,
all-contract ATH coverage or an immutable provider snapshot. CLI, local
manifest and documentation retain these limitations and mismatch counts.
No runner thresholds, monetary arithmetic or ATH definition change.

The two-row live-response subset fixture
`crates/scout-devtracker/tests/fixtures/codex_base_created_at_mismatch_2026-10-11.json`
and mock-HTTP/isolated PostgreSQL regression failed against the strict old
parser, then passed. They verify the original 165,600-cent ATH/timestamp,
source provenance, offset advancement, persisted examples and zero-request
completed-day resume. Unit tests cover bounded examples, old cursor defaults
and malformed timestamps/quality metadata.

## Amendment 9 (2026-10-11) — independent, bounded ATH polling

Facts use Helius on Solana and Alchemy with Infura fallback on BSC/Base.
Codex supplies ATH observations across all four configured chains; its
monthly account allowance is separate from RPC quotas. With nonempty
candidate queues, one call per chain on every 30-minute ingestion cycle
can consume 192 calls/day across four chains. An explicit legacy budget of
20 calls/chain/cycle is much higher; it must not be mistaken for a monthly
free-tier guarantee. The owner requested light regular Codex usage before
the October 31 account reset.

The daemon schedules facts, ATH and derivation separately. New optional
`schedule.ath_every_minutes` defaults to 240 and must be positive.
`ath_requests_per_chain` defaults to 1; zero disables ATH. Four configured
chains therefore permit at most 24 successful scheduled calls/day, or 744
over 31 days, excluding additional process restarts and manual backfills.
The direct observer does not automatically retry HTTP calls. The shared
account allowance still bounds every HTTP attempt, including other jobs.
Explicit old TOML budgets remain explicit and require review; omitted fields
use the new defaults. Facts default to 30 minutes, derivation to 60.

Daemon ingestion passes use zero inline ATH budget. A separate ATH deadline
can wake the loop without a facts pass. Only an actual ATH pass updates its
Codex incident; disabled/skipped ATH must not report recovery. Existing
provider/facts incidents and shutdown cancellation remain unchanged. The
facts progress watchdog still uses the existing facts/derivation cadences: a
slow optional ATH interval cannot extend the regular facts stall allowance.

Candidate eligibility, observation refresh ages, runner gates, monetary
arithmetic and missing-is-unknown semantics do not change. A bounded poll
can leave a large queue behind; this configuration is a spending bound, not
a guarantee of complete or fresh ATH for every candidate. Free current
quotes or limited candle history are not silently substituted for full ATH.

Config regression failed before the new field existed, then passed. Tests
cover legacy/default TOML, invalid zero cadence, independent ATH deadline,
ATH-only wake, disabled budget and unchanged 90-minute default watchdog.

## Amendment 10 (2026-10-11) — all-time filtered EVM runner enumeration

The separate opt-in `backfill --chains <solana|base|bsc|robinhood> --phases ath
--ath-runners-from-index` enumerates Codex mainnet tokens on Solana, Base,
BSC, or Robinhood Chain with
`athFdv >= top_runners.runner_ath_usd`,
`athFdv <= top_runners.max_plausible_ath_usd`, and
`holders >= top_runners.min_holders` from the selected
`--ath-index-config` (default `config/dev-tracker.toml`). It sets
`includeScams:true`, orders by `createdAt ASC`, and does not send a
first-pair `createdAt` range. Optional
`--ath-index-token-created-since <unix-seconds>` adds the provider's
`tokenCreatedAt: {gte: ...}` birth-time filter and has a separate cursor; the
requested `token.createdAt` is checked as returned-data quality metadata.
Missing, malformed, negative, future, or below-floor returned token birth
timestamps increment a bounded count while valid numeric ATH remains stored.
This is provider-filter membership only: returned birth dates do not prove
coverage of every token created after the floor. The current holder count and
current index membership can change.

The pass stores only rows matching already tracked tokens on the selected
chain. Chain and Codex network are part of cursor identity and database
intersection, so identical address strings on different networks remain
separate assets. Solana mint addresses are validated as 32-byte Base58 keys
and retain their case-sensitive spelling in storage and lookup; EVM
hex-address normalization remains lowercase. The Base cursor continues its
existing namespace. The pass advances by raw provider page length,
and checkpoints only after idempotent ATH writes. Unknown database membership
does not change pagination or create a zero/missing ATH observation. Different
thresholds use separate cursors. A full provider page at the 10,000 offset cap
is an explicit saturation error; it does not prove natural completion. The
shared Codex request budget is reserved before every request, with no retries
in this mode. Response bodies remain bounded.

`complete=true` means only that the provider-filtered enumeration reached a
short page. Mutable offset ordering can still skip or repeat rows as Codex's
index changes. It does not establish full-lifetime ATH coverage, an immutable
snapshot, or absence of qualifying tokens outside the provider index. Annual
category derivation remains independently scoped by `since_days` from tracked
launch facts. Unknown or unindexed tokens remain unknown, never zero. The
optional token-birth floor also does not establish complete birth-time or
launch coverage. The
public five-row Base response fixture
`crates/scout-devtracker/tests/fixtures/codex_base_runner_filter_2026-10-11.json`
records the observed provider shape without credentials. The synthetic
Solana mock fixture `codex_solana_runner_filter_mock.json` checks the Base58
mint shape without representing a live Codex response.

This is a targeted candidate-enumeration aid for runner analysis. It does not
change the runner definition, ATH policy, or statistical interpretation; the
filter itself selects high-ATH tokens and therefore cannot be treated as an
unbiased sample of all launches.

## Amendment 11 (2026-10-11) — retain quality metadata when ATH is unknown

Codex can return a token, its current holders and liquidity, but null extrema.
The regular observation path previously discarded that metadata. Such a row
now remains `codex-missing` (the derivation still projects ATH as null), with
the returned optional holders and liquidity retained. The existing maximum
observed liquidity rule is unchanged. Omitted tokens still have unknown
metadata. Neither case is a numeric ATH zero. The report's missing count
includes both null-ATH results and omitted tokens.

Validate returned network, requested token membership and duplicate rows
before persistence. Store lowercase EVM keys and case-sensitive Solana keys.
The sanitized null-extrema fixture and test against `scout_test` check these
properties and preservation of prior maximum liquidity. This changes no
runner or migration threshold and supplies no missing extrema.

## Amendment 12 (2026-10-11) — separate Birdeye price-window observations

Birdeye token OHLCV supplies daily USD-per-token prices, not Codex ATH FDV.
The explicit `birdeye-prices` command saves these in migration 0009's
`ath_price_windows`, keyed by network, token, source policy and requested
inclusive time range. It neither updates `ath` nor enters runner derivation.
Empty returned series have null prices and zero returned-candle count;
that is not a numeric zero price or evidence of no historical trading.

Use `type=1D`, `currency=usd`, `padding=false`, `outlier=true`; retain the
requested range, first/last returned UTC bucket, maximum's bucket and
observation time. A bucket timestamp is not the exact time of its high.
Partial current-day buckets, omitted no-padding days, pool aggregation and
provider tracking start do not prove lifetime coverage. The command's
completed pass means only that every requested price window was saved.

Calls are sequential, at least one second apart, with an explicit attempt
ceiling and no automatic retries. Input is bounded to 2 MiB/2,000 entries,
responses to 2 MiB/5,000 candles and 25 seconds. Access denial or rate limiting
stops the pass. Exact saved windows are skipped on resume; transport/schema
failures are not saved as empty observations and cause a partial exit.
Prices preserve JSON decimal lexemes, including scientific notation, and
use checked integer arithmetic. Unsupported scaled UI prices are rejected.

The overview adapter separately exposes current optional supply, holders,
liquidity, price, market cap and FDV. It does not equate current supply with
historical invariant supply, or current FDV with ATH. Token-version/supply
and observed market-scope validation remain required before any future
promotion into the monetary ATH table. The supply findings and outstanding
checks are in `2026-10-11-birdeye-fdv-policy.md`. Golden response fixtures,
mock HTTP tests and a real `scout_test` persistence test cover the boundary.

## Amendment 13 (2026-10-11) — a verified fallback FDV observation

A hand-checked Birdeye price window may supply a missing numerical FDV
observation only after validating its denominator and USD market identity.
The first case is Base token `0xe44861a39b8f73040455fa618aa921907d450b07`:
the exact verified, non-proxy ClankerToken source allows constructor mint,
ordinary burns and Superchain bridge mint/burn. Current supply alone was
insufficient. Alchemy's token-scoped zero-address Transfer queries from the
creation block through the confirmed state block returned one creation mint
and no burns, reconciling exactly to `totalSupply()` = 100 billion, decimals
18. Under that verified ERC-20 implementation, supply was constant across
the observed price window.

GeckoTerminal's launch-time token/USDC pool independently reports a high on
the same UTC day within 0.19% of Birdeye. Its other returned token/USDC pool
was also checked. This corroborates this price level; it does not prove
complete token-wide indexing or rule out thin-market prices. Existing holder,
liquidity, plausibility and migration gates are unchanged.

Further hand-checked cases require the same implementation/supply evidence,
a same-day USD high within 1% of Birdeye, and a maximum across every returned
pool history within that tolerance. The 1% comparison is corroboration of
provider observations, not an assertion about a canonical market-wide ATH.

The source is `birdeye-window-supply-validated-v1`. Retain its exact price
window and day bucket in `ath_price_windows` and the dated proof manifest;
`ath_at` stays null because a candle does not give the exact trade time.
The numerical value means maximum FDV observed in that provider window,
not a new claim of canonical lifetime completeness. It may contribute to
the existing provider-observed runner derivation with this explicit source
scope. Uncorroborated or supply-unverified windows remain separate prices.

`upsert_missing_ath` atomically fills only absent or `codex-missing` rows,
rejects older observations and protects every existing numeric observation,
including zero. The caller must establish the source/supply evidence; this
storage primitive cannot establish it. Golden fixture
`birdeye_validated_fdv_case.json` checks the exact floored 51,104,374,107 cents;
the database test checks replacement guards and maximum liquidity retention.
No broad automatic promotion of the other Birdeye windows is enabled.

A later `codex-missing` refresh must not erase a previously known numeric ATH,
including this validated fallback. It advances `observed_at` to record the
latest refresh attempt, while retaining the prior `ath_fdv_cents`, `ath_at`,
and `source` as the original value and window proof. A null incoming holder
count also retains the previous holder count; observed maximum liquidity
remains a high-water mark. Thus refresh time is not the observation time or
provenance of a retained ATH value.

## Amendment 14 (2026-10-11) — swap-event price corroboration with FX evidence

For fixed-supply tokens covered by amendment13, a provider's USD candle high
may also be independently corroborated by a successful, deployment-gated
pool Swap event on the same UTC day. Validate the exact token/quote identity,
decimal scales, signed nonzero opposite deltas, receipt/block/hash/log index,
and all queried known-pool execution maxima. The ratio is a pool execution
quote, not a wallet-net price or an attributed purchase. Keep post-swap
square-root spot prices separate; hook/router fees may alter wallet flows.

A non-USD quote requires an independent dated FX observation. For the three
Base token/USDC cases, Kraken's closed daily USDC/USD low/high observations
are used as a sensitivity range: converting the exact event ratio at both
ends must stay within 1% of Birdeye's USD high. This is cross-source price
corroboration, not a guaranteed bound on global FX or a fixed USDC peg.
The value stored remains Birdeye's USD high times proven invariant supply;
no synthetic on-chain USD price replaces it.

Use source `birdeye-window-supply-swap-fx-validated-v1`, retain the provider
window, FX and exact-event evidence, and leave `ath_at` null. The same guarded
missing-only write policy applies. Coverage is the queried known pools and
provider window; new canonical lifetime or wallet-PnL completeness is not
claimed. Variable-supply cases remain unpromoted until the denominator and
price time can be reconciled without guessing intrabucket order.

The same dated-FX check covers five Robinhood token/WETH cases using a closed
Coinbase ETH/USD minute at each successful Swap timestamp. Both minute
low/high conversions stay within 1%. The quote wrapper is an upgradeable
proxy: verified proxy/implementation bytecode and the EIP1967 implementation
slot at every observed Swap block establish the same verified aeWETH code;
its deposit/withdraw paths convert native ETH 1:1. A current proxy address or
token symbol alone is insufficient. Token clones separately match verified
Doppler source and their complete mint/burn ledger establishes constant 1B
supply through the anchor. Retain these source/slot/receipt/FX proofs and
golden `birdeye_robinhood_swap_fx_validated_fdv_cases.json`. Observations are
from launch-context pools and may reflect thin markets; source scope and
`ath_at=NULL` remain unchanged, as do monetary thresholds and wallet fee rules.

## Amendment 15 (2026-10-11) — corroborated alternative-provider windows

When provider highs disagree, do not average them or increase the tolerance.
A missing numerical ATH may use a separately corroborated GeckoTerminal
known-pools USD window high under the same source/supply/receipt/dated-FX
checks as amendment14. Preserve the contradictory Birdeye window separately.
The three BSC cases use verified invariant 1B supply and registry-gated
PancakeSwap V2 pool identities/full-range Swap logs. Successful peak receipts
and block timestamps match the provider's peak day. Known returned curve
and unsupported-venue provider histories remain in the maximum comparison;
they do not become supported on-chain DEX claims.

Their WBNB quote matches verified nonproxy wrapping code. Convert the exact
pool execution ratio with a closed KuCoin BNB/USDT minute and dated Kraken
USDT/USD low/high observations; both resulting endpoints must be within1%
of the GeckoTerminal high. Do not assume USDT parity or use a daily BNB price
for a timestamped Swap. Values stored remain the corroborated provider USD
high times proven supply, with source
`geckoterminal-window-supply-swap-fx-validated-v1`, `ath_at=NULL` and a separate
`ath_price_windows` entry. Apply the existing missing-only write guards;
known numeric observations, including zero, remain protected.

Golden `geckoterminal_bsc_swap_fx_validated_fdv_cases.json` checks exact
FDV cents and both FX conversions. This remains observed-window/known-pool
evidence with thin-market and unindexed-venue gaps, not canonical lifetime
completeness. USD1 requires dated market evidence; its token name alone does
not supply a USD exchange rate.

The fourth BSC case instead corroborates Birdeye using a closed, traded
KuCoin USD1/USDT daily candle and dated Kraken USDT/USD daily low/high. Pair
and BSC quote-contract identity are verified; both daily-conversion endpoints
agree within1%. Its zero-volume minute is excluded as an actual trade
observation, and no five-minute populated-candle bracket was established.
Daily sensitivity follows amendment14's stablecoin method and does not
provide an instantaneous or executable FX quote. Store the Birdeye value
under `birdeye-window-supply-swap-fx-validated-v1`; retain the contrary GT
window and timing gaps. Golden
`birdeye_bsc_usd1_daily_fx_validated_fdv_case.json` checks exact FDV and both
FX steps. The unchanged1% check applies to both ends, without assumed pegs.

Five additional Base Clanker cases follow the same alternative-window policy
with verified invariant 100B supply and registry-gated Uniswap v4 pools.
Historical USDC decimals at each successful peak block are6; token decimals
are18, requiring an exact10^12 ratio adjustment. Both dated Kraken USDC/USD
daily endpoints agree within1% of the GT maximum over all returned page1
pool histories. Quote notionals are small, including3 raw USDC units; retain
this limitation rather than equating FDV with executable market depth.
Golden `geckoterminal_base_swap_fx_validated_fdv_cases.json` records the
decimal conversion, floor-to-cents values, known-pool maxima and day ties.
For an equal maximum appearing on multiple UTC days, corroboration may select
the winning day that contains the independently verified Swap; the original
vendor rows and any Birdeye day disagreement remain intact. This provides
a source-window high day, not a precise ATH timestamp; `ath_at` stays null.

## Amendment 16 (2026-10-11) — stock-token quote with traded cross conversion

The remaining Robinhood SMOGGY3 observation uses NVDA as its pool quote.
An equity ticker or a stale oracle answer is insufficient USD evidence.
Validate the exact stock-token contract, its decimals and raw-unit multiplier
at the target block. Independently identify its NVDA/USDG v3 pool by official
factory deployment, historical token0/token1/fee/decimals and factory lookup;
verify source/runtime and successful trade receipts. This is manual FX
evidence, not admission of a new DEX to the wallet decoder registry.

Use actual opposite-delta NVDA/USDG executions before and after the target
Swap, each within five minutes in canonical block order. This case has one
trade one second before and two trades in a later block at the same timestamp
second. Verify exact USDG identity against Paxos's official deployment table
and historical OFT token/unit getters. Convert all three observed execution
ratios with Kraken's closed, positive-volume USDG/USD daily low/high; every
endpoint must agree with the Birdeye high within1%. Do not assume a peg or
substitute the stale stock/USDG oracle. Both quote legs use exact arithmetic;
the USDG6/NVDA18 ratio requires10^12 normalization.

Retain the original Birdeye high times proven invariant1B token supply,
`ath_at=NULL` and source
`birdeye-window-supply-stock-swap-fx-validated-v1`. Apply missing-only guards
and preserve the nearby trade, source, identity and daily FX proofs. Golden
`birdeye_robinhood_stock_quote_swap_fx_validated_fdv_case.json` checks the
two quote legs, canonical time bracket, both FX endpoints and FDV floor.
This corroborates a provider-window USD level in the identified pools;
the trades are not simultaneous fiat executions, a guaranteed global FX
bound, a redemption guarantee, or a canonical lifetime ATH proof.

## Amendment 17 (2026-10-11) — retain Solana Tracker provider ATH observations

The explicit `solana-tracker-ath` command may save the provider's `highest_price`,
millisecond timestamp, pool id and `highest_market_cap` in a separate
`ath_price_observations` table. Decimal JSON numbers are parsed exactly;
missing or null fields remain null. The response hash, observation time and
fixed scope `provider-recording-window-unknown` are retained. These fields are
not mapped into `ath_price_windows` (which means a requested candle interval)
or `ath` (which means the selected monetary ATH-FDV record). No price-times-
supply calculation or runner eligibility change is implied.

Provider-reported ATH means only the provider's recorded history; API contract
does not promise chain-lifetime tracking or complete market-wide pool coverage.
The command is opt-in, outside the daemon, sequential at no more than one
request per second, with a per-run attempt ceiling, 2 MiB response cap, no
automatic retries, and exact-observation skip on resume. Access denial,
payment rejection or rate limiting stops the pass.

Every request first reserves from a durable, owner-selected *application*
monthly allocation keyed by the stable quota group `owner-soltracker`, not by
credential fingerprint alone. The monthly app cap and confirmed prior app
usage must be supplied and match the persisted baseline; the cap is at most
250. The known prior seven requests are app usage only, not a statement about
the provider account's monthly usage or remaining quota. Key fingerprints are
stored only as request provenance, and key values never enter URLs, database fields,
or output. Failed HTTP attempts count against the app allocation. This ledger
does not claim to include requests outside the tracked app; coordinate them
when choosing the allocation and prior-use baseline.

Example invocation (the JSON file must contain only already-tracked Solana
mints; set `SCOUT_SOLANA_TRACKER_API_KEY` in the process environment and choose
a private/raw-data directory):
`dev-tracker solana-tracker-ath --tokens solana-mints.json --raw-dir .scout-data/solana-tracker-ath-raw --max-requests 100 --billing-month 2026-10 --application-monthly-limit 250 --prior-application-usage 7`.
The command is not part of daemon startup or ATH-FDV derivation.
Successful 2xx response bodies are archived before validation under filenames
derived from the token and response hashes. Bodies are capped at 2 MiB and
scanned for the configured key before writing; malformed bodies remain
available for review but do not become observations.

Sanitized 48, 81 and JC responses are frozen in
`crates/scout-devtracker/tests/fixtures/solana_tracker_ath_{48,81,jc}.json`.
Adapter tests preserve exact values and nulls and reject malformed decimals,
timestamps and pool ids. A `scout_test` database test checks network-keyed
storage, unknown-field retention and separation from the ATH candidate table.

## Amendment 18 (2026-10-11) — bound FDV cents for a decreasing supply

`ExactDecimal::fdv_cents_for_raw_supply` multiplies an exact USD price by raw
token supply and divides by `10^decimals`, flooring only after conversion to
cents. `fdv_cent_bounds_for_nonincreasing_supply` evaluates current supply for
the lower endpoint and birth supply for the upper endpoint. A single rounded
cent value is established only if both independently checked endpoint floors
are equal.

The bound requires validated birth and current supplies, correct decimals,
irreversible mint-authority revocation during the verified creation transaction,
no supply-changing
mint extensions or other rebase/mint paths, and a current snapshot later than
the provider's maximum-price timestamp. The snapshot need not postdate every
other provider record: the lower endpoint uses current supply as a lower bound on supply at the
maximum-price observation, and the upper endpoint uses birth supply. The helper
checks arithmetic and `0 < current <= initial`; callers must establish those
historical conditions. An unknown provider window remains unknown: this bound
does not establish lifetime or market-wide price completeness, validate the
price independently, provide an ATH timestamp, or change runner eligibility.

The frozen Solana Tracker examples `8Cc` and `PEC` have equal bounds of
4,795,252 and 6,565,114 cents respectively. The JC negative control has bounds
of 22,731,099 to 23,536,979 cents; the interval does not collapse, so its
provider peak cannot be assigned one exact FDV cent value by this method.
Fixture `solana_tracker_supply_fdv_cent_bounds.json` pins the inputs and the
source join hash. These arithmetic examples do not promote or write ATH facts.

## Amendment 19 (2026-10-11) — corroborate scoped Solana Tracker prices with AMM spots

Two source-specific observations have additional timestamp-adjacent spot
evidence beyond the supply bounds in Amendment 18. The 8Cc Solana Tracker price is
`0.000047952524582282125` at `1771704112806` ms. The verified migration
transaction creates the exact target/WSOL pool and executes its initial AMM Buy;
the post-Buy vault balances are `98956567965` lamports and
`177704238872329` target raw units. The same-minute closed Coinbase SOL-USD
candle is `$86.09–$86.14`. Exact reserve-ratio conversion places both endpoints
within 1% of the provider point, with the provider timestamp 1.806 seconds
after the transaction's one-second-resolution block time. This is an
independent, timestamp-adjacent spot check, not an execution-average price.
The initial Buy also reconciles against its fee-free `quote_amount_in`: with
post-Buy base reserve `177704238872329`, base output `29195761127671`, and
pre-Buy quote reserve `84990359206`, the exact integer-rounding interval
`[(quote_in−1) × post_base / base_out, quote_in × post_base / base_out]`
contains the recorded pre-Buy quote reserve.

For this observation only, the verified birth/current supplies (`1000000000000000`
and `999999927688911` raw units at 6 decimals) give equal lower and upper
provider-price FDV floors of `4795252` cents. A finalized current-supply
snapshot follows the price record; the birth receipt and monotonic-supply
preconditions are recorded in the source evidence. The later same-pool state is
a negative control: its spot is about `$0.00000369`, far outside the 1%
comparison. This prevents substituting an unrelated later trade state for the
migration-time pool observation.

The PEC Solana Tracker point is `0.00006565114747464346` at
`1762660115868` ms. Its initial migration Buy state is outside the strict 1%
spot comparison. Two subsequent same-slot PumpSwap Buy post-states in the same
minute both meet the comparison at both Coinbase endpoints; the later is
`$0.0000656342961374–$0.0000657186264678` per token. The provider timestamp is
1.868 seconds after the integer block time of these buys. A first sell has the
same integer block-time second as the provider point, so its exact ordering
relative to the provider's millisecond timestamp remains unknown. This is
nearby price corroboration, not proof of the exact pool state at that instant.
PEC's birth/current-supply endpoints independently floor to `6565114` cents.

The source name for this narrowly corroborated record is
`solana-tracker-window-supply-bounds-pool-fx-validated-v1`. It retains
`ath_at=NULL`, `provider-recording-window-unknown`, and unknown market-wide
scope. This does not establish lifetime price history, complete pool coverage,
or canonical ATH/FDV completeness. The provider's `highest_market_cap` and
Pump `/coins-v2` `ath_market_cap` remain source-only fields and are not the
candidate's FDV basis. Fixture
`solana_tracker_scoped_spot_supply_cases.json` checks both exact supply bounds,
timestamp relationships, CPMM rounding intervals, both FX endpoints, and the
8Cc later-spot negative control. The full source reconciliations are in
[`2026-10-11-solana-8cc-migration-pool-spot-corroboration.md`](../p0/measurements/2026-10-11-solana-8cc-migration-pool-spot-corroboration.md).
[`2026-10-11-solana-pec-migration-spot-corroboration.md`](../p0/measurements/2026-10-11-solana-pec-migration-spot-corroboration.md).
These records preserve `ath_at=NULL`, unknown provider recording window, and
unknown market scope. They add no automatic promotion and change no runner
gates.

These two manually reviewed candidates may fill only existing `codex-missing`
zero-sentinel rows, guarded by matching stored provider price, timestamp, pool
and response hash. Preserve holders/liquidity and every already-known numeric
observation, including genuine zero. This selects an observed-provider-scope
FDV under the checks above; it does not upgrade either record's completeness.

## Amendment 20 (2026-10-11) — exact-cent bound from peak-time supply

For a provider recording scope with maximum recorded price `Pmax`, verified
nonincreasing supply, birth supply `B`, and supply `Speak` at that price, the
maximum FDV in that scope is bounded by `Pmax × Speak ≤ max(Pi × Si) ≤ Pmax × B`.
If exact cent floors of both endpoints match, the maximum's rounded cents are
determined without assuming that supply stayed constant. This does not extend
the provider's coverage beyond its unknown recording window or independently
validate its prices.

The frozen burn reconstruction (SHA-256
`419c8f7f1d8064cc67b230d800e011f691257de9cd8853b66f41405fc9e3c143`) records
five Solana Tracker maximum-price observations with complete birth-to-snapshot
burn ordering. Three have `Speak = B`; two have `Speak = B − 1` raw unit. For
all five, the existing exact decimal-to-cent calculation gives equal endpoint
floors, respectively `12133750`, `5912528`, `7616407`, `3236880`, and `8220527`
cents. Their birth and current supply differences are larger, but must not be
substituted for peak-time supply when applying this bound.

The JC negative control's conservative interval does not collapse: at price
`0.00023536979981208752`, the finalized current-supply lower bound is
`965761089271256` raw units against birth supply `1000000000000000` at 6
decimals. The resulting lower and upper floors are `22731099` and `23536979`
cents. Current supply is a lower bound on supply at this earlier price point,
not its exact peak-time supply. This wide interval cannot assign one exact
source-scope maximum FDV cent value; a tighter historical price/supply bound
could still establish one.

Fixture `solana_tracker_peak_supply_fdv_bounds.json` pins the five-case
reconstruction and JC negative control. This arithmetic does not assign an ATH
timestamp, establish lifetime or market-wide price completeness, waive the
independent price-validation gate, or trigger automatic promotion.

## Amendment 21 (2026-10-11) — two more scoped price checks

Two additional Solana Tracker maximum-price observations pass the existing
independent spot-price comparison using transaction states from the named
PumpSwap pool and the closed Coinbase SOL-USD candle for the matching UTC
minute. For 5b2, the selected post-Buy state is 1.990 seconds before the
provider timestamp. Because the legacy quote-input interpretation differs by
one lamport, the CPMM check uses both `user_quote_amount_in − 1` and
`user_quote_amount_in`; the resulting effective-quote interval is
`157497770361–157497771060` raw units. Applying the observed quote-vault delta
gives post-state bounds `157723425616–157723426315`. Across that interval and
the Coinbase `$86.92–$86.94` range, both spot endpoints are within 1% of the
provider price. For 7DGZ, the selected SELL pre-state is in the provider's
whole-second bucket; millisecond ordering within that second is unresolved.
Its exact CPMM effective-quote interval is `87520405888–87520405894` raw
units. Across that interval and the `$135.73–$135.85` Coinbase range, both
spot endpoints are within 1%.

For both observations, the verified supply at the provider maximum is the
birth supply (`1000000000000000` raw units, 6 decimals); the birth-to-snapshot
burn ledger places all burns after the maximum-price observation. The exact
provider-price FDV floors are `12133750` cents for 5b2 and `5912528` cents for
7DGZ. The review artifact is pinned by SHA-256
`7633ace65d9586828bb14077695450e633331fba85fa0602c184cd27ac6794c9`, alongside
the independent burn reconstruction pinned in Amendment 20.

These checks corroborate price level, not an exact millisecond pool state.
They retain `ath_at=NULL`, unknown provider recording-window scope, and
unknown market scope. They do not establish lifetime price or pool coverage.
The two records may be written only through the existing guarded
missing-only path: preserve every known numeric observation, including genuine
zero, and update only a matching `codex-missing` zero-sentinel row. The source
identifier is
`solana-tracker-window-historical-supply-bounds-pool-fx-validated-v1`.
Fixture `solana_tracker_two_price_gate_cases.json` pins the exact FDV floors,
quote-input and reserve-rounding intervals, supply, timestamps, and all
price/FX endpoint comparisons. The three other cases in the same audit fail
the independent price gate and remain ineligible.

## Amendment 22 (2026-10-11) — empty provider history for a verified zero-swap pool

An empty target-USD candle response for a known initialized pool may be treated
as no observed pool price, rather than an automatic blocker, only when a
complete Swap-log scan from token creation through the pinned anchor proves
that the pool had zero swaps. The pool remains in the known-pool scope; its
empty history contributes no price. An Initialize event's initial square-root
price is state, not a trade, and must not be used to fill the missing candle.
If a scan is incomplete, returns unsupported events, or finds a Swap, this
exception does not apply.

For `0x67d5ab989378bfb2070954989a2f09ca6b19ab07` on Base, the four observed
PoolManager Initialize pools have target-address GeckoTerminal histories except
for USDC pool `0x218492c713cd10484093bf261b85c65702627dbb54a01c57ac3dc60dbe92da7a`.
Its full-range creation-to-anchor scan is complete and contains zero Swap logs;
the provider returned no target-USD candles for it. The other three target-USD
histories include the corrected native-pool target selector. Their provider
window maximum is `$0.000144069791573811` per token on 2026-05-01; the native
pool maximum is `$0.00000816440733742597`. Verified fixed supply of 100 billion
tokens at 18 decimals yields a provider-window FDV floor of `1440697915` cents.

An independent USDC Swap in the maximum's pool on the same UTC day gives an
exact execution-price ratio. Applying that day's closed Kraken USDC/USD
`0.9995–0.9999` low/high range places both endpoints within 1% of the
GeckoTerminal maximum. The same-day Birdeye value is materially lower and
remains an unresolved source discrepancy; do not average it away or claim
cross-source agreement. Under Amendment 15, this supports considering the
GeckoTerminal value as a separately corroborated provider-window observation,
not a global or lifetime ATH. Retain `ath_at=NULL`, unknown provider-window
start and candle-history completeness despite the request bounds, gaps outside
the four observed pools, and the Birdeye discrepancy. Do not infer a price or
timestamp from the empty pool's Initialize state.

The cached GeckoTerminal requests are daily (`aggregate=1`, `limit=1000`,
`before_timestamp=1791676799`, USD, `include_empty_intervals=false`). The
corrected native-pool request selects the exact target address; the excluded
control uses `token=quote`. These request bounds and returned candle timestamps
describe the query and observed rows only. They do not prove the provider has
returned every historical candle or every market. The zero-Swap scan supports
the empty-history exception only for that scanned pool and block range.

The proposed source identifier remains
`geckoterminal-window-supply-swap-fx-validated-v1`; runner gates are unchanged.
Fixture `geckoterminal_base_zero_swap_pool_fdv_case.json` pins the pool set,
zero-swap evidence, exact supply/FDV arithmetic, target identity, and both
independent FX endpoints. The source audit is pinned at SHA-256
`a586689bc26758a2faec06bf66d1b9ce0a118a01c46f0470b0015813404840c5` and
reported in
[`2026-10-11-base-six-triage-67d5-gt-source-audit.md`](../p0/measurements/2026-10-11-base-six-triage-67d5-gt-source-audit.md).

## Amendment 23 (2026-10-12) — two-percent independent price corroboration tolerance

The user authorizes a maximum 2% deviation for independent price corroboration.
This changes only that comparison tolerance. Existing 1% evidence and golden
checks remain historical records; token identity, UTC-day matching, verified
supply, pool and history scope, source quality, and all runner gates remain
unchanged. Compare exact integer/rational values, including both endpoints of
the dated FX range. A value exactly 2% away passes; the next representable
amount beyond 2% fails.

For Base token `0x92d95883e79a695ca4afa40d4885b4ef4ac38b07`, the May 3, 2026
GeckoTerminal returned-pool maximum is `$0.0000100542474772537`. The
receipt-backed USDC execution ratio converted with Kraken's same-day
`0.9997–0.9999` USDC/USD range differs by 1.74–1.76% from GeckoTerminal and
1.78–1.80% from Birdeye. The two provider highs differ by about 0.038%.
The execution observation is only 45 raw USDC (`0.000045 USDC`),
so that small notional remains explicit quality context. The post-swap marginal
spot is a different price kind, about 36% higher, and is not substituted for
execution price. Eight returned PoolManager pools were scanned over their
creation-to-anchor ranges, including four zero-Swap USDC histories; this does
not establish all-market or lifetime price completeness. The independent
corroboration supports a scoped GeckoTerminal provider-window value only;
`ath_at` remains unknown. New guarded writes use source
`geckoterminal-window-supply-swap-fx-validated-v2` to identify the 2% policy;
the old v1 observations keep their original provenance. No automatic
promotion or runner-gate change follows.

Golden `geckoterminal_base_two_percent_price_gate_case.json` pins the exact
candidate inputs and source audit hash, checks both FX endpoints and the
same-day/supply/pool scope, and tests 1.8%, exact 2%, and 2% plus one integer
quantum. Its 1% fixtures are unchanged. Audit:
`.scout-data/diagnostics/base-rh-two-percent-offline-audit-2026-10-12/audit.json`
(SHA-256 `a018509e9cff9cdf9b5aa24eadf7253534a7e7660714400c18d8bdd5e5f2c206`);
report: [`2026-10-12-base-rh-two-percent-offline-audit.md`](../p0/measurements/2026-10-12-base-rh-two-percent-offline-audit.md).

Golden `geckoterminal_base_cross_day_max_tie_price_gate_case.json` records a
second scoped Base window case. Its same-pool May 4 open/high equal the May 3
close/high, so the daily maximum ties across dates; the May 3 receipt-backed
USDC execution remains same-day and passes the unchanged 2% gate at both dated
FX endpoints. The empty-history pool has a complete zero-Swap scan, fixed
supply is retained, the distinct post-swap spot and Birdeye disagreement stay
separate, and `ath_at` remains unknown. Source certificate SHA-256:
`66b194cd41f530eb112199933fa28cc9d14c3dad095c3721a5af2f12f1fc38c7`.

## Amendment 24 (2026-10-13) — selected Zora execution-window candidate

For Base token `0x058e72b9e99561844582b019ab91cdab6c9ac738`, the selected
Zora-indexed edge derives an execution-average candidate from the exact
receipt-verified MATRY quote amount and the API's `priceUsdc` value, divided by
the exact DVA output. A source-specific, conservative 24-decimal floor is
`$0.001506582619089042126709` per DVA; with validated fixed supply of one
billion, the provider-window FDV floor is `150658261` cents. The API quote
amount's `amountDecimal` differs from the exact on-chain quote by
`0.000000000000020574` MATRY units and remains disclosed as precision context.

The same transaction's verified pool-core WETH route gives `0.000000565714882340740481…`
WETH per DVA. Multiplying by the matched Coinbase ETH/USD one-minute range
`$2686.29–$2686.84` places both endpoints within 2% of the candidate. The
Birdeye daily high `$0.0014819050096223831` differs by more than 2% from both
route-derived endpoints; retain the two source observations separately and do
not average them. This establishes
source-specific precedence for this selected, corroborated Zora execution
window only. It does not change the independent 2% threshold or create a
general Zora parser/automatic ingestion rule.

The corroboration is a pool-core route execution average converted with the
dated quote/USD candle. The receipt contains ten core Swaps across four pools,
plus four PoolManager-to-hook WETH transfers and twenty hook payouts totaling
`0.000003823677346108` WETH (3.312% of the external WETH input). There are no
DVA or MATRY ERC-20 Transfer logs in this receipt. These observations do not
establish per-leg fee allocation or final wallet settlement; this price must
not be used as wallet-net acquisition cost or PnL. The supplementary cached
receipt audit does not alter the selected provider-window price or its 2%
corroboration gate. Audit SHA-256:
`e5ff8abcabc2e01305a6c39d79b97e6c60c05170a885600aa5f395c3194a3526`.

The index returned 92 execution edges (91 distinct `priceUsdc` values) while
the same-pool chain scan found 326 Swaps through its anchor. Cursor exhaustion
is not complete market or lifetime coverage; `priceUsdc` units and exact
timestamp semantics remain undocumented. The selected edge supports only a
scoped provider-window value, not a lifetime ATH; keep `ath_at=NULL`. It is
execution evidence, not 92 OHLC candles, and does not replace Birdeye candle
history. Golden `zora_route_fx_window_case.json` pins the same-transaction raw
amounts, exact rational 24-decimal floor, both FX endpoints, supply, and
coverage limits. Certificate SHA-256:
`941113c79a66900a52f079de7cdbcbd3638f2d4fba60c36858e24d8626778b56`.

## Amendment 25 (2026-10-13) — Solana Tracker peak-time supply with daily-high corroboration

Two Solana Tracker maximum-price records pass the existing 2% comparison
against a GeckoTerminal daily high for the same UTC day and token pool. The
comparison is between source-recorded prices, not an execution ratio. Complete
mint-signature and successful full-transaction histories reach each token's
birth, agree on their signature sets, contain no post-birth mint, and reconcile
all burns to finalized snapshot supply. At the respective provider price
seconds, neither token had burned supply: peak supply equals birth supply of
`2000000000000000` raw units at six decimals. The first burns followed 19 and
54 seconds after the respective provider timestamps. Exact price-times-supply
floors at both peak and birth supply therefore agree at `6804467` and `6804889`
cents. This supports those maximum FDVs only within the unknown Solana Tracker
recording window. Keep `ath_at=NULL`; it does not establish lifetime ATH or
complete market coverage.

The BQJF control also passes the same-day daily-high price comparison and has
complete birth-to-snapshot supply history, but burns preceded its maximum-price
second. Its selected peak-time FDV is `834066` cents while the birth-supply
upper endpoint is `1621320` cents. The selected peak FDV cannot be called the
provider-window maximum: an earlier lower price could have coincided with the
higher pre-burn supply. This keeps the peak-time arithmetic distinct from the
provider-scope maximum-FDV proof.

Golden `solana_tracker_window_peak_supply_gecko_cases.json` pins both accepted
cases, the negative control, exact-cent calculations, and price-kind/scope
limits. The source remains
`solana-tracker-window-peak-supply-geckoterminal-validated-v1`; no automatic
price adapter or promotion rule follows. Supply analysis SHA-256
`4e129fb0a33bca1adb62f430cf6ea483d0758d5aee93e3faf828f6297ef8abb7`, history
proof-marker SHA-256
`eb53a2c969301176ae80da21c0f8831087095f0ca58e542a0238f2a647a45791`, and
GeckoTerminal comparison SHA-256
`3635c1313cd7f611fefeb31c901d21002cdfa1f7db67786534d32300894e0e4a`.

## Amendment 26 (2026-10-13) — Base 3607 same-kind marginal spot corroboration

For Base token `0x3607f8f1d16c5ae717b61d28eec1d624df763b07`, the
GeckoTerminal daily high is `$0.000388974286395251` on both May 4 and May 5.
The same-day maximum post-swap marginal spot across the four known token/USDC
PoolManager pools is derived from the exact Uniswap v4 `sqrtPriceX96` state:
`sqrtPriceX96² / 2^192 × 10^(18−6)` USDC per target token. Converting with the
May 4 Kraken USDC/USD daily range `0.9970–0.9999` places both USD endpoints
within 2% of the provider daily high. The independent all-state recheck
contains 15 raw May 4 swaps and 15 eligible positive-liquidity post-states; it
corrects a prior `31` state-point count that was not a raw event count. Include
the one-sided swap state because its post-state has nonzero `sqrtPriceX96` and
positive liquidity even though an execution-average ratio is undefined.

Creation-to-anchor supply evidence records 100 billion tokens at 18 decimals,
one creation mint and no burns through the anchor. Bridge mint/burn capability
exists in verified source but is not an observed event in that interval. The
daily-high price times fixed supply floors to `3889742863` cents. This supports
only source-window FDV and same-kind spot corroboration; retain `ath_at=NULL`
and do not claim all-DEX or lifetime completeness. The same Swap's execution
average is `0.002799987759250452142275286391` USDC per target, about 7.198 times
the provider USD daily high. It is a separate price kind and must not be
averaged with or substituted for the marginal spot comparison. Do not treat
the marginal spot as execution acquisition cost or wallet PnL.

Golden `geckoterminal_base_3607_marginal_spot_window_case.json` pins exact
square-root arithmetic, both FX endpoints, corrected state counts, fixed
supply, and the distinct execution price. Certificate SHA-256
`231fb11c15f6d517d5eeaa694e538d4d62fbcf2de948ad9a55d41569dea747f1`; all-state
recheck SHA-256
`f2747b81e124831a0b0478af14c96907d6e686e9fed0ae570c819d5e86697452`. The
source is `geckoterminal-window-supply-pool-state-fx-validated-v1`; this adds
no automatic parser or general promotion rule.

## Amendment 27a (2026-10-13) — Robinhood direct pool maxima with traded-reference FX

For the explicitly scanned canonical initial pools, define a reference-priced
maximum by converting each successful post-creation Swap event through its
own contemporaneous traded-market reference, then selecting the highest
converted event. This is a source-specific model, not an exact second-level
USD execution price. It does not generalize to other pools, tokens, or FX
sources.

For WETH-quoted token `0x01193b12f91826c2bf313d4b9620e83280a3fee1`, all three
post-creation events are priced with the Coinbase ETH/USD close from their own
UTC minute. The maximum after conversion is block `46,586,365`, log `11`, at
`$0.00001006353488124656040241272196` per token, or `$10,063.53488124656…`
FDV for one billion tokens. The selected minute's low/high sensitivity remains
`$0.00001005229116336928213389850947–$0.00001006406447665382350926302907`
per token. This is a minute-close reference model, not exact fiat execution or
wallet acquisition cost.

For AAPL-quoted token `0x4016fa67c597bf5e1991c852a6dfbb3eb9801e18`, price all
six scanned events using the last successful fee-3000 USDG/AAPL execution
before that event's timestamp group, multiplied by Kraken's closed Aug 4
USDG/USD daily close `0.9999`. Each of the two groups is bracketed by actual
USDG/AAPL executions; their ratios and the daily low/high remain sensitivity
inputs. The converted maximum is block `27,216,237`, log `22`, at
`$0.00002035554268939785137805557743` per token, or `$20,355.54268939785…`
FDV for one billion tokens. Preserve the separate sensitivity range
`$0.00002035147117370840286292845360–$0.00002035820501954546327338853483` per
token. USDG is priced through the observed market route and dated Kraken
reference; no 1:1 peg or stale AAPL oracle is assumed.

The third candidate `0x48c01e934d149b54af69398a3f767b7e399fb4aa` remains a
negative control: available USDG/AAPL executions do not form a sufficiently
timed two-sided bracket around its pool maximum. No reference-priced maximum
is assigned without that quote evidence. The two selected models use sources
`onchain-pool-window-supply-cex-reference-v1` and
`onchain-pool-window-supply-stock-trade-fx-reference-v1`; both retain
`ath_at=NULL` and cover only three or six enumerated events in their initial
pools through the pinned head. Neither establishes alternate-pool or lifetime
completeness. The model output is a maximum over per-event FX-weighted values;
do not select a native quote-ratio maximum first and convert it afterward.
Keep the separate market-sensitivity bands intact rather than averaging them
into the reference point. In this three-event WETH sample, the highest native
ratio happens to remain the winner after per-minute weighting; this is not
evidence of a ranking reversal.

Golden `robinhood_scoped_reference_maxima_case.json` pins all nine event ratios
and per-event FX models, winner identities, supply, uncertainty bands, and the
unbracketed negative control. Capture review SHA-256
`06e56f774c800cb8e7539159ddc1b6cdc9a2e82d6b33ca495e98a001abed3399`. This
amendment defines only these two manually reviewed reference models; it adds
no general adapter or automatic source promotion.

## Amendment 27b (2026-10-14) — Robinhood Doppler pool maxima with per-minute ETH/USD

Apply the same event-weighted reference model to three additional Robinhood
initial pools. For each post-creation Swap, calculate the exact absolute
WETH/token raw-delta ratio (both assets have 18 decimals) and multiply by that
event's Coinbase ETH/USD UTC-minute close. Select the maximum only after all
80 enumerated events are individually converted. The one-billion-token supply
at 18 decimals gives these conservative FDV-cent floors:

- `0x3d5733d2ac4d8f572aa22531e84bf620cc98fee1`: 60 events across 25 blocks;
  maximum at block `46,091,504`, log `34`; `3,248,630` cents.
- `0xcc799821eba30921d0f387f846d36cb6a0cafee1`: 10 events across 5 blocks;
  maximum at block `46,871,356`, log `32`; `1,226,265` cents.
- `0xd34b6aa31674b581ac37766198ce3a9350a3fee1`: 10 events across 7 blocks;
  maximum at block `46,140,551`, log `94`; `3,476,092` cents.

Each scan starts strictly after its verified creation receipt and ends at the
pinned head `87,808,917`; each selected event has a confirmed `10^27` raw
supply at 18 decimals. These three event-window models use
`onchain-pool-window-supply-cex-reference-v1`. Retain `ath_at=NULL`: this
evidence covers only the listed pool events through the anchor, not all-market
or lifetime history, exact second-level USD execution, or wallet acquisition
cost/PnL. The selected event's minute low/high remains sensitivity data and is
not averaged into the close reference point.

Golden `robinhood_doppler_no_v4_reference_maxima_case.json` contains compact
raw event tuples for all 80 Swaps and the Coinbase candle minutes they map to;
the test derives every exact price and winner rather than trusting summary
flags. Follow-up review SHA-256
`f82812ccee80ce7ffcd974fb1343fe40155da83447484cc5e551263f5261c8a5`, raw
Swap capture SHA-256
`667c00d5b5ae020d93735556947e6cb2d0d003ee6d8a7f0ccdc68ff5386d3ffe`, and
Coinbase minute evidence SHA-256
`cd2b70bf6ea7cba494b4634dd5303ab35fd92c38754355009a482c86e8bc8113`. This
adds no automatic adapter or general source promotion rule.

## Amendment 27c (2026-10-14) — Base e465 indirect marginal-spot reference

For Base token `0xe46504da7f7f9d9233d6118e27bdfbf27964a215`, the scoped
PoolManager scan begins at the verified creation block `0x314c7c0` and ends at
anchor `0x320a267`. It contains one successful Swap in the configured target
pool, at block `0x3164990`, transaction index `98`, log `366`. The receipt is
successful and its target Swap log matches the archived pool scan. The
creation-to-event supply evidence records one `10^27`-raw mint (one billion
18-decimal tokens), no burns, and constant supply through the event.

The selected value is an indirect marginal-spot reference, not the Swap's
execution average. The target pool's post-Swap currency0/currency1 state gives
quote-token per target-token from the inverse squared `sqrtPriceX96` ratio.
Within the same transaction, log `353` in the quote/ZORA pool precedes the
target log `366`; its verified currency order gives ZORA per quote-token.
Multiply those ratios by the end-of-event-block ZORA/WETH marginal spot from the
fee-3000 Uniswap v3 pool, then by the Coinbase ETH/USD close for the event's
UTC minute. The resulting reference is about `$0.0000001126` per target token,
or `11260` FDV cents floored for the fixed supply. The minute low/high produce
separate floors of `11257` and `11264` cents. A second positive-liquidity
fee-10000 ZORA/WETH pool gives `11296` close-reference cents, about `0.31854%`
from the primary result and within the existing 2% corroboration bound; it is
corroboration, not an average or replacement. The selected target event's
execution-average estimate remains separately recorded as a distinct price
kind; its value is slightly higher than this marginal-spot reference.

This is one configured target-pool event with a composed quote route and two
parent-pool observations. GeckoTerminal returned no daily candle for the
token. Keep `ath_at=NULL`; this does not establish all-market or lifetime ATH,
an exact USD execution price, wallet acquisition cost, or PnL. Its source id is
`onchain-pool-window-supply-indirect-spot-cex-reference-v1`. Golden
`base_e465_indirect_spot_reference_case.json` pins the receipt, route/order,
spot arithmetic, FX range, supply, and scope. Candidate SHA-256
`888e884399a2d0c09e390f3c9710245595c729706666ac7ac6472c4fc4cee03a`, receipt
recapture SHA-256
`6233f14b75e01bfb109fe944e9691b5b9df4730e9f1977db28558de3a765b74f`, and
independent recheck SHA-256
`56b2a7f03e4c64d6b3eb1c2dc8982d3e81aa4546cf5b7b69d34ea96909d5441a` identify
the reviewed evidence. This amendment adds no automatic adapter or source
promotion rule.

## Amendment 27d (2026-10-14) — Robinhood 48c follow-up with traded USDG/AAPL references

The prior 48c snapshot remains an accurate record of its then-incomplete quote
evidence, but a pinned follow-up now supplies actual pre-event USDG/AAPL trades
for each of the two target-event time groups. Apply the existing 27a per-event
model to all 15 successful Swaps (10 in block `22,791,875`, five in
`22,792,033`): absolute AAPL/target raw-delta ratio, multiplied by that
group's last actual USDG/AAPL trade before the events and Kraken's July 29
USDG/USD daily close `1.0000`; select the maximum only after converting every
event. The group-one reference trade is block `22,790,408`, log `4`; its
bracketing follow-up trade is block `22,791,960`, log `29` (148 seconds before
and 8 seconds after the target group). Group two uses that latter trade as its
pre-event reference and is bracketed by a same-pair fee-500 trade at block
`22,792,331`, log `19` (7 seconds before and 30 seconds after). The different
fee tier in the second group's trailing quote is retained in the sensitivity
range, not averaged into the reference point.

The maximum is the first group's block `22,791,875`, log `129`, at
`$0.00006143395877629962…` per token; with one billion 18-decimal tokens this
floors to `6,143,395` FDV cents. The bracket and daily FX low/high sensitivity
is `$61,427.81538…–$61,448.49062…` FDV. This is a traded-reference model, not
an exact second-level fiat execution price. The source remains
`onchain-pool-window-supply-stock-trade-fx-reference-v1`, with `ath_at=NULL`;
the evidence covers the listed pool events only, not all markets or lifetime
history. The historical negative-control snapshot is preserved, while this
follow-up supersedes its conclusion that the scoped quote evidence was
insufficient. Golden
`robinhood_48c_usdg_aapl_followup_case.json` includes all 15 raw event tuples,
group references/brackets, exact maximum arithmetic, supply, and the prior
snapshot's status. Candidate SHA-256
`988799f1fd6b5a105eacbad67e5698109378467c3519f617fa08147989c080c4` and Kraken
USDG/USD candle SHA-256
`2c3222370fb29cf678eb33bc08fd0112fc1a175a2e5ec78e38c069987b460517` pin the
reviewed inputs. This remains a manual source-specific case with no automatic
adapter or promotion rule.

## Amendment 27e (2026-10-14) — Robinhood WETH-reference pool-window follow-up

Two additional configured initial Uniswap v4 pools have manually reviewed
WETH-reference maxima under the existing 27a event-weighted method. For every
successful post-creation event in each scanned pool, calculate the exact
absolute WETH/target raw-delta ratio and multiply by that event's own Coinbase
ETH/USD UTC-minute close, then select the maximum. The target and WETH both
have 18 decimals, and the selected event's one-billion-token supply is
`10^27` raw units from its creation mint; no burn appears through the scanned
window end.

- `0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1`: three post-creation events in
  the configured pool, maximum block `45,582,061`, log `107`, at approximately
  `$0.0000103730435063` per token; one-billion-token FDV floors to `1,037,304`
  cents. Across these events the minute low/high sensitivity spans about
  `$10,361.81–$10,377.20` FDV.
- `0xc3a0e1a6c10b97c3e124ff8a2c291e5677a6fee1`: two post-creation events in
  the configured pool, maximum block `49,435,080`, log `41`, at approximately
  `$0.0000102116903886` per token; FDV floors to `1,021,169` cents. Its
  event-window minute sensitivity spans about `$10,211.15–$10,219.67` FDV.

Both scans start after the verified creation block and stop at pinned head
`87,808,917`; creation-transaction Swap logs are excluded. Each selected
event's receipt/log and block-header timestamp are verified, and its own
matching candle is retained. The source is
`onchain-pool-window-supply-cex-reference-v1`. These event-window references
are not exact fiat execution prices, alternate-pool coverage, lifetime ATH, or
wallet acquisition cost/PnL; retain `ath_at=NULL`.

Golden `robinhood_weth_reference_followup_case.json` adds the raw tuples for
all five events to the exact-rational test already covering 80 Doppler events,
and recomputes both winners and cent floors. Candidate review SHA-256
`3eb7ff79c93912eff0c03676589d359891e81d54e7d24f08ec04b519ce8b224e`, shared
Swap capture SHA-256
`667c00d5b5ae020d93735556947e6cb2d0d003ee6d8a7f0ccdc68ff5386d3ffe`, and
matching Coinbase candles
`610c07d496f00477c8536c7cee0eb38ece3d164b51204ee3e6104145b0df0801` and
`040e86ae2aaedb31e96ae316337c8575b6149bc082fb9469b24e6607d5067a8e` pin the
reviewed inputs. For aa539, `totalSupply()` and `decimals()` were directly read
at the last included swap block `45,582,406`, later than the winning block
`45,582,061`; the complete birth-through-last mint/burn scan (one mint, no
burns) proves the one-billion supply also held at the winner. Decimals were
also independently read at the winning block. Root verification SHA-256
`7f55bcbc64b630602bd0448b6ee88a02a15485f18ac52e1843714f1afa9653e5` and the
aa539 full-window supply manifest SHA-256
`6b89d834968823f75913f11524ada44a35ac3150a021b18c2b83d3b7a5793958` pin this
distinction. This manual extension adds no automatic adapter or source
promotion rule.

## Amendment 27f (2026-10-14) — Robinhood NVDA-reference pool-window follow-up

The configured Robinhood pool for `0x1e6489ef6d75c6f785843669b01262a7562b1e18`
has three successful post-creation target events. Convert each event's exact
target/NVDA raw-delta ratio through the preceding NVDA/USDG v3 trade in the
same transaction where available, then through the event's dated Kraken
USDG/USD daily close; select the maximum only after converting all events.
The winning block `17,403,416`, log `12` has a same-transaction quote route
whose NVDA output exactly matches the target event's NVDA input. Its reference
price is approximately `$0.0000202037022883` per token, or `2,020,370` FDV
cents for one billion tokens. The one-birth-mint, no-burn supply check extends
through the last scanned event at block `17,404,579`.

The source remains
`onchain-pool-window-supply-stock-trade-fx-reference-v1`, with `ath_at=NULL`.
This is a configured-pool/event-window reference model, not exact fiat
execution price, wallet cost basis, all-market coverage, or lifetime ATH.
Golden `robinhood_nvda_reference_followup_case.json` checks all three event
conversions, the same-transaction route, supply, and cent floor. Candidate
SHA-256
`77d19011f3c103ae03c90bde88264cb5744f2fb64ee8aaead04a82c600bf7e97`, supply
manifest SHA-256
`870cfbf0d02cd39788e4e15229782ff9dbd70131d89bfaa8a735f0664994a65c`, and
Kraken candle SHA-256
`ed81dd2a05a92a744a1c6d1da01d65fdcdb8dd795ea50ecca401577a40db1ce7` pin the
reviewed inputs. This manual case adds no automatic adapter or promotion rule.

## Amendment 28 (2026-10-14) — BRkf legacy PumpSwap spot and provider-window supply bound

For Solana token `BRkf1C38Y8ZKt1sApadJDP1EAGEFrbtqZbQ1kwSupump`, Solana
Tracker reports a recorded-window maximum spot of
`0.00012703138168442376` USD at `1760342677.288`. A successful Buy in the
selected PumpSwap pool is timestamped `1760342676`, 1.288 seconds earlier.
Its pre-event pool-owned vault balances are `165088137533041` target raw units
at six decimals and `106619718925` WSOL raw units at nine decimals. The exact
reserve ratio multiplied by the Coinbase SOL/USD close `196.50` for that
minute gives a nearby marginal spot of about `0.00012690660323531349` USD per
token, 0.0982% below the provider price. Applying the minute low/high
`196.35–196.81` keeps both endpoint comparisons within 2%. This corroborates
the price level near the reported maximum; it does not identify the exact pool
state at the provider's millisecond timestamp. The check pins the nearest
preceding and earliest following successful pool states around that timestamp;
both occur in the same FX minute and also pass the endpoint comparison.

The complete birth-to-finalized-snapshot mint history contains 764 signatures
and 764 matching full transactions, including one birth mint of
`1000000000000000` raw tokens at six decimals. The 27 burns sum to
`603228892359` raw units and reconcile to finalized supply
`999396771107641`; none precedes or overlaps the provider-maximum second, and
no later mint appears in the complete history. Thus the selected peak has
birth supply and attains the maximum price-times-supply bound within Solana
Tracker's recorded price scope: `12703138` cents. This proves neither all-pool
nor lifetime ATH; retain `ath_at=NULL`.

The selected-pool model uses observed real-reserve vault balances and the
legacy fee/curve interpretation. Sample Buy and Sell reserve fields, curve
amounts, and fee components reconcile. Three official PumpSwap IDL snapshots
(`7645c16c`, `33590255`, `ff9cbf82`) omit `virtual_quote_reserves` from the
Pool and BuyEvent schemas. This supports the legacy interpretation for this
pool and time only: there is no deployed-binary capture, and absence of that
field does not establish a general `virtual_quote_reserves = 0` rule.

The source is
`solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1`. Golden
`solana_tracker_brkf_legacy_pool_spot_case.json` derives the raw-reserve spot,
both FX endpoint comparisons, exact cents, supply bound, and schema-version
limits. Candidate SHA-256
`b394638cf3d4bcce90551ef13b61f9b8a10107f7af3c3a1ac2ba30e9d391004e`,
full-history verification SHA-256
`afd16ba8c1b736c3646c4164d043b0ee4315ce911dffc766c205ad9898502c02`, and
pool-state transaction capture SHA-256
`ce9b72bd1392dd8103942700fb4066c3d80857e933e30652c0530138b3a8d792` pin the
reviewed inputs. This is a manually reviewed source-specific case, not an
automatic decoder or promotion rule.

### Avx legacy PumpSwap follow-up

The same narrowly scoped interpretation is independently checked for
`Avx71ncfuZBMJxxmVqHLsy34RYzVoHTrwD26HjQVpump`. Solana Tracker's recorded-window
maximum is `0.00007372003307230689` USD at `1765803131.144`. The nearest
preceding and earliest following successful swaps in the selected legacy
PumpSwap pool yield close-referenced marginal spots `0.00007373331466016226`
(+0.018016%) and `0.0000732802454296758` (-0.596565%). Both anchors share the
Coinbase SOL/USD minute and pass the 2% check at both minute low and high. Six
successful pool states have second-resolution timestamps equal to the provider
second and are all lower at the candle close; they cannot be ordered against
the provider's millisecond timestamp, so this does not claim an exact state at
that instant.

Birth supply was `1000000000000000` raw units at six decimals. One raw unit was
burned before the provider timestamp, leaving peak supply `999999999999999`;
complete ascending/descending signature-index coverage and full transaction
checks establish no later mint and a nonincreasing supply bound through the
finalized snapshot. The provider price times both birth upper-bound supply and
exact peak supply floors to `7372003` cents. The reviewed legacy real-reserve
model is supported by historical IDL snapshots for this pool/time, not a
deployed-binary capture; it does not establish a universal rule that a missing
virtual reserve is zero. Golden
`solana_tracker_avx_legacy_pool_spot_case.json` pins the raw balances, FX
endpoints, supply evidence, same-second ambiguity, and source limitations.
Candidate SHA-256 is
`837a8826283825c011dc62cc97244e25c5428c691c35e31a0cb8485dd2635578`.
This is another manual source-specific validation, not an automatic adapter,
all-pool/lifetime ATH claim, or database promotion.

An additional successful Avx control transaction exercises the distinct
`buy_exact_quote_in` instruction variant. In that observed variant,
`quote_amount_in` is gross (`79200000` raw) while `user_quote_amount_in` is net
(`78260868` raw); the 20/5/95-bps LP/protocol/creator fees round up separately
to `156522`, `39131`, and `743479` raw units and reconcile gross to net plus
fees. With pre-state base/quote reserves `179841645839855` and `97888658987`,
the net-input formula
`floor((net - 1) * base / (quote + net - 1))` reproduces `143666486066` raw
base output. This is a variant-specific observed mapping; do not apply the
gross/net field assignment to the separate `buy` variant. The official Pump
documentation gives the same `-1` constant-product equation for the pool leg
of `buy_exact_quote_in_v3` and states that this pool leg uses PumpSwap buy
math: [Synthetic Migration: The Last Buy on the Bonding Curve Has No Max Size](https://github.com/pump-fun/pump-public-docs/blob/main/docs/SYNTHETIC_MIGRATION.md).
Golden `solana_tracker_avx_legacy_pool_spot_case.json` pins this control sample
and checks the curve and fee arithmetic exactly.

## Amendment 29 (2026-10-14; corrected) — 88pqb dated legacy pool spot and supply bound

For Solana Tracker's recorded-window maximum for
`88pqbGiXSTb9UpNuSKegCBBEa23vaneN8R9EkKJhpump`, the reported price is
`$0.00000960624821632361` at `1775781520.246`. In the configured PumpSwap pool,
the nearest preceding successful Buy and earliest following successful Sell
are at `1775781519` and `1775781522`. **Correction:** the earlier
interpretation of the final 16 bytes as signed `virtual_quote_reserves` is
withdrawn. The selected 439-byte `BuyEvent` and 376-byte `SellEvent` decode
completely under the official 2026-02-17 IDL at commit `82dacacf15`; the final
fields are `cashback_fee_basis_points` and `cashback`, both zero. The selected
price model uses the actual pre-swap quote-vault balance and applies no
virtual-reserve adjustment. This dated IDL does not prove the deployed
binary's activation date or establish a general default for missing fields.
The exact fee, curve, and pool-vault deltas reconcile for both events. The
pre-state real-reserve ratios, normalized for six-decimal target and
nine-decimal WSOL and multiplied by the event-minute Coinbase SOL/USD close,
are within 2% of the provider price; both candle low/high endpoints also
remain within 2%. These are nearby pool states, not the exact state at the
provider millisecond.

The complete 191-transaction birth-to-peak window matches its signature index,
has revoked birth mint authority and no close authority, and records no mint
or burn affecting peak supply. The verified supply is `2,000,000,000` tokens
at six decimals, so the provider price yields an FDV floor of `1,921,249`
cents. Keep the reported `$9,606.25` market-cap value distinct; it is not the
FDV calculation from that supply.

The source identifier is
`solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1`, and
`ath_at=NULL`. The dated IDL contains cashback fields, not
`virtual_quote_reserves`; the old signed-suffix interpretation is superseded.
The evidence supports only this selected pool and recorded price window, not
all-pool or lifetime ATH. Golden
`solana_tracker_88pqb_pool_spot_case.json` pins the dated event layout,
cashback fields, exact legacy reserve math, pool identity, FX endpoints,
supply, and scope. Candidate SHA-256
`ff7c37ef3c72ebac3af380f964de6eac86cf6724213040904756ec61fe8b82e4`, event
validation SHA-256
`271c1ff08e79a60f2f7ccb54dd9e4e42d8b4d3eaedee1c8cf81e0f24a2f251ba`, and
Coinbase candle SHA-256
`ec80a23421b0c40a0ea6503679543cbdfc0e8d08c751926e1b14b1c0124e5011` pin the
reviewed inputs. That original independent proof retains the now-withdrawn
suffix interpretation. The corrected independent dated-layout check is
separately pinned by SHA-256
`9248a86e615262e62b70959514d19f206cf9bdc07d3d0f744a0d77e48d8edfb7`; do not
substitute the original proof for it. This manual case adds no automatic
decoder or database promotion rule.

## Amendment 30 (2026-10-14) — Robinhood AMD oracle-reference pool window

For token `0x52df349d941a790762a7474244a276ac1f371e18`, three successful events
in its configured initial Uniswap v4 pool are valued individually as the
absolute AMD/target raw-delta ratio multiplied by the latest-before-event
AMD/USD Chainlink round, using eight feed decimals. The selected winning
event is block `17,438,576`, log `28`, at approximately `$0.0000202951885` per
target token; with the verified one-billion-token supply, its FDV floors to
`2,029,518` cents. `totalSupply()` and `decimals()` were read at the winning
block, and complete birth-through-last-event mint/burn history establishes
that the one-billion supply did not change afterward.

The AMD feed round used at each event is positive, has matching nonzero
`roundId`/`answeredInRound`, and is 149–169 seconds old. The following updates
arrive 11–31 seconds after the event; the oracle-answer endpoints
`$544.875–$548.71805` differ by about 0.705% of the lower value. These are
dated oracle-reference bounds, not an AMD/USD market trade. Robinhood's
[oracle documentation](https://docs.robinhood.com/chain/oracles-and-price-feeds/)
states that the stock-token feed returns the per-token value with the
`uiMultiplier` already incorporated. Historical `uiMultiplier()` reads at
both the winning and last target-event blocks return `1e18`; do not apply it
to the feed answer again. A TSLA comparison with a 7,054-second-old round is
rejected by the 300-second freshness check. The standard USDG/AMD fee-tier
probe was limited to three tiers and does not prove that no other market
exists.

The selected source is
`onchain-pool-window-supply-stock-oracle-reference-v1`, with `ath_at=NULL`.
It covers this configured pool and three recorded events only, not all venues,
lifetime ATH, a direct USDG execution price, or wallet acquisition cost.
Golden `robinhood_amd_oracle_reference_case.json` checks each event's exact
ratio and round age/identity, reevaluates the maximum and cent floor, and
retains the stale-feed negative control. Candidate SHA-256
`332df5d9d53f05722a2f899c9a6be3e97effbf9a060a4608aa16b281151c7f2b`, adjacent
round sidecar SHA-256
`0e98414594d9ece1bde4bc8b08d3e8b6d1e480e1af302a8f4c50baaf7cf0e9d2`, and
historical multiplier manifest SHA-256
`e842608793f64047ad121a99b5fa42a1add6dc58b5ef4d2e75b5cfcb72607c12` pin the
reviewed inputs. This is a manual source-specific validation with no
automatic adapter or promotion rule.

## Amendment 31 (2026-10-14) — Robinhood 60d63 and TSLA quote-trade references

For `0x60d63f32f0520178eabf4045cfb6e78941901e18`, value each of the 37
recorded target-pool events from its exact target/quote raw-delta ratio, the
same quote asset's own USDG-market swap immediately before the event, and that
day's Kraken USDG/USD close. Both quote-market observations are ordered around
the target event, no more than 300 seconds away, and their implied quote/USD
rates differ by at most 2%. The maximum is block `65,505,526`, log `107`, at
approximately `$0.00003000729` per token; for the verified one-billion-token
supply, the FDV floor is `3,000,729` cents.

For `0x9aadb292cd6e17cdf98ad16d1a1caf1704041e18` (TSLA), the same calculation
covers six events. The selected quote-trade observations are 353/204 and
370/187 seconds before/after for the two older groups; their implied quote/USD
rates differ by `0.03657016%`, within the existing 2% agreement limit. This
selected case uses the source-specific 600-second age limit: all six events
fail the existing 300-second limit, and the 600-second allowance does not
change any other source policy. The maximum is block `21,054,458`, log `43`,
at approximately `$0.00002027889` per token, yielding a `2,027,889` cent FDV
floor. A TSLA Chainlink observation 7,054 seconds old remains a rejected
negative control.

For both cases, calculate all event prices before selecting the maximum;
quote and USDG decimals are 18 and 6 respectively. The one-billion-token,
18-decimal supply is observed at each case's last recorded event, while the
captured birth-to-last mint/burn history records one birth mint and no burns.
These source-specific windows have `ath_at=NULL` and do not establish all-
market or lifetime ATH. Golden
`robinhood_60d63_tsla_quote_trade_followup_case.json` pins all 43 event ratios,
reference ordering and ages, neighbor-rate agreement, FX endpoints, maximum,
cent floor, supply, and scope. Candidate SHA-256
`f321bf77d62c8a7969c9169c407567b36e85b65523069819eb427d1059eacde0`, target
swap capture SHA-256
`667c00d5b5ae020d93735556947e6cb2d0d003ee6d8a7f0ccdc68ff5386d3ffe`, RPC
manifest SHA-256
`a1caf1fb9c06b434143b7a6df016dd5cd423d433f157fd5c5a678694db020ab3`, and
Kraken manifest SHA-256
`fe9d2f30aaec6a77be4e5a2758a36416a6fdf32170ade092f079dfb503ae1d93` pin the
reviewed inputs. These manual validations add no automatic adapter or
promotion rule.
## Amendment 32 (2026-10-14) — 5b2Bs dated legacy event revalidation

For `5b2BsBEVvsjBegVaKik9vByFJ8YGcDbk8R3A96RTpump`, the selected
provider-window price is `$0.00012133750413201461`. The nearest prior
successful pool event is a 439-byte `BuyEvent` at `1773522435`; the first
following event is a 376-byte `SellEvent` at `1773522438`. Both are fully
decoded under the official 2026-02-17 PumpSwap IDL (`82dacacf15`, SHA-256
`5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6`). Their
trailing fields are `cashback_fee_basis_points` and `cashback`, both zero.
There is no virtual quote-reserve field in this dated layout, and this
selected legacy calculation does not create a generic missing-field default.

The buy event is `buy_exact_quote_in`; its curve output is checked with
`(net_quote - 1) * base_reserve / (quote_reserve + net_quote - 1)`. The sell
curve output, gross/net quote amounts, ceiling fee calculations, and both
pool-vault balance deltas reconcile to the recorded events. Using the actual
pre-swap quote-vault balance, six-decimal target, nine-decimal WSOL, and
Coinbase SOL/USD minute low/high of `$86.92–$86.94`, both strict bracket
prices remain within 2% of the provider value. The complete captured
birth-to-peak window has 429 successful transactions; its 428-row successful
signature-index set plus the validated birth signature matches that set. It
contains one birth mint and no burn or unknown target opcode. Peak supply is
`1,000,000,000` tokens at six decimals, producing an FDV floor of `12,133,750`
cents. A finalized current-snapshot supply of `999,999,912,053,342` raw units
at slot `456603633` floors to `12,133,749` cents, preserving the distinction
between the historical peak supply and the later current balance.

The existing source remains
`solana-tracker-window-historical-supply-bounds-pool-fx-validated-v1`, with
`ath_at=NULL`. This revalidation strengthens the already-known 5b2Bs record;
it is not a new candidate or a new numerical database write. The historical
IDL establishes the selected payload layout, not the deployed binary
activation date or a universal rule for other unrecognized layouts. Golden
`solana_tracker_5b2bs_legacy_pool_spot_case.json` pins raw event hashes and
decoded values, curve/fee/vault arithmetic, FX endpoints, supply, and scope.
Proof SHA-256 `16ed533ead71dd4b173196c879ca49acd71d7c7cace8269f4381547d962ef95e`,
selected transaction capture SHA-256
`0f5641c6062b76e80097ecb434e2579c49140010ac433cdb33e6c05060265f87`, and
dated IDL SHA-256
`5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6` pin the
reviewed inputs. The finalized-account capture SHA-256
`a5fd4460378d62d1dd4bbed38f06e95069340c37e2ff13219efa69927985942c` pins the
current-supply comparison. Independent arithmetic recheck SHA-256
`bf6a6a2200cc3846154eebbb13e6e12d73dd0f85f9853d6ad5929b4969581fa1` pins the
revalidation. This manual validation adds no automatic decoder or
database-promotion rule.

## Amendment 33 (2026-10-14) — conservative maximum FDV with one-cent uncertainty

For `8ihYkBBPHu86isqxU4CnNQdbixqtbRcMcicxTxYYpump`, preserve the distinction
between exact supply at the provider price peak and the maximum of price
times supply throughout the provider window. The complete birth-to-peak
history contains 838 successful transactions: 837 successful signatures plus
the explicitly excluded birth signature. One birth mint issued `1e15` raw
units; five burns before the peak removed `92,734,328` raw units. At six
decimals, the peak supply is `999,999,907.265672` tokens. Mint authority was
revoked at birth, and the finalized mint has no close authority or
supply-scaling extension, establishing a nonincreasing supply bound.

The observed peak price is `$0.00010435468782714595`. Its peak-supply product
is a lower bound on maximum provider-window FDV; its birth-supply product is
an upper bound. Their exact difference is less than one cent, with cent
floors **10,435,467–10,435,468**. Under the owner's accepted 1–2% tolerance,
this selected case stores the conservative lower value. Arithmetic and
threshold comparisons remain exact; this does not claim the true maximum
cent is known. The source is
`solana-tracker-window-peak-supply-cent-tolerance-legacy-pool-fx-validated-v1`,
with `ath_at=NULL`. Do not apply this exception to wider intervals or zero
lower bounds, and do not relabel the result as lifetime/all-market ATH.

Both adjacent PumpSwap states decode completely under the dated January IDL,
match the actual pool vaults, reproduce curve/fee amounts, and agree with
both dated SOL/USD minute endpoints within 2%. Golden
`solana_tracker_8ih_legacy_cent_bound_case.json` uses captured event-CPI bytes
through the production decoder, exact interval arithmetic, and negative
controls for two-cent uncertainty, zero and reversed bounds. Independent
proof SHA-256 `33a7a7a8cab70395ea09d15508b11c12375cd1488e2cff993ceb1eaf8deb2463`
pins the reviewed inputs. This is a manual source-specific validation;
runner thresholds and automatic adapter rules are unchanged.

## Amendment 34 (2026-10-14) — selected native-SOL Mayhem curve marks

Three selected Solana Tracker price maxima for `DwzNzHsH…`, `CemXM4du…`
and `AkKQKnhP…` have `pool_id == mint`; this is a provider association for
pump.fun bonding curves, not a PumpSwap pool account. Their finalized
standard transactions contain successful Mayhem-router calls and nested
Pump `buy` instructions with the official discriminator, exact mint account
and emitted TradeEvent. Actual native lamport debits and token balance
increases equal the selected event amounts. The historical payloads end at
cashback fields; later quote/reserve fields are absent, not zero.

The official Pump SDK 4.1.0 defines its fee-tier market-cap mark as
`virtualQuoteReserves * mintSupply / virtualTokenReserves`; Mayhem uses the
actual mint supply. For native SOL, reserve units are lamports and raw token
units. This supports a selected **curve-mark reference**, not execution-average
price, wallet acquisition cost or an identified Data API pricing algorithm.
Mayhem can explicitly update virtual reserves: ordinary constant-product
trade deltas must not be used to reject or reconstruct such updates.
The full Mayhem state-transition schedule is not proven here.

The complete captured ±3-second windows contain respectively 3, 14 and 10
TradeEvents. All event-CPI bytes match emitted payloads in the actual Pump
program log context, with the exact mint and successful transaction. Compute
the maximum virtual-reserve ratio over every captured state before comparing
both dated SOL/USD minute low/high endpoints with the provider price; do not
pick a matching state while omitting a higher one. Each selected maximum
occurs one second before the provider timestamp and agrees at both FX
endpoints within 2%. This neighborhood check is corroboration, not proof of
all-market annual coverage.

Complete birth-to-peak histories contain 17, 25 and 181 successful transactions,
matching finalized signature-index sets plus the explicitly excluded birth.
Each birth has two `MintTo` instructions of `1e15` raw units, producing 2B
six-decimal tokens, followed by mint-authority revocation. No burn occurs
through the peak, and the observed mint extensions have no close authority
or supply-scaling feature. Price-times-peak and price-times-birth maximum FDV
bounds therefore have identical cent floors: **1,430,309**, **1,567,048** and
**5,643,677**. The provider-reported market caps use approximately 1B and are
not copied into the FDV ledger.

The selected source is
`solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1`, with
`ath_at=NULL`, existing holders/liquidity preserved, and an unknown provider
recording window. Golden `solana_tracker_mayhem_curve_mark_cases.json` exercises
captured bytes through the production parser and exact mark/FX/supply math.
The independent price, supply and native-execution proofs have SHA-256 values
`2607199ac0d7398223c5925aa76ca059cb77a8e60c5df7f0328eed2679fcdb4f`,
`b3688fe3ff869e23f078215fe65ab9ba16e9db1fe9a74113fdac2e2a29bd5b54`
and `fdec592f71aded401a17f542f7f72780968bbd8037a6c4f4be17083b0590b158`.
This manual validation adds no automatic adapter/promotion rule and changes
no runner threshold or selection definition.

## Amendment 35 (2026-10-14) — Mayhem marks with separate native fee legs

Seven further selected Mayhem curve cases use amendment 34's mark/source
semantics. Complete ±3-second windows contain 36 TradeEvents in total. The
global maximum in each captured window occurs one or two seconds **before**
the provider timestamp; both dated SOL/USD minute endpoints agree within 2%.
The observed two-second offset is retained explicitly; the earlier three
cases' one-second offset was an observation, not proof of exact provider
clock alignment. No higher captured state is omitted. Two negative controls
have earlier matching states but later higher marks differing by 6–17%;
selecting only the matching earlier state would be unsupported. They remain
unpromoted, as does the separately captured PumpSwap candidate.

Birth-to-peak successful history counts are 2/2/1/2/4/2/5, each reconciling
with the finalized signature query plus explicit birth. Each mint has two
birth `MintTo` instructions totaling `2e15` raw units, authority revocation,
no burn through the price peak, and an observed Token-2022 mint with only
metadata extensions 18/19. Thus the selected provider maximum FDV cent floors
are 500,586 / 487,004 / 522,541 / 703,446 / 499,152 / 546,571 / 642,711.
Provider market-cap fields with a nominal 1B denominator are not reused.

The selected successful standard transactions bind actual Pump `buy`
instructions and mint/native-SOL account roles. Some are direct Pump calls;
one is a nested Mayhem-router call. Native System transfers are scoped to
the buy invocation using instruction group/order and stack depth. Transfers
into `bonding_curve` equal the event `sol_amount`; fee-recipient transfers
match `fee`. Ceiling rounding of the event's SOL amount and fee basis points
reproduces protocol and creator fee fields. An individual creator-vault
transfer matches `creator_fee`; the `5N9Zfj9t…` transaction also transfers
890,880 lamports into that vault. This extra amount is retained separately,
without calling it creator fee or assigning it a rent purpose without a
separate proof. Fee-payer balance changes also include transaction fees,
account funding and other flows; they are not substituted for event quote
amount or for the curve mark. No PnL/trader attribution rule changes here.

The source remains
`solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1`, with
`ath_at=NULL`, preserved holders/liquidity and unknown provider recording
window. Golden `solana_tracker_mayhem_curve_mark_fee_cases.json` exercises
captured event-CPI decoding, maximum selection, exact FX and supply math,
raw System transfer amounts, fee rounding and the two non-cherry-picked
negative controls. Independent price, supply and native proofs are pinned by
SHA-256 `f3b4ad42d145575ccdb25c425333a69a24908e866b2b86a1858b216493326b2b`,
`4bde343662cc8e4693b9fff64a3ee72bb4718bf39d9476713f724909ab03a31a`
and `5b1f92f85e35462704a2671f6657549fb9598a41c2213497f48ed1396a93ceee`.
The separate worker audit hash is
`db9b6230f5b41ad3608c9411f2d6867bbcaa4971c53da35839befb20dc840cf6`.
This adds manually validated cases, not a general automatic adapter rule;
annual all-market coverage and full Mayhem transition semantics remain
unproven.

## Amendment 36 (2026-10-14) — signature cursor versus supply time scope

Seven further native-SOL Mayhem curve marks retain amendments 34–35's price
kind, source and unknown recording-window limits. Their complete captured
±3-second neighborhoods contain 101 TradeEvents; selected global maxima are
at the provider timestamp's second or one second before it, agreeing at both
dated FX endpoints within 2%. No higher neighboring state is omitted. The
`AiEpwMiG…` control has an earlier matching state but its captured global
maximum differs by about 8.6%; it remains unpromoted. Two AMM candidates are
not evaluated with the curve formula.

A signature cursor is not itself an exact timestamp boundary. For
`FgFgrJLZ…`, `before=first_strict_post_peak_signature` retains one successful
signature two seconds after the provider peak and two failed signatures
inside the queried interval. The saved capture manifest's original mismatch
status remains unchanged. A separate proof explicitly scopes returned
signatures to successful transactions with known timestamps between birth
and the peak, before comparing with the complete filtered TFA set; birth is
added explicitly because `until=birth_signature` is exclusive. Out-of-scope
and failed rows are retained, not silently discarded or counted as effects.
A null/unknown signature time cannot be excluded by this rule. The raw
query/response remains immutable. This corrects a query-scope comparison,
not the on-chain transaction order or economic ledger policy.

The seven complete birth-to-peak success counts are 6/10/47/14/70/14/23.
All have two birth `MintTo` instructions totaling `2e15` raw units, birth
mint-authority revocation, no burn or unknown supply opcode through the
peak, and current metadata-only mint extensions. Actual peak supply is 2B
six-decimal tokens. Exact maximum FDV cent floors are
535,428 / 1,116,635 / 2,474,933 / 602,043 / 683,696 / 651,093 / 2,236,670.
All selected standard finalized transactions reconcile with the captured
TFA messages, balances and inner instructions. Native transfers are scoped
to direct/nested Pump buys; curve and fee legs reconcile, with no additional
creator-vault amount in these selected transactions.

Golden `solana_tracker_mayhem_curve_mark_batch7_cases.json` uses the same
captured-byte regression implementation as the preceding fee cases and
additionally checks the explicit signature-time and failed-transaction
boundary. Root price, supply and native proofs are pinned by SHA-256
`25be858f6306b51dfb8f3ae53982a888c47d07d015afacb849c8e920df12185d`,
`70398ca44cbf59fa6394dba162a703f261b597d1040751a2edb61ebae49040cf`
and `b3831fbadb139490326f24ee657f15054252b04d1d9fa1839097894a86122a9c`.
The final worker supply sidecar has SHA-256
`64546a46fc73a917e5fdfb4d4c138e86d549b793e4083722526b4bb0f54aaecd`;
its earlier strict-cursor-only analysis was revised before any promotion,
while all raw captures and the initial manifest status were preserved.
The source remains `solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1`,
with `ath_at=NULL` and preserved holders/liquidity. No automatic adapter,
runner threshold, full Mayhem transition rule or all-market coverage is added.

## Amendment 37 (2026-10-14) — terminal signature pagination for curve supply

Eight further native-SOL Mayhem curve marks retain amendments 34–36's
source, price kind and provider-window limits. Their complete captured
±3-second neighborhoods contain 140 TradeEvents. All global neighborhood
maxima occur at the provider timestamp's second or one second before it,
within 2% at both dated SOL/USD endpoints. Every event in these neighborhoods
is in the actual captured FX minute; no neighboring minute was substituted.
The `ApRm3VBA…` control's global maximum differs by about 8.7%, so it remains
unknown. The AMM input is not evaluated using the curve formula.

A full `getSignaturesForAddress` page of 1,000 rows is not terminal. For
`EzqFMWBu…`, two bounded continuations return 564 and zero rows, with the
exclusive `before` cursor chained to the preceding page's last signature and
the same explicit birth `until` cursor. All 1,564 signatures have known
timestamps and unique signatures: 228 successful effects and 1,336 failed
transactions. The successful set plus explicit birth equals all 229 filtered
birth-to-peak TFA transactions. Failed rows and raw pages remain retained;
failed execution does not mint or burn. Other candidates' scoped signature
sets also match their complete TFA histories. Unknown timestamps, a
nonterminal page or an unreconciled set would leave supply unknown.

All eight have two birth mint instructions totaling `2e15` raw units,
birth authority revocation, no burn/unknown supply opcode through the peak
and metadata-only current mint extensions. Birth-to-peak success counts are
14 / 202 / 65 / 22 / 62 / 84 / 25 / 229. Actual peak supply is 2B six-decimal
tokens. Stored FDV uses the exact saved provider price times this supply;
cent floors are 565,021 / 2,435,430 / 851,171 / 609,137 / 1,100,700 /
1,208,986 / 1,150,218 / 1,052,206. Independent SDK mark × FX low/high values
are corroboration bands, not uncertainty in supply or a bound on all-market
maximum FDV. No provider `highest_market_cap` with implicit 1B supply is used.

Standard finalized transaction recaptures match complete TFA messages and
selected metadata. Native transfers are scoped to each actual Pump buy;
curve and fee/creator legs and ceiling fee rounding reconcile. These eight
selected transactions have no extra creator-vault amount. Golden
`solana_tracker_mayhem_curve_mark_batch8_cases.json` adds terminal pagination,
failed-signature retention and the negative price control to the shared
captured-byte regression. Root price, supply and native proof SHA-256 values
are `d037e37306bac71c38f626b2b7a4f672c28096ed37aa7f048d4ab69b81b8ba15`,
`26f6ee8acea9d10cec2ccaf18636e0d989c6778f9dca6f555918312f1a98e63c`
and `c32571e54b60cadfaa03dcefd98fcd529d6f79951915224c8e5d6a69b754fd94`.
The separate final worker proof is
`c2544e49bc8dd8932062603fdb33950cf9ac7ef09caa12b445d8b00e101fed6b`;
its prior revisions and all raw captures remain separate. The source remains
`solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1`, with
`ath_at=NULL`, preserved holders/liquidity and unknown recording-window
start. This adds manual cases, not a general adapter or annual all-market
coverage claim.
