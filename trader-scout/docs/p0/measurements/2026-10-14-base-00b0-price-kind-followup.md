# Base 00b0: execution and post-swap price maxima remain distinct

For `0x00b0a07ec58d87dc09606e3166e1312f679e3b07`, the seven discovered
direct-USDC V4 pools have 108 saved Swap logs from birth block 45,447,242
through anchor 52,456,236. Initialize events bind the exact target as
currency0 (18 decimals) and USDC as currency1 (6 decimals). All 108 have
positive emitted liquidity and a positive square-root price. Four have a
zero-sided amount delta: their execution ratio is undefined, while their
post-swap marginal state remains included. The unrelated unverified-quote
pool is not converted to USD.

Full signed ABI amount decoding produces 104 two-sided execution ratios.
The maximum, at block 45,450,118/log 59, is approximately
`0.0000225075058564` USDC/token from only `0.463849` USDC consideration.
Its emitted Swap fee is 900,000 pips, or 90%. The maximum is 9.50 times its
own post-swap marginal mark. A large fee is consistent with this divergence;
the comparison does not prove that fees alone explain all route/hook effects.
This amount ratio must not be silently used as a marginal price ATH.

The maximum post-swap marginal mark over **all 108** events is approximately
`0.0000153008582873` USDC/token, at block 45,452,962/log 96, in pool
`0x97bfae4087592d108a3a5ba107973fa53852e5a185052eaf70eeda4869228961`.
Its event has 10,000 fee pips (1%) and only `0.009671` USDC input. The exact
mark formula is `sqrtPriceX96² / 2¹⁹² × 10¹²`, reflecting the verified asset
orientation and decimal difference. Same-day Kraken USDC/USD low, close and
high are 0.9997, 0.9998 and 0.9999; parity is not assumed.

Five new bounded RPC methods confirm Base chain identity and recapture both
maxima's successful receipts and block headers. Both complete receipt logs,
block hashes, numbers, timestamps, signed amounts, price, liquidity and fees
match the retained historical logs. Existing raw captures were not replaced.

**No ATH promotion was made.** The cached Birdeye daily maximum for May 2 is
`0.000010010536877171716` USD/token, materially below the on-chain post-state
mark (about 53%). Birdeye returned only two daily rows, and its aggregation
and outlier treatment are not established by this comparison. The saved
GeckoTerminal history covers two other discovered pools, not this maximum's
pool. These limits, tiny trade sizes, and incomplete venue scope prevent
calling the reconstructed mark a corroborated annual market-wide ATH.
No notional filter was invented to remove the maximum or improve the result;
all observations and both price kinds are retained separately.

Private reproducible evidence:

- `base-00b0-execution-ratio-offline-2026-10-14/proof.json`, SHA-256
  `0f1f9a18c03fc88ee241faa7c7010738be83a0b6cfc41cc1eb620f61955c8a82`;
- `base-00b0-postspot-offline-2026-10-14/proof.json`, SHA-256
  `cb413a605475c289294d3e0d423dd041599d953244fa4000d6eaa93f6d4ee225`;
- `base-00b0-canonical-maxima-2026-10-14/root-canonical-proof.json`, SHA-256
  `4976c019e54b1010627b0685d8d5e4f39e703f509be30186df4c5f6ea44f5d02`.

The first-party [Pool library](https://github.com/Uniswap/v4-core/blob/main/src/libraries/Pool.sol)
defines swap fees in pips; [PoolManager](https://github.com/Uniswap/v4-core/blob/main/src/PoolManager.sol)
emits the applied fee, and [LPFeeLibrary](https://github.com/Uniswap/v4-core/blob/main/src/libraries/LPFeeLibrary.sol)
uses 1,000,000 as 100%. These source semantics support distinguishing the
fee-bearing execution ratio from the marginal state; they do not establish
provider ATH algorithms or wallet-net acquisition costs.
