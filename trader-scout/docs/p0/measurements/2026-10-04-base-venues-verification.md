# 2026-10-04 Base venues: live verification (Uniswap v2/v3, Aerodrome v2, Slipstream)

Fixture: `docs/p0/measurements/fixtures/evm_base_swaps_all_2026-10-04.json` (`evm-capture --chain base --swaps all`,
blocks 52,146,910..52,146,939, 1,622 swap logs, 300 txs kept, 52 pools, 578 requests, 0 x 429).
Test: `crates/scout-engine/tests/evm_base_venues.rs` (data-driven over `evm_base_*.json`).

## Rule
Admission: the emitter's `factory()` is pinned for the venue AND (CREATE2 address with the pinned hash, Uniswap v3 only)
or the factory's own `getPool`/`getPair` record names the emitter (Aerodrome v2: `getPool(t0,t1,stable)`,
Slipstream: `getPool(t0,t1,tickSpacing)`). Verification, per (tx, pool): sum of the pool's `Swap` amounts plus the
pool's other token-moving events equals the pool's ERC-20 net flow, exactly. Other events (topics keccak-computed in
the test): v3/Slipstream Mint(+), Collect(-), Flash(+paid), CollectProtocol(-), CollectFees(-), Burn (no flow);
v2/Aerodrome Mint(+), Burn(-), Aerodrome `Fees`(-). The equality was not loosened.

## The one mismatch (first run)
tx `0xa64d50c8...0730`, Uniswap v3 pool `0xe8f16fbf...d6de`: event swap amounts [207509, -10964350826886990] vs pool net
[-8818, -1837981670460432]. The tx also holds, for the same pool, `Burn` (log 0x145), `Collect` (0x148) and `Mint`
(0x14f) around the one `Swap` (0x14c): a position rebalance. Not a fee-on-transfer token, no second swap, no donation.
With Collect subtracted and Mint added the equation holds exactly. Only that one v3 sample had other pool events.

## Results (admitted n / exact n)
| Venue | Factory | admitted | exact | samples with other pool events | status |
|---|---|---|---|---|---|
| Uniswap v3 | `0x33128a8fC17869897dcE68Ed026d694621f6FDfD` | 182 | 182 | 1 | FixtureVerified (canonical init-code hash reproduces 17/17 pools) |
| Uniswap v2 | `0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6` | 5 | 5 | 0 | FixtureVerified (registry record decides; no hash) |
| Aerodrome v2 | `0x420DD381b31aEf6683db6B902084cB0FFECe40Da` | 8 | 8 | 8 (all: `Fees`; 1 also `Mint`) | FixtureVerified |
| Slipstream | `0x5e7BB104d84c7CB9B682AaC2F3d509f5F406809A` | 21 | 21 | 0 | FixtureVerified |
| Slipstream | `0xf8f2eB4940CFE7d13603DDDD87f123820Fc061Ef` | 14 | 14 | 0 | FixtureVerified |
| Slipstream | `0xaDe65c38CD4849aDBA595a4323a8C7DdfE89716a` | 4 | 4 | 0 | FixtureVerified (small n) |
| Uniswap v4 PoolManager | `0x498581ff718922c3f8e6a244956af099b2652b2b` | 92 | 92 | 0 | FixtureVerified, see below (`evm_uniswap_v4_base.rs`) |

Refused (coverage gaps, 4 pools): factory `0x02a84c1b...` (PancakeSwap v2 on Base), `0x4bd16d59...`,
`0xc35dadb6...` (unpinned forks, possibly SushiSwap v3), and one emitter without `factory()`.

## Aerodrome v2
- Live pools emit topic0 `0xb3e2773606abfd36b5bd91394b3a54d1398336c65005baf7bf7a05efeffaf75b`
  (`Swap(address,address,uint256,uint256,uint256,uint256)`), 8/8 swaps; none emit Uniswap v2's `0xd78ad95f...`. The
  research doc's "same topic0" note is wrong. Data layout confirmed: amount0In, amount1In, amount0Out, amount1Out.
- Fee rule: the earlier heuristic ("pool transfers of the input token to a recipient other than to/sender") was NOT
  needed and was removed. The pool's own `Fees(sender, amount0, amount1)` event names the fee moved to PoolFees, and
  subtracting it makes all 8 samples exact. Sample `0xe2704acb...c8b3` additionally has a `Mint` (zap: swap + add liquidity).

## Caveats
Samples come from a 30-block window; Slipstream `0xade65c...` has n=4. Activation blocks stay unpinned (0).

## Uniswap v4 (PoolManager `0x498581ff718922c3f8e6a244956af099b2652b2b`)
Test: `crates/scout-engine/tests/evm_uniswap_v4_base.rs` (data-driven over `evm_base_*.json`; regenerate the per-sample table with `--nocapture`).
Method as on Robinhood (`2026-10-04-uniswap-v4-robinhood-verification.md`): the `Swap` amounts are the swapper's `BalanceDelta`, so per ERC-20 token the PoolManager's net flow in the tx (Transfers to minus from it) equals `-sum(amount_i)` of the swaps on that currency.
Result: 159 v4 `Swap` logs in the receipts; **n = 92 samples, 92 exact**. Currencies: 92 derived from PoolManager Transfers of the same tx (single-pool txs: each side matched exactly by one token, native side only on side 0, ERC-20 pairs sorted), 0 from `Initialize` (3 `Initialize` logs exist in the receipts, but no sampled tx has all its pools announced by one). 69 ERC-20/ERC-20 pools, 23 with a native-ETH side (invisible in logs: corroborated by `tx.value == -amount0` in 10 of 13 recorded payer txs, informational only). 2 samples are multi-swap txs on one pool; both directions occur (48 samples with a negative `amount0`).
Excluded and printed (not samples): 7 txs with other PoolManager events (ModifyLiquidity `0xf208f491...`, ERC-6909 transfer `0x1b3d7edb...`), 18 multi-pool txs (intermediate currencies cancel in the net; no `Initialize` in the window to name them).
Promotion: n >= 1 and all samples exact => `FixtureVerified` (flag in `gate.rs`, the test checks flag and evidence agree). `active_from_block` stays 0.
