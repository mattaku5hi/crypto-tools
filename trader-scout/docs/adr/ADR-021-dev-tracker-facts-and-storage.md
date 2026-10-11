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
