# Chronological Solana FDV reference windows — 2026-10-14

Three previously unknown records now have independently replayed reference
intervals. Maximum FDV is selected across chronological price/supply states,
including burns, rather than multiplying maximum price by supply at that
price timestamp. This resolves an important counterexample in FLkQktRL.

| Token | Lower FDV USD | Upper FDV USD, rounded up | Combined uncertainty | States |
|---|---:|---:|---:|---:|
| `EWbabspX…` | 6,130.81 | 6,138.03 | 0.11759% | 469 |
| `FLkQktRL…` | 6,326.73 | 6,328.92 | 0.03445% | 455 |
| `2b4p8ien…` | 60,254.65 | 60,311.81 | 0.09477% | 791 |

The combined gate compares the floored lower scalar with the unrounded upper
maximum, including cent rounding. All1715 states participate. FLk's FDV
maximum is onMarch14, before its April28 provider price peak; the price-peak
supply scalar alone is about$3,734.54 and misses the larger earlier FDV.
Both global price endpoints agree with the saved provider maximum within
0.08% for all three cases. These values do not meet the$500k runner threshold.

## Evidence and scope

Captured successful mint-address histories are terminal at238 /234 /404
transactions, and match separately paginated scoped successful signatures
plus birth. Failed signatures and the earlier nonterminal2b4 capture remain
preserved. All three actual birth transactions issue2B tokens through two
mint operations and revoke mint authority. A strict raw-instruction audit and
later metadata-only Token2022 mint layouts reject unsupported supply features.
Later current supply is not substituted for historical supply.

The official Pump SDK1.31.0 release predates the March births and declares
the complete captured Create/Trade schema. Its IDL documents exact-SOL-in
net-minus-one rounding; its bondingCurve.ts supplies the other direction-
specific integer quote formulas. The tarball and extracted member hashes
are verified. This is dated official client evidence, not independently
verified historical deployment of the wrapper binary.
[Official release registry](https://registry.npmjs.org/@pump-fun%2fpump-sdk),
[official SDK1.31.0 package](https://www.npmjs.com/package/@pump-fun/pump-sdk/v/1.31.0).

All684 curve trades bind actual instruction/event-CPI identities and scope.
Root independently parses one scoped SPL transfer per trade: mint, token
program, decimals, authority, source, destination and amount all reconcile.
The359 buy native inputs match scoped System transfers. The325 sell gross
outputs match whole-transaction curve lamport deltas, with one curve trade
per transaction; these are not claimed as isolated per-CPI System transfers.
Separate fee/rent semantics remain explicit.

The previous POST quote reserve reproduces every trade under the dated SDK
formula, but integer rounding generally allows more than one reserve. A
separate inversion audit derives the full compatible PRE interval, below
0.001% width in every case. Both endpoints are included in FDV maxima.
Mayhem's current POST quote minus actual native movement disagrees555 times;
that subtraction is not used as PRE evidence. Wrapper payloads remain
unparsed, and no reset timing or complete deployed-wrapper ABI is asserted.

The replay preserves1031 observed Create/curve POST and selected-pool PRE/POST
states and adds684 bounded curve PRE references. Supply changes follow actual
instruction order. Coinbase SOL/USD minute low/high cover all84 event minutes;
80 missing minutes required14 bounded candle requests, and four were cached.
This is discrete state and minute-reference coverage from birth through the
saved peak neighborhood, not continuous USD-price or annual/all-market ATH.
Mint-address history is not a universal index of markets or reserve-only
mutations. ADR-021 amendment43 records these limits and the conditional model.

## Reproducibility

Final full replay SHA-256:
`414d4555fda934b0a8eab435430ff7efcabc746ca9ae02d134214cf4d2ef2d2d`.
Root independent certificate-v3:
`2897e4dfac148afb0c91e8a8f2bd66d75d6cde25fbdee166c89b8fbfb11d90ed`.
Root scoped token-transfer proof:
`fc16c015c3b9d2bdf9c1218eef8d30fa849f2636251ea2386dc08c8b6463a323`.
Six finalized standard recaptures of births and FDV-winning transactions
match entire messages and selected metadata; manifest SHA-256:
`c58ac6e1b807ff1aa0d3564642d443569c9af8e6085e7c0992063fb6b8a200d0`.

The initial root certificate-v2 read a transient v25 body while its producer
was still correcting max selection. Its source hash is stale and it is not
accepted. Both the invalid certificate and a supersession note are retained;
the unavailable transient body is not reconstructed. Certificate-v3 pins the
final replay before and after independent verification. Maximum values did
not change, but the provenance correction is necessary.

Network phases are separate: the main capture/history used25Helius methods
under its27-method allocation; the current wrapper-IDL probe used2; finalized
birth/winner confirmation used6. Initial peak-window FX used3Coinbase GETs,
and full-history FX used14 more. No new Tracker/Codex calls, automatic retries
or key rotation were used. Offline replay and independent certificates make
no network or database calls.

The stored source is
`onchain-mint-history-curve-selected-pool-fdv-window-cpm-bounded-cex-v1`,
with lower floors, null ATH dates and preserved holder/liquidity metadata.
The captured-case golden is
`solana_tracker_chronological_fdv_window_cases.json`, SHA-256
`bb9e9ebc04d7730bd23add291c6e7354c034c0a496af7197ce834b5e3ca2e171`.
Its regression verifies all states, exact raw FX multiplication, actual trade/
token-transfer roles and bytes, invocation boundaries, integer-interval edge
controls, mixed direction/rounding and the earlier-FDV counterexample.
Full workspace fmt, all-target/all-feature Clippy and1234tests passed against
isolated scout_test in48.34seconds. A subsequent atomic guarded transaction
wrote all three lower floors and verified null dates and unchanged holder/
liquidity metadata. Fixedcohort511numeric/168unknown,77closures since434.
The74-closure archive predates these three writes. B8.4 remains active.

Changed files: birdeye.rs captured-case regression, new golden, ADR-021,
AGENTS gotchas, this and central measurement, tickets and archive checkpoint.
Next task B8.4: remaining scoped price/supply/schema gaps; annual coverage
is not inferred from this candidate cohort.
