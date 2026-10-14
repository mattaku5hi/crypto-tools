# QBH/HUvd observed-window reference recovery — 2026-10-14

QBH's apparent unsupported80-byte event is a fully decoded dated
CloseUserVolumeAccumulatorEvent. Exact instruction/event-CPI/log bytes and
account roles identify it as nontrade volume metadata beside the separate
BuyEvent. Reclassifying it retains57actual trades and114price states.

HUvd's runtime logs end with Log truncated. A canonical getTransaction has
the same log, so it cannot manufacture lost text. Its unique EventCPI carries
the complete439-byte dated BuyEvent, matches the logged payload, and follows
a clean PumpSwap invocation prefix. Accounts, actual vault deltas and fee
identities bind the recovered two states;90prior states bring the total to92.
The raw buy_exact_quote_in is24bytes while dated IDL requires25with track_volume.
That byte remains unparsed, with no inferred default. Price marks use actual
vault observations, with a conditional model and unknown historical VQ.

The first1000successful HUvd history rows cover only118seconds after birth;
requested filter bounds are not returned coverage. A six-call terminal
signature census bounds the missing history, and29additional TFA calls finish
it. Total39TFA pages have3721successes, exactly matching the5210before-anchor
census records plus one pinned anchor (1490failed signatures retained).
One1B birth mint, authority revocation and two pre-window burns yield
993824561055834raw@6 at every window boundary. Current861553990334431raw
is later/lower. All target metadata/mint/burn/authority wire structures and
parents are independently checked; SetAuthority(None) is3bytes in instruction
data, not the4-byte account COption representation.

QBH does not need a full burn history for this bounded reference: raw birth
mint and revocation plus current mint snapshot bound historical supply between
998812646101067 and1e15raw@6. Both snapshots have verified account identities,
6decimals and metadata-only extensions18/19. This is an interval, not exact
historical supply. Root independently recomputes all114state bounds.

| Case | Marks | Supply policy | Source | Outward reference USD |
| --- | --- | --- | --- | --- |
|HUvd|92|Exact replay at each state|onchain-pool-window-supply-cex-reference-v1|43515.45–43607.35|
|QBH|114|Current lower/birth upper bounds|onchain-pool-window-bounded-supply-cex-reference-v1|38150.36–38261.87|

Both global price endpoints pass the2% comparison to original provider peaks.
Tiny FX/cent/supply intervals do not bound model error or omitted markets.
These remain conditional selected-pool windows, not lifetime/all-market/
annual ATH. Dates remain null, holder/liquidity metadata and runner gates
remain unchanged. No generic production adapter is introduced.

Private evidence SHA-256:

- Root all206marks: `14f855e2c82e11c75d1455d93ac9b2d554d6e2109f3c0ea22439e596e185e785`.
- HUvd supply audit: `beecf24559a5933981f67e633b37c77f08a5fb5f1192cc74df7a78d1f3add517`.
- HUvd complete replay v3: `987c2f979a65d832ddf89efa25b70d53c5cfa0ae4c3cceb24a2a33bc57a68a2b`.
- QBH raw-bound audit v3: `b5a2550daacae56d18a74fd97b9aaf68cac54f8c7af735a448de98d212d5bd6f`.
- QBH all114state root replay: `0247fd7edd68b3649b7ab06677ec65d38d934b371f827dd5f88dc4e17a5a01ac`.
- QBH nontrade resolution: `09c0af342ef5721c4cf9bd0000f3612e603c186d3cb286de4f6ea289b6dc5235`.

HUvd used46Helius methods (one canonical receipt,39TFA pages, six census
pages), QBH used no new RPC. Both reuse saved price/minute/mint/birth evidence;
no new Codex, Solana Tracker or CEX requests. The initial raw-gate certificate
was changed after its359c…SHA was announced; that original body is unavailable.
The later435b…body is frozen at a new v2 path, with a correction sidecar.
No numerical claim changed; do not treat that v2 copy as the missing original.
Subsequent supply audits and source proofs are separately versioned/pinned.
Next task B8.4: remaining historical price, supply and model gaps.

Golden: `solana_tracker_qbh_huvd_metadata_cpi_cases_v1.json`, SHA-256
`90af07a933604ccddc12af4b1c93fc63f562d0665fd4359d6f0f79d00b630760`.
It verifies the nontrade/CPI wire facts, all114QBH bounded-supply marks,
all92HUvd exact-supply marks, scope/source-kind equality, raw authority and
metadata conditions, and the distinction between high floor4360734 and
outward ceiling4360735. Full fmt/Clippy/1238tests passed in58.99seconds in
isolatedscout_test, production DB tests disabled, incremental compilation off.
Earlier round22Clippy type-complexity failure is retained; a local type alias
resolves it without a lint exemption.

The guarded two-row transaction stored4351545cents for HUvd and3815036 for
QBH, with date null and holder/liquidity preserved. Fixed679-key cohort now
has519numeric/160unknown,85new references since434 (SOL71/Base2/RH12).
Remaining cohort gaps are Base100, Robinhood35, Solana25, BSC0; these counts
are not annual all-market coverage. The sealed80package predates five recent
writes. The daemon kept PID92417 with no restart; no new full snapshot is
created for these individual reference updates.
