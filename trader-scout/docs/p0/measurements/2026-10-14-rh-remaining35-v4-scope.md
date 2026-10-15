# Robinhood remaining ATH rows: configured v4 pool-scope audit — 2026-10-14

## Result

The frozen B8.4 progress snapshot lists 35 Robinhood Chain tokens still missing a Codex ATH record. All 35 map to an `Initialize` pool at the configured Robinhood Uniswap v4 PoolManager (`0x8366a39cc670b4001a1121b8f6a443a643e40951`). Across their initialization-to-captured-head Swap-log scopes, the audit found **no post-creation swaps**. Three pools each emitted two Swap logs inside the token-creation transaction; those launch/bootstrap events are not evidence of later trading. The other 32 pools had no Swap logs in their covered ranges.

This result is limited to this configured PoolManager and the frozen 35-token cohort. It does not prove that the tokens had no trades on other venues, that no other pool manager exists, or that ATH equals zero. With no post-launch trade in this audited venue, the pool initialization state cannot supply an executed market price; ATH/FDV remains unknown from this evidence.

## Scope repair and exact evidence

The 35 authoritative token keys are retained in the immutable [`root-immutable-current35-input.json`](../../../.scout-data/diagnostics/rh-remaining35-poolmanager-discovery-2026-10-14/root-immutable-current35-input.json), SHA-256 `aac0cfb7bd2c0ba4827fc16218054a48e12d206d63b30b6c5d4c0dfa54b4e9a5`. They match the Robinhood `remaining_records` in the live checkpoint at the recorded input time, scoped to the immutable 679-key diagnostic cohort (not the full annual token universe). The live checkpoint itself is mutable; its earlier observed hash is historical provenance, not a promise that the current file keeps that hash. An exploratory 35-address scan had instead selected 32 of these addresses and three addresses outside this snapshot. The final counts below use only the authoritative 35; the mismatched exploratory target set is preserved and disclosed in the private manifest.

The resulting pool mapping and per-token scan provenance are in the mode-0600 [`remaining35-reconciliation.json`](../../../.scout-data/diagnostics/rh-remaining35-poolmanager-discovery-2026-10-14/remaining35-reconciliation.json), with source file hashes and the exact token-to-pool map. The raw RPC requests/responses and their hashes are in the same mode-0700 directory. The capture pinned chain ID 4663 and latest head block `0x53cd18d` (87,871,885), hash `0x12b687d6b1bc4496e97a1ca66239c5f218aa4cbece07ab787078969f83b5a22a`; this is a latest-head observation, not finalized history.

Coverage was assembled from these bounded reads:

- The earlier 30-pool audit has 30 successful, zero-log Swap scans from each pool's Initialize block to its pinned 2026-10-11 anchor. The two saved delta filters cover all 30 of those exact pool IDs from the next block through the 2026-10-14 captured head; both responses are empty.
- The separate 13-pool follow-up covers four other tokens in this cohort from their launch through its pinned head: three pools had two Swap logs each, both in the token's creation transaction; the fourth had zero. The three two-log pools were additionally scanned from birth through the 2026-10-14 head, again finding only the two creation-transaction logs. The zero-log pool was checked from the follow-up head through the 2026-10-14 head and remained empty.
- One additional initial pool for token `0x6b91…` was scanned from its Initialize block through the 2026-10-14 head and had no Swap logs.
- The corrected two-position Initialize search for the three current snapshot addresses missed by the exploratory target list returned one pool per token. A single grouped Swap scan from the earliest of their exact birth blocks through the pinned head returned six logs, two per pool; each pair has the token's launch transaction hash. Thus all three have zero post-creation Swap logs.

The audit made 25 read-only RPC calls against the owner's configured Robinhood endpoint within a 30-call cap, with response bodies bounded to 2 MiB, no retries, and no database writes or price-provider calls. No credentials or credential-bearing URLs are stored in the captures.

## Interpretation and limits

An Uniswap v4 `Initialize` event proves a pool was initialized; it is not a traded price. The two Swap logs in each of the three launch transactions are retained as bootstrap activity and excluded from the post-creation market-trade count. This classification is consistent with the project's existing distinction between launch/bootstrap events and later market activity; it does not erase those raw events.

The search covers only the listed manager's `Initialize` and `Swap` events for these exact token addresses. It does not inspect every DEX, every factory, later migration destinations, or unrelated market structures. Therefore “no post-creation swaps” here means only “none in the configured Robinhood v4 pool(s) through the captured head.” It is a price-availability classification for this source scope, not a universal market-absence claim.

The earlier 30-pool baseline documents the protocol address and source mapping: [Robinhood Doppler/v4 empty-window audit](2026-10-11-rh-doppler-empty-window-protocol-audit.md). Its primary references identify Doppler's chain-specific deployment and Uniswap v4's dynamic-fee sentinel. Uniswap v4 emits `Swap` and `Initialize` from the PoolManager; the exact event layouts and signed `Swap` amount convention used for decoding are pinned in [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs).

## Follow-up

To obtain a nonzero ATH estimate for these rows, first establish an independently identified post-launch price source for the relevant token and quote asset, including any migrated pools. Do not convert these empty configured-pool windows into zero prices or zero FDV.
