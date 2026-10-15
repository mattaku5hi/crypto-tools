# BSC ATH cohort: token-supply source audit (2026-10-11)

## Finding

For the 13-token BSC cohort whose Codex rows had no extrema, the source and runtime evidence now covers every token. The contracts reduce to five runtime groups: three FourMeme implementations/variants (including two versions of Token5), plus the two Flap implementations behind fixed minimal-proxy clones. Each group has a Sourcify source/runtime reference; every candidate's on-chain runtime bytes either match a reference directly or are byte-for-byte identical to that reference. The verified source code exposes only one-time initialization minting and no later mint, supply-rebase, or upgrade path. The cohort has one 1,000,000,000-token mint per address and no zero-address burns in the complete queried history; current `totalSupply` is also 1,000,000,000 at the same confirmed BSC anchor. This is enough to treat current supply as historically constant for these 13 contracts and to reconstruct an FDV denominator from that fixed supply.

This closes the **supply denominator** gap for this cohort. It does not turn the Birdeye observations into lifetime ATH: the saved price windows contain only 4–23 daily bars per address, and every maximum is in the first returned candle. Historical pool/price coverage remains a separate gap; no ATH or FDV rows were written by this audit.

## Source/runtime groups

Sourcify's official v2 contract lookup is documented for individual contracts and exposes source, runtime bytecode, match grade, and proxy-resolution fields. The API distinguishes `match` and `exact_match`; for this audit the stored `runtimeBytecode.onchainBytecode` hash was compared directly with each live runtime, so the conclusion relies on byte equality as well as Sourcify's match result. The official docs also say not to crawl individual contract lookups; this audit used the four already-cached source references plus one additional representative lookup for the previously unreferenced FourMeme runtime group.

| Cohort contracts | Runtime evidence | Verified source and supply behavior |
|---|---|---|
| FourMeme Token5, 9,334 bytes: `0x29b9…ffff`, `0x5b02…ffff`, `0x7e31…ffff`, `0x8f4c…ffff`, `0x930e…ffff` | SHA-256 `7bd8f002e50982c4bc4f55e8ea81a2fafba4eb220b6d3b7b7edce368be5f23e0`; matches Sourcify reference `0x29b9…ffff` (`match`) | `Token5.init` is `onlyOwner`, guarded by `_initialized`, and calls `_mint` once (`Token5.sol` lines 228–245 in stored source). No later `_mint` or `_burn` call exists in the token contract. The fee allocation named `rateBurn` transfers tokens to `DEAD`, which changes balances but not ERC-20 total supply. |
| FourMeme Token5 variant, 10,456 bytes: `0x7fca…ffff`, `0x8ce6…ffff`, `0x8dbc…ffff` | SHA-256 `562becfdca0af0b09f921c0ca8227329846f57344c2d4b7aa4916180bbe2a3e8`; Sourcify returned `match` for representative `0x7fca…ffff`; the other two live runtimes have the same SHA-256 | The verified `Token5.sol` is a source variant with added user/reward bookkeeping, but the same one-shot `_mint(owner(), a.totalSupply)` and no later `_mint`/`_burn`. The supply mutation behavior is therefore source-checked for this distinct bytecode group, not inferred from the 9,334-byte build. |
| FourMeme older Token3, 3,822 bytes: `0xfd74…4444`, `0xff48…4444` | SHA-256 `0354180f58d3cf13581fa95c4915946b2cb0831161f201bf889ae8a25cb0e1e2`; matches Sourcify reference `0xff48…4444` (`match`) | `Token3.init` is `onlyOwner`, guarded by `_initialized`, and calls `_mint(owner(), totalSupply)` once (lines 15–23 in stored source). There is no token-level later mint or burn function. |
| Flap V2, 45-byte minimal-proxy clones `0x4353…7777`, `0x8e43…7777` | Clone runtime SHA-256 `4718f8695cc94d314151bec31232a085075b9b291207c620a08846ba90935fdf` embeds implementation `0xae562…07ba`; implementation runtime SHA-256 `b87913aa0db11cad8fc079c2d542c69e4960e0657c25d6eb853374492a41264d`, Sourcify `exact_match` | `maxSupply = 1e9 ether`; `initialize` is protected by OpenZeppelin `initializer` and mints that constant once (`FlapTaxTokenV2.sol` lines 36, 100–107, 138). Implementation constructor disables direct initialization; clone bytecode fixes the implementation address. No later mint/burn/rebase or implementation upgrade entry point appears in the verified token source. |
| Flap V1, 45-byte minimal-proxy clone `0xbf6c…7777` | Clone runtime SHA-256 `8039e3e5d1bfddb86dd0f97ece30d177a2d4416faf9c307a9c6c1efc3cd86757` embeds implementation `0x29e6…32aa8`; implementation runtime SHA-256 `fb1d537eb3ace36badaafa496c375af0323067644e766459f0e19a39716e8ec6`, Sourcify `exact_match` | `maxSupply = 1e9 ether`; `initialize` uses `initializer` and mints that constant once (`FlapTaxToken.sol` lines 115, 147–149, 192–208). The implementation constructor disables initialization; the clone fixes its implementation address. No later supply mutation or upgrade path appears in source. |

The full candidate addresses and exact per-address code hashes are in the private RPC/source manifests below. Ellipses in the table are display abbreviations only.

## State and event corroboration

- The RPC audit used BSC chain ID `0x38` and confirmed anchor block `126987545` (20 blocks behind latest at capture), hash `0x9558e75a5fcfbb3164a83035430d99d2e691e703f12b728271863d460b30f7df`.
- At that anchor, all 13 tokens returned 18 decimals and `totalSupply = 1,000,000,000` tokens.
- For each exact token address, two `eth_getLogs` filters covered its creation block through the anchor: ERC-20 `Transfer` from zero, and `Transfer` to zero. Across the full ranges the audit found one mint log per token (raw amount `10^27`, i.e. 1B at 18 decimals) and zero burn logs.
- The source code confines total-supply changes to the standard ERC-20 mint/burn paths, whose implementation emits those zero-address `Transfer` events. The token implementations do not expose a rebase or upgrade mechanism. Thus the full event history plus runtime-matched source establishes that these current 1B supplies were also their supplies at earlier price highs.

## Evidence files

Private raw evidence remains under `.scout-data/diagnostics/` and is not committed:

- `.scout-data/diagnostics/bsc-ath-unknown-source-supply-2026-10-11/manifest.json` — prior 40-call RPC/price manifest and 13-token cohort details; includes anchor, supply/log totals, launch blocks, Birdeye windows, and code samples.
- `.scout-data/diagnostics/bsc-ath-unknown-source-supply-2026-10-11/raw/` — original RPC responses, including the four initial token runtimes and two Flap clone runtimes.
- `.scout-data/diagnostics/bsc-sourcify-reference-v2-2026-10-11/manifest.json` and raw files — four reference sources: modern FourMeme, older FourMeme, Flap V2 implementation, and Flap V1 implementation.
- `.scout-data/diagnostics/bsc-ath-runtime-source-match-2026-10-11/manifest.json` and raw files — six additional `eth_getCode` reads at the same anchor; four complete missing FourMeme runtimes, the older FourMeme cohort member, and Flap V1 implementation. An initial malformed-block-tag failure is retained as metadata only: its raw body was overwritten by the corrected request. The failed response is excluded from the source proof; successful response hashes are verified.
- `.scout-data/diagnostics/bsc-ath-runtime-source-match-2026-10-11/sourcify-7fca431f.json` — one representative Sourcify source result for the third FourMeme runtime group, HTTP 200, response SHA-256 `85edd3c9761476d48fd80821e93962d920d856d70acd46d692bce4e9e9ee59a0`.

## Primary sources

- [Sourcify API v2 docs](https://docs.sourcify.dev/docs/api/) — individual contract lookup, selected fields, source/runtime match metadata, and fair-use limit against crawling.
- [Sourcify Token5 reference (`0x29b9…`)](https://sourcify.dev/server/v2/contract/56/0x29b9f4e55b3c53b740597e69276a09885185ffff?fields=all)
- [Sourcify older FourMeme reference (`0xff48…`)](https://sourcify.dev/server/v2/contract/56/0xff48007f4560c3a3837b05a8614b4cc00da64444?fields=all)
- [Sourcify FourMeme Token5 variant (`0x7fca…`)](https://sourcify.dev/server/v2/contract/56/0x7fca431f8a3b946f29e40f88cd29b9382530ffff?fields=all)
- [Sourcify Flap V2 implementation (`0xae56…`)](https://sourcify.dev/server/v2/contract/56/0xae562c6a05b798499507c6276c6ed796027807ba?fields=all)
- [Sourcify Flap V1 implementation (`0x29e6…`)](https://sourcify.dev/server/v2/contract/56/0x29e6383f0ce68507b5a72a53c2b118a118332aa8?fields=all)
- [Ethereum JSON-RPC `eth_getLogs` reference](https://ethereum.org/en/developers/apis/json-rpc/#eth_getlogs) — query model for the archived zero-address transfer filters.

## Limits and next step

This proves supply invariance only for the specified 13 addresses and runtime groups. It does not establish completeness of Birdeye's pool universe, daily-candle coverage, migrated plus pre-migration pricing, or a lifetime price maximum. Keep their price-window highs labeled as observed-window values until price coverage is separately established; with this denominator evidence, an independently validated price high can now be multiplied by 1B without guessing token supply.
