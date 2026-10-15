# Pump curve trade wire tails: dated SDK source check (2026-10-14)

## Finding

The pre-observation official Pump SDK `1.31.0` IDL contradicts the earlier
24-byte baseline recorded in the vC95 wire audit. Its `buy` and
`buy_exact_sol_in` instructions each declare two `u64` arguments and a final
`track_volume: OptionBool`; `OptionBool` is a one-field struct containing a
`bool`. Their complete instruction data is therefore 25 bytes including the
8-byte discriminator. `sell` declares two `u64` arguments and is 24 bytes.
The IDL declares 16 buy accounts and 14 sell accounts.

For captured buys with 25 bytes, the final byte is the schema's `track_volume`
only when it is a valid Borsh boolean (`00` or `01`). The source schema makes
that byte meaningful; its value must still be validated from the raw bytes.
Captured 24-byte buys omit the mandatory dated-IDL field and are not complete
SDK-1.31.0 decodes. They may represent an older/alternate deployed wire
variant, but this source does not identify which. The 17th buy account and
extra sell accounts likewise have no role in this IDL and remain unresolved.
Keep those shapes conditional and preserve their tails.

This does not establish deployed-program decoder behavior or a Mayhem wrapper
ABI. It does not affect PumpSwap: the separate PumpSwap 6SFB/7rWY instruction
variants remain unresolved by this Pump curve source.

## Primary source and reproducible evidence

- Official npm release metadata: [Pump SDK registry record](https://registry.npmjs.org/@pump-fun%2fpump-sdk), version `1.31.0`, published `2026-03-06T06:49:58.068Z`, before the March 14 and March 20 observations. Registry SRI: `sha512-pGubvJBYbxhNzlBOQhOI4WyCu5z1J/8bT1sgrKK6iRNGBpOw7ZV1Fiq6eyDmNBDtnx+AliBl476tkwU7JY/E3g==`; git head `569b25b0433cec2997b9cb4fd3d28146fc417f76`.
- Official package tarball: [Pump SDK 1.31.0](https://registry.npmjs.org/@pump-fun/pump-sdk/-/pump-sdk-1.31.0.tgz). Existing credential-free capture SHA-256: `b12fdf292139f16bb2ff209a0bd57ac5e2b2168d687dc3d51a131cf45575ad18`; embedded `package/src/idl/pump.json` SHA-256: `0ba934dd633279d22c33b8ed247e0c06accf81f5c91537d68c2d20b592aa0f57`. The package was inspected as an archive; no package code was installed or executed.
- Saved raw transaction-wire proof: `.scout-data/diagnostics/solana-vC95-full-fdv-2026-10-14/analysis/wire-consistency-audit-v1.json`, SHA-256 `cc9407ad85c7eea3fe72bdc21e45547ca141c2b0c19c302933804fe862911bcd`. It records the audited cohort shapes and itself labels the dated instruction data as 24 bytes, which is inconsistent with the actual extracted IDL buy arguments described above. The proof's raw observed byte/account counts remain usable; its `dated_idl_data_bytes` baseline and any inference depending on it need correction in a separately versioned follow-up.
- Earlier dated-source inventory: `.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch13/dated-curve-idl-budget-v1.json`, SHA-256 `9b19b5f18809b231cde5749cce916123fb2ef06fa1cd5e1fa362cb3e6343de0b`. It records the prior source-capture budget and integrity checks.

## Scope and limits

No public requests were made for this check; it reused saved official npm
metadata/tarball captures and saved raw-wire proof. The prior capture manifest
records five source GETs in its phase and eight cumulative source requests
within that earlier authorization. No RPC/provider calls, credentials,
database writes, or production code changes were made here.

Resolved: the official dated SDK's declared buy/sell schema widths and named
account counts. Unresolved: why observed buys sometimes lack `track_volume`,
what the extra account metas mean, and whether any captured wrapper/deployment
accepted those shapes. This remains conditional curve evidence, not proof of
complete deployed semantics.
