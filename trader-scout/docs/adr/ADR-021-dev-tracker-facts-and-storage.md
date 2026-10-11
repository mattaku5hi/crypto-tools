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

## Amendment 10 (2026-10-11) — all-time filtered Base runner enumeration

The separate opt-in `backfill --chains base --phases ath
--ath-runners-from-index` enumerates Codex Base mainnet tokens with
`athFdv >= top_runners.runner_ath_usd`,
`athFdv <= top_runners.max_plausible_ath_usd`, and
`holders >= top_runners.min_holders` from the selected
`--ath-index-config` (default `config/dev-tracker.toml`). It sets
`includeScams:true`, orders by `createdAt ASC`, and does not send a
`createdAt` range. This is an all-time provider-index filter pass; the current
holder count and current index membership can change.

The pass stores only rows matching already tracked Base launch tokens. It
uses a separate filter-identity cursor, advances by raw provider page length,
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
public five-row response fixture
`crates/scout-devtracker/tests/fixtures/codex_base_runner_filter_2026-10-11.json`
records the observed provider shape without credentials.

This is a targeted candidate-enumeration aid for runner analysis. It does not
change the runner definition, ATH policy, or statistical interpretation; the
filter itself selects high-ATH tokens and therefore cannot be treated as an
unbiased sample of all launches.
