# Pump Mayhem bonding-curve market-cap semantics

**Question.** For native-SOL Mayhem bonding curves, does the official TypeScript SDK’s market-cap calculation use `virtual SOL reserves × token supply ÷ virtual token reserves`, and can Mayhem alter those reserves independently of a trade’s `sol_amount`?

## Findings

The official `@pump-fun/pump-sdk` 4.1.0 package defines `bondingCurveMarketCap` as integer arithmetic:

```text
market_cap_quote_units = virtualQuoteReserves * mintSupply / virtualTokenReserves
```

The helper accepts no Mayhem flag and applies the same reserve-ratio calculation to the inputs it receives. In `getFee`, the SDK chooses `mintSupply` for Mayhem curves and its `ONE_BILLION_SUPPLY` constant for non-Mayhem curves, then passes that supply and both virtual reserves to the helper. The source comment describes the result as market cap in quote-mint base units (lamports for SOL curves). This is direct evidence that the SDK’s fee-tier market-cap calculation uses the formula for Mayhem too; it does not establish that every Pump UI/API “market cap” or ATH field uses that same definition.

The official Pump program IDL includes `set_mayhem_virtual_params` and an `UpdateMayhemVirtualParamsEvent` carrying the mint, old and new virtual token/SOL reserves, and real token/SOL reserves. The official program README documents `is_mayhem_mode` as a bonding-curve field. Together these establish that Mayhem virtual reserves can be explicitly changed and that the new values are observable in the event schema. The IDL alone does not document the instruction’s triggering schedule or full state-transition rules; do not reconstruct Mayhem reserve changes from user `sol_amount` or assume the ordinary buy/sell reserve delta rule covers these updates.

This matches the narrow observation supplied for three Mayhem mints: fee was zero, the virtual SOL reserve changed by more than the user’s `sol_amount`, and the observed tracker peak was within 0.15% of the maximum virtual-reserve ratio. That observation is consistent with the SDK formula; it is not a general proof of Pump’s UI ATH semantics or of every transition in Mayhem mode.

## Source and reproducibility

- [Official Pump program README](https://github.com/pump-fun/pump-public-docs/blob/main/docs/PUMP_PROGRAM_README.md) documents bonding-curve reserve fields and normal buy/sell movements, and identifies `is_mayhem_mode`.
- [Official Pump public docs README](https://github.com/pump-fun/pump-public-docs/blob/main/README.md) links the TypeScript SDK and describes the virtual quote reserve field.
- [Official Pump program IDL](https://github.com/pump-fun/pump-public-docs/blob/main/idl/pump.json) contains `set_mayhem_virtual_params` and `UpdateMayhemVirtualParamsEvent`.
- [Official npm package, version 4.1.0](https://www.npmjs.com/package/@pump-fun/pump-sdk/v/4.1.0); [public npm registry metadata](https://registry.npmjs.org/@pump-fun%2fpump-sdk/4.1.0) identifies the package repository as `pump-fun/pump-sdk` and points to the published tarball.
- The downloaded package was inspected as data only (no install or code execution). Tarball SHA-256: `5c0af15764468ac4c6fa6aaada9653d97eff1ea3532b62bc60da451354faacfb`; npm integrity: `sha512-bjxHqUFgC6mcltooQZBbfIrmNBXeiCQNXGhEFPGaOSOCG47vuos153uAbIRL4dESa+6IaVodYUvIMWvgm9DV1w==`. The archive is retained at `.scout-data/diagnostics/pump-sdk-source-2026-10-14/pump-sdk-4.1.0.tgz`.
- Relevant package source hashes: `package/src/bondingCurve.ts` SHA-256 `c9d796111d676861d6fe6359cc96dd045afda4bd4bf1702f48d326cb3d5910da` (helper at lines 738–750); `package/src/fees.ts` SHA-256 `039ad335b2932b128c6d48693965ae6ed96a4acb7ae0139330dde10d0a04d8ef` (Mayhem supply selection at lines 72–81 and helper call at lines 159–164); `package/src/idl/pump.json` SHA-256 `7616681ea557975f9f623ac211080f22a93060781eaf4765b0a488d980997207` (instruction and event schema).

No paid-key API, RPC, package installation, or live-chain request was used. The published SDK source and IDL answer the formula and identify a Mayhem-specific reserve update, but the exact on-chain Mayhem transition behavior remains outside this source-only check.
