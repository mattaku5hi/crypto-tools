# 2026-10-04 BSC venues: verification (PancakeSwap v2/v3, Uniswap v2/v3, four.meme)

Fixture: `docs/p0/measurements/fixtures/evm_bsc_swaps_all_2026-10-04.json` (EXPLORATORY: `evm-capture --chain bsc
--swaps all` of a build that knew only the Uniswap v2/v3 and Aerodrome topics; blocks 125,603,198..125,603,227,
487 swap logs, 300 txs, 162 emitters, 1,007 requests). It has NO Pancake v3 / four.meme topic scan and no Pancake v3
`pool_metadata`; the live recapture (command at the end) replaces it. Test:
`crates/scout-engine/tests/evm_bsc_venues.rs` (data-driven over `evm_bsc_*.json`; mirrors `evm_base_venues.rs`).

## Sources of the pins (invariant #16)
- PancakeSwap (developer.pancakeswap.finance, read by the orchestrator 2026-10-04): v2 Factory
  `0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73`, Router `0x10ED43C718714eb63d5aA57B78B54704E256024E`; v3 Factory
  `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865`, PoolDeployer `0x41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9` (pools are
  CREATE2-deployed by the PoolDeployer, not the factory), SwapRouter `0x1b81D678ffb9C0263b24A97847620C99d213eB14`,
  Smart Router `0x13f4EA83D0bd40E75C8222255bc855a974568Dd4`. Init-code hashes are not published there.
- Uniswap BNB deployments (docs, as pinned before): v3 factory `0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7`, v2 factory
  `0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6`.
- four.meme (official `four-meme-community/fourmeme-docs` @ `5f7f589b`): TokenManager V1
  `0xEC4549caDcE5DA21Df6E6422d448034B5233bFbC`, TokenManager2 V2 `0x5c952063c7fc8610FFDB798152D69F0B9550762b`, Helper3
  `0xF251F83e40a78868FcfA3FA4599Dad6494E46034`; ABIs pinned as `fourmeme_*_5f7f589b.lite.json` (sha256 asserted by
  `fourmeme::tests::pinned_abi_files_are_the_committed_bytes`).

## Topic0s (keccak of the canonical signature; asserted in unit tests, the four.meme ones against the pinned ABI JSON)
| Event | topic0 |
|---|---|
| PancakeSwap v3 `Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)` | `0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83` |
| four.meme V1 `TokenPurchase(address,address,uint256,uint256,uint256)` | `0x00fe0e12b43090c1fc19a34aefa5cc138a4eeafc60ab800f855c730b3fb9480e` |
| four.meme V1 `TokenSale(address,address,uint256,uint256,uint256)` | `0x80d4e495cda89b31af98c8e977ff11f417bafcee26902a17a15be51830c47533` |
| four.meme V2 `TokenPurchase(address,address,uint256,uint256,uint256,uint256,uint256,uint256)` | `0x7db52723a3b2cdd6164364b3b766e65e540d7be48ffa89582956d8eaebe62942` |
| four.meme V2 `TokenSale(...same...)` | `0x0a5575b3648bae2210cee56bf33254cc1ddfbc7bf637c0af2ac18b14fb1bae19` |
| four.meme `LiquidityAdded(address,uint256,address,uint256)` (known non-trade) | `0xc18aa71171b358b706fe3dd345299685ba21a5316c66ffa9e319268b033c44b0` |

The Pancake v3 topic is present in the exploratory fixture's receipts (the kept transactions carry such logs): 64 pools,
157 swap logs, every one with 3 topics and 7 data words (224 bytes), which the decoder demands.

## Rule
Pool venues: admission by the gate over recorded `pool_metadata` (pinned factory + CREATE2 with the pinned hash, the
factory's own record cross-checks when fetched). Verification per (tx, pool): Σ pool `Swap` amounts + the pool's other
token-moving events (Mint/Collect/Flash/CollectProtocol, v2 Mint/Burn) == the pool's ERC-20 net flow. The test prints
two columns. `strict` is the Base rule. `exact` (what promotion requires) adds three named classes the BSC data
exposed, each printed per sample and counted below:
1. **Position.** Pool code emits `Swap`/`Mint`/`Burn`/`Collect`/`Flash` after the call's token movements (v2 `swap`:
   payouts, `_update`, then `Swap`; v3: callback payment verified, then `Swap`). An ERC-20 transfer to/from the pool
   positioned after the pool's LAST own event in the tx cannot belong to it (`skim`/token hook). A flow before the last
   event is never excluded.
2. **v3-family input overpay.** v3/Pancake v3 only require `balanceBefore + amountIn <= balanceAfter`; a payer may
   overpay the input token. Residual `>= 0` is accepted only on a token whose pool amount is positive; the output side
   must equal the event exactly.
3. **Round trip.** A pool transfer of a token to an address that earlier in the tx sent that same token to the pool,
   between the pool's first and last own event, is a return of the swap input (bot `skim`-after-swap loop). It is
   subtracted only if the residual equals it exactly.
The equality was never loosened beyond these; any other difference fails the test.

four.meme: no pools; the manager emits. Per (tx, manager, token): `Σ TokenSale.amount − Σ TokenPurchase.amount` ==
the TokenManager's own ERC-20 net flow of `token` (the curve holds the supply). Native side (derived from the ABI, NOT
yet confirmable offline): V1 `etherAmount`, V2 `cost`, plus `fee`; for a direct buy (`tx.to` = manager, `account` =
`tx.from`, buys only) `tx.value` must be `cost + fee` or `cost` (or 0 for a BEP20-quoted curve). The sale's BNB leg is an
internal transfer from the manager and needs an archive balance diff of the manager per transaction; a busy manager is
touched by many txs per block so the sole-touch rule cannot isolate it: the BNB side of a sale stays unverified.
**These native equations must be confirmed against a live four.meme capture.** The wallet-side rule stays ADR-020: the
event is evidence of a swap on `token` for `account`; the trade is booked only when `account == tx.from` and the
wallet's own deltas are one token + one quote; amounts come from those deltas, never from the event. A router/bot
`account` is `launchpad_account_not_wallet` (counted, not attributed).

## Results (exploratory fixture, admitted (tx, pool) samples)
| Venue | Factory | pools | admitted | strict | exact | extra classes used | status |
|---|---|---|---|---|---|---|---|
| PancakeSwap v2 (v2-style) | `0xcA143Ce3...0c73` | 119 | 258 | 256 | 258 | position 2 samples, round trip 1 sample | FixtureVerified (CREATE2 hash `0x00fb7f63...9bd5` reproduces 119/119) |
| Uniswap v3 | `0xdB1d1001...61F7` | 10 | 19 | 19 | 19 | none | FixtureVerified (canonical hash reproduces 10/10) |
| Uniswap v2 | `0x8909Dc15...8eC6` | 1 | 1 | 1 | 1 | none | FixtureVerified, SMALL n=1 (canonical v2 hash reproduces 1/1) |
| PancakeSwap v3 | `0x0BFbCF9f...1865` | 64 emitters, no metadata | 0 | | | | IdlOnly: not admitted until a live capture records `factory()`/`getPool` |
| four.meme V1/V2 | `0xEC4549ca...`, `0x5c952063...` | | 0 | | | | IdlOnly: no event in the fixture |
| Uniswap v4 PoolManager | `0x28e2ea09...` | | | | | | IdlOnly (no BSC v4 verification test) |

Refused (32 recorded emitters, counted as coverage gaps): 15 without `factory()`; 17 with unpinned factories (five
`0x73dc984d...` v3-topic pools and 12 other single-pool factories: forks, unidentified). They are exactly what the gate is for.

### The two non-strict Pancake v2 samples (both bot "swap then skim" transactions, all Pancake v2 pairs)
- tx `0xcd69b6ef...330cdf`, pair `0x454dc6c5...f85e0e`: event input `896142998413977745197` of token0, pair net 0. The router
  sends token0 in, the pair swaps, then the pair sends the same token0 amount back to the router (log after the `Swap`,
  followed by a `Sync` that lowers the reserve by exactly that amount). Class 1.
- tx `0xe7ef8c03...31a9fc`, pair `0xeed3e35a...92e9f5`: three swaps of token1 in at the same pair; after each the pair returns the
  exact input to the router. Two returns sit between swaps (class 3), the last after the final `Swap` (class 1).
  Event total `+7785187819518121926089`, pair net 0.
These are not decoder errors (amounts, layout and topics match everywhere else) but they are why `exact` differs from
`strict`. Pancake v2 promotion relies on classes 1 and 3 for these 2 of 258 samples; revert that one row to `dep(` to
require `strict`.

### PancakeSwap v3, DERIVED evidence only (not admission, not live)
`derived_pancake_v3_pools_reproduce_create2_and_match_pool_deltas`: for the 64 Pancake v3 swap emitters of the fixture,
`(token0, token1)` taken from the pool's own Transfers and `fee` from the four Pancake tiers, the CREATE2 address from
`(PoolDeployer, ..., hash 0x6ce8eb47...f7e2)` equals the emitter for 64/64 (cryptographic match, so the hash is right
and the PoolDeployer is the deployer). 156 (tx, pool) samples: 155 strict, 156 exact (one pool received 1 wei more
than its event input: class 2, tx `0xb6cb37e8...e8178`, pool `0x81bef404...dd7a50`). The two extra words obey: fee only
on the token the pool received and never above that amount (157/157 swap logs; the invariant is asserted for admitted
samples). `factory()` was never read for these pools, so the row stays IdlOnly.

## Pending (needs the live capture)
- Pancake v3: recorded `factory()` == `0x0BFb...` and `getPool` == emitter (the test asserts the hash reproduces every
  recorded pool and unpins nothing silently); then flip the row if n >= 1 and all exact.
- four.meme: token-side equation, the two native forms (`cost+fee` vs `cost`), BEP20-quoted curves, how many events name
  a router as `account`.
- BSC quote assets: USDT/USDC are 18-decimal Binance-Peg tokens; from the busiest pools of the fixture the candidates are
  `0x55d398326f99059ff775485246999027b3197955` (44 pools) and `0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d` (5 pools),
  NOT verified against an official source and NOT pinned. Until pinned, a trade quoted in them is `multi_asset`
  (counted, never booked): only native BNB (WBNB merged) is a quote.
- Activation blocks stay unpinned (0).

## Caveats
30-block window; ~13 s of BSC. USD for BNB: Coinbase `BNB-USD` exists but its minutes are sparse (research doc §4): the
ADR-018 staleness rule (5 min) applies, a longer gap is `price_unknown`, never zero. No fixture holds manager balances or
traces; native legs on BSC use the archive balance difference where the endpoint serves historical state (Alchemy
`internal` is unsupported on BSC).

## Live recapture command (orchestrator)
```
SCOUT_BSC_RPC_URL=<keyed archive endpoint> \
evm-capture --chain bsc --rpc-url-env SCOUT_BSC_RPC_URL --swaps all \
  --from-block <A> --to-block <A+29> --max-txs 300 --max-pools 200 \
  --rpc-cu-per-sec 250 --out docs/p0/measurements/fixtures/evm_bsc_swaps_all_<date>.json
```
(`--swaps fourmeme` alone captures just the TokenManagers' events; `--swaps pancake-v3` just the Pancake v3 topic.) Use a
window where four.meme is busy; a range cap of 10 blocks (Alchemy free tier) is handled by the span probe, set
`SCOUT_BSC_LOGS_RPC_URL` to an endpoint without the cap for long windows. Then run
`cargo test -p scout-engine --test evm_bsc_venues -- --nocapture` and look for `PROMOTABLE` lines.
