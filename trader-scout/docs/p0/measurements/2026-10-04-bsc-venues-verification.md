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
4. **Fee-on-transfer output** (added after the live recapture). The pool paid out `amountOut` of a token whose
   ERC-20 `Transfer` shows a smaller, NON-ZERO amount (the token emits the recipient's net amount; its tax has no
   Transfer of its own). Accepted only on the output side, only when a Transfer exists (a missing one never qualifies)
   and only when the Transfer is smaller; the shortfall is printed. The pool code does transfer `amountOut`
   (v2 `_safeTransfer(to, amountOut)`), so the event is right and the log is short.
The equality was never loosened beyond these; any other difference fails the test.

four.meme: no pools; the manager emits. Per (tx, manager, token): `Σ TokenSale.amount − Σ TokenPurchase.amount` ==
the TokenManager's own ERC-20 net flow of `token` (the curve holds the supply). Native side (derived from the ABI, NOT
yet confirmable offline): V1 `etherAmount`, V2 `cost`, plus `fee`; for a direct buy (`tx.to` = manager, `account` =
`tx.from`, buys only) `tx.value` must be `cost + fee` or `cost` (or 0 for a BEP20-quoted curve). The sale's BNB leg is an
internal transfer from the manager and needs an archive balance diff of the manager per transaction; a busy manager is
touched by many txs per block so the sole-touch rule cannot isolate it: the BNB side of a sale stays unverified.
**The native equations needed a live capture; it showed the launch-buy surcharge above.** The wallet-side rule stays ADR-020: the
event is evidence of a swap on `token` for `account`; the trade is booked only when `account == tx.from` and the
wallet's own deltas are one token + one quote; amounts come from those deltas, never from the event. A router/bot
`account` is `launchpad_account_not_wallet` (counted, not attributed).

## Live recapture 2026-10-04b (HEAD-200..HEAD-171, 624 swap logs, 300 txs, 171 emitters)
Added `evm_bsc_swaps_all_2026-10-04b.json`; the table below is over BOTH fixtures (exploratory + live). The two failures
of the first live run and their causes:

1. **Pool equation, 1 inexact admitted sample: a fee-on-transfer token (class 4).** Tx `0xd97e9c4f...ed566`, Pancake v2
   pair `0x3a4b1ff7...d4781d0e` (token0 `0x0fd1ebe9...32f2`): `Swap.amount0Out` = 26,392,017,734,329,494,472, but the
   token's `Transfer(pair -> recipient)` is 24,597,360,528,395,088,848; the shortfall 1,794,657,205,934,405,624 (6.8%) is the
   token's tax, which has no Transfer. Not a decoder error. The newly admitted Pancake v3 pools were NOT the cause:
   40 pools, 130 samples, 130 strict (protocol-fee words, PoolDeployer/factory roles and the hash all fine; the hash
   reproduces 40/40 recorded pools with live `factory()`/`getPool`).
2. **four.meme, 2 of 4 inexact: token launch in the same transaction.** The tokens are created by the manager's own
   transaction: `TokenCreate` (topic0 `0x396d5e90...`, same signature in both ABIs, asserted against the pinned JSON) is
   emitted by the manager and the token's whole `totalSupply` (1e27 raw) is transferred to the manager BEFORE the first
   buy, so the manager's net is `totalSupply - amount`, not `-amount`. With the named launch class
   (expected net += `TokenCreate.totalSupply` for the same token and manager in the tx) all 4 samples are exact on the
   token side (2 launches, 2 ordinary: one buy through a router `0x43dd...ae1bb` whose event account is still the signer,
   one `TokenSale`). Where tokens really are: the manager holds the supply from creation; no separate vault was needed.
3. **four.meme native side: a surcharge the event does not explain (resolved by the live check at the end; was IdlOnly, now FixtureVerified on the ADR-015/017 standard).** Both direct launch buys have `tx.value` above
   `cost + fee`: tx `0x72c96fae...caaa2`: value 2,361,000,000,000 = 1.03 x cost (cost 2,292,233,009,705, fee 1%); tx
   `0x06ab2e52...bf10`: value 1,780,000,000,000 = 1.11 x cost. The extra 2% / 10% of cost is in neither the event fields
   nor `launchFee` (0). A time-decaying launch/anti-sniper fee is a guess; no ABI field says it. These samples are
   classed `Other`: they do not fail the token-side test but block promotion (`NOT PROMOTABLE` is printed). Needs the
   four.meme docs or more launch samples (value / cost across blocks after launch).
4. **Why no `fourmeme:` line in the capture summary.** The summary was printed through `Secrets::redact`, which caps text
   at 2,000 characters; with ~170 pool lines the tail (the `fourmeme:` line and the extraction counts) was cut. Fixed:
   the summary uses `redact_full` (secrets removed, control characters blanked, no cap); errors/notices keep the cap
   (unit test `the_summary_is_not_truncated_but_notices_are`). The four.meme events themselves were gated and counted
   correctly (the 4 samples above have `account == tx.from`).

## Results (both fixtures, admitted (tx, pool) samples)
| Venue | Factory | pools | admitted | strict | exact | extra classes used | status |
|---|---|---|---|---|---|---|---|
| PancakeSwap v2 (v2-style) | `0xcA143Ce3...0c73` | 222 | 420 | 417 | 420 | position 2, round trip 1, fee-on-transfer output 1 | FixtureVerified (hash `0x00fb7f63...9bd5` reproduces 222/222) |
| Uniswap v3 | `0xdB1d1001...61F7` | 19 | 34 | 34 | 34 | none | FixtureVerified (canonical hash 19/19) |
| Uniswap v2 | `0x8909Dc15...8eC6` | 2 | 2 | 2 | 2 | none | FixtureVerified, small n=2 (canonical v2 hash 2/2) |
| PancakeSwap v3 | `0x0BFbCF9f...1865` | 40 (live metadata) | 130 | 130 | 130 | none | FixtureVerified (hash `0x6ce8eb47...f7e2` + PoolDeployer reproduces 40/40) |
| four.meme V2 | `0x5c952063...762b` | 4 (tx, manager, token) | | token side 4/4 (launch class for 2) | | paid >= cost+fee 2/2 direct buys (surcharge 198 / 990 bps, live-confirmed) | FixtureVerified (n = 4, small; ADR-020 amendment 7) |
| four.meme V1 | `0xEC4549ca...` | 0 | | | | | IdlOnly: no event |
| Uniswap v4 PoolManager | `0x28e2ea09...` | | | | | | IdlOnly (no BSC v4 verification test) |

Refused (32 recorded emitters, counted as coverage gaps): 15 without `factory()`; 17 with unpinned factories (five
`0x73dc984d...` v3-topic pools and 12 other single-pool factories: forks, unidentified). They are exactly what the gate is for.

### The two non-strict Pancake v2 samples (both bot "swap then skim" transactions, all Pancake v2 pairs; exploratory fixture)
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
- four.meme: the native side of launch buys (value = 1.03 / 1.11 x cost, unexplained), BEP20-quoted curves, V1 samples,
  sale-side BNB leg, more samples (n=4).
- BSC quote assets (pinned, ADR-020 amendment 6): Binance-Peg BSC-USD (USDT) `0x55d398326f99059fF775485246999027B3197955`
  and Binance-Peg USD Coin (USDC) `0x8AC76a51cc950d9822D68b83fE1ad97B32Cd580d`. Sources: bscscan token pages
  "Binance-Peg BSC-USD (BSC-USD)" and "Binance-Peg USD Coin (USDC)" (label "binance-pegged") and the BNB Community Support
  article "Binance-Peg token list". These are bridged/pegged assets (`origin = binance_peg`), 18 decimals, checked live by
  the run preflight (`decimals()`, mismatch = refusal). Own units `usdt_peg`/`usdc_peg` (18 dp, never the 6-dp
  `usdt`/`usdc`); USD: USDT via Coinbase `USDT-USD`, USDC par (`usdc_par_assumed`), `binance_peg_legs` + `+binance_peg`
  label suffix in the coverage. The earlier "`multi_asset`" behaviour for trades quoted in them is gone. The two
  fixture-derived candidates (44 and 5 pools) are the same addresses. Test: `evm_bsc_quote_assets.rs` (synthetic,
  hand-computed). Residual risk: a depeg of the bridged tokens is invisible (par/USDT-USD is assumed for both).
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

## four.meme native surcharge — refund hypothesis tested live (orchestrator, 2026-10-04)

Archive balance difference (Alchemy BSC) for the two direct launch buys, W touched by no other tx
in the block: tx `0x72c96fae…` value 2,361,000,000,000 wei, `cost+fee` 2,315,155,339,802, W paid
(balance diff − gas) 2,360,999,999,996 (refund 4 wei); tx `0x06ab2e52…` value 1,780,000,000,000,
`cost+fee` 1,619,639,639,636, paid 1,779,999,999,996 (refund 4 wei). **No refund**: the wallet really
pays ~2–10 % above `cost+fee` in these launch-window buys (a surcharge not represented in the event).
Consequence: the wallet-side consideration (ADR-013/020: wallet's own deltas, exact via balance-diff)
already books the true cost incl. the surcharge; for venue *evidence* the quote side follows the
ADR-015/017 standard ("never better for the wallet than the event" — here the wallet pays more), and
the token side is exact 4/4 → four.meme V2 promotable as evidence (n = 4, small; re-check on more
samples). four.meme V1 has no samples (stays IdlOnly).

Final criterion (ADR-020 amendment 7): `evm_bsc_venues.rs` fails if a four.meme sample has an inexact token side OR a direct
native buy with `tx.value < cost + fee (+ launchFee)`; it prints the surcharge per sample (offline run: 2 direct buys,
`paid/min` 1,780,000,000,000 / 1,619,639,639,636 = +990 bps and 2,361,000,000,000 / 2,315,155,339,802 = +198 bps; 2
not-applicable). four.meme V2 is FixtureVerified with n = 4 (small); V1 stays IdlOnly (no samples).

## Quoter pins (valuation, ADR-019 amendment 1 / ADR-020 amendment 7, orchestrator, 2026-10-04)
Live getters: Pancake v3 QuoterV2 Base `0x4c650FB4…4e3B` and BSC `0xB048Bbc1…5997` (factory `0x0bfbcf9f…1865`, deployer
`0x41ff9aa7…71c9`); Uniswap v3 QuoterV2 BSC `0x78D78E42…B077` (factory `0xdb1d1001…61f7`); V4Quoter Base `0x0d5e0f97…048d`
(poolManager `0x498581ff…2b2b`), BSC `0x9f75dd27…37b0` (`0x28e2ea09…e9df`); Slipstream Base gen1 `0x254cF9E1…15b0`, gen2
`0x3d4C2225…1c6C`, gen3 `0x514c8B5f…9259` (each `factory()` equals the generation's factory). Slipstream live quotes
(`quoteExactInputSingle((address,address,uint256,int24,uint160))`, `0x9e7defe6`): gen1 on pool `0x47ca96ea…` (tickSpacing 1)
1,140,890,463,828 out for 1e12 in; gen3 on `0x01271a20…` (100) 14,137,063,755; gen2 on `0xb1857b20…` (100) returned 0 for
1e12 in (a tiny amount: a valid quote of 0, not a revert).
