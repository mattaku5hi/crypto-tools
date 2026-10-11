# Additional Base Clanker supply verification — 2026-10-11

All 53 additional Clanker tokens with nonempty saved Birdeye windows passed
the source/runtime and supply checks. They are separate from the earlier
24-token verified cohort. No ATH values were written by this audit: USD price
corroboration remains pending.

The reference is the [Sourcify verified ClankerToken deployment](https://sourcify.dev/server/v2/contract/8453/0xe44861a39b8f73040455fa618aa921907d450b07?fields=all),
compiled with solc 0.8.28, optimizer 20,000 runs and viaIR. Its runtime is
12,791 bytes. Sourcify reports a runtime match, no proxy, no bytecode link
references and no CBOR auxiliary data. Constructor immutables differ between
deployments, so comparison masks only the compiler's ten 32-byte immutable
locations. Every other byte must match the verified reference. All 53 do.

The declared immutable fields are seven EIP712 domain/name/version fields
and ClankerToken's original administrator, whose value appears three times.
They do not define total supply. This is comparison against a verified
implementation using compiler-supplied offsets, rather than accepting a
similar bytecode length or selector list. The offsets and each deployment's
actual immutable values are preserved in the proof manifest.

The source permits constructor mint, ordinary ERC20 burns and Superchain
bridge mint/burn; all supply changes use ERC20 paths emitting zero-address
Transfer events. No rebase or upgrade mechanism is present. Consequently,
current supply alone would be insufficient. At Base block **52,456,236**,
hash `0x021113f4f7d0a2fe23e0036948b341ef7bbe5c75a604eb56f96c8a873d5e39b8`,
all tokens have decimals18 and raw supply `100000000000000000000000000000`.
Two address-array log filters retain exact parameters and cover block
45,105,283 through that anchor. Each token has exactly one 100B creation
mint at its recorded creation block and no burn. The complete queried ledger
reconciles to the state, establishing invariant supply through the anchor
for these specific tokens, despite callable burn/bridge paths.

Four bounded HTTP requests carried 163 JSON-RPC reads: chain/block preflight,
53 runtime reads, 106 supply/decimals calls and two mint/burn log queries.
All returned HTTP200; no retries. Response cap2MiB, timeout25s, spacing2.5s;
the largest response was 1,357,958 bytes. Every raw response SHA-256 was
revalidated. Endpoint credentials remain private.

Evidence:

- `.scout-data/diagnostics/base-additional-clanker-input-2026-10-11.json` —
  exact 53-address input and saved price scope.
- `.scout-data/diagnostics/base-clanker-sourcify-reference-2026-10-11/manifest.json`
  — full verified source/compiler/runtime reference and response hash.
- `.scout-data/diagnostics/base-additional-clanker-proof-2026-10-11/manifest.json`
  — exact request parameters, anchors, runtime comparisons and per-token ledger.

This closes a denominator gap. It does not establish completeness of the
provider's markets, validate its USD high, or prove canonical lifetime ATH.
Next task remains **B8.4**: independently corroborate the saved price windows
before any guarded missing-only FDV writes.
