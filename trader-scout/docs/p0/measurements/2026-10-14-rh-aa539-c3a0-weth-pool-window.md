# RH aa539 / c3a0 WETH pool-window price candidates

Read-only capture on 2026-10-14 for two still-unqualified members of the frozen Robinhood ATH cohort. This is a follow-up to the 13-token Doppler/v4 audit, not a claim of lifetime or all-market ATH. It prices only successful post-creation swaps in each token's indexed initial v4 pool, using the matching Coinbase Exchange ETH-USD one-minute candle close and retaining each event minute's low/high as sensitivity.

## Results

Both pools have currency0 = Robinhood WETH (`0x0bd7d308f8e1639fab988df18a8011f41eacad73`) and currency1 = the target token, verified from the exact creation receipt's PoolManager `Initialize`. Both assets have 18 decimals. V4 `Swap.amount0/amount1` are caller-perspective signed deltas; the execution ratio here is `abs(WETH delta) / abs(target-token delta)`. All retained raw amounts are nonzero on both sides. The point maximum is selected only after converting every post-creation event with the matching minute close.

| Token | Post-creation events | Selected event (UTC, block, log) | WETH / token | Coinbase ETH-USD minute close (low–high) | Reference USD/token | 1B-supply FDV; floor cents | All-event low–high sensitivity FDV |
|---|---:|---|---:|---:|---:|---:|---:|
| `0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1` | 3 | 2026-08-25 08:24:05Z, 45,582,061, 107 | `12313550331025 / 2960557767537765289663` | `$2,494` (`$2,491.30–$2,495`) | `$0.000010373043506297537228277634796028538891647705133570667083523304921328681006740379…` | `$10,373.043506…`; **1,037,304 cents** | `$10,361.813667…–$10,377.202705…` |
| `0xc3a0e1a6c10b97c3e124ff8a2c291e5677a6fee1` | 2 | 2026-08-29 20:21:11Z, 49,435,080, 41 | `4538120533941 / 1091323979779174633040` | `$2,455.70` (`$2,455.57–$2,457.62`) | `$0.000010211690388635933737433713388586076929886563579029016290389269630832292705118533…` | `$10,211.690388…`; **1,021,169 cents** | `$10,211.149801…–$10,219.674444…` |

The aa539 pool has two post-creation events in block 45,582,061 and one in block 45,582,406; log 107 is the USD-close maximum. The c3a0 pool has two post-creation events in block 49,435,080; log 41 is the maximum. The raw review manifest contains all five signed raw event tuples and exact rational prices so the maxima can be rederived. These are pool-execution reference prices, not wallet-net acquisition cost.

For aa539, `totalSupply()` and `decimals()` were read at the last included swap block 45,582,406, later than the winning block 45,582,061. The creation receipt contains one target-token mint of `1e27`; the bounded `Transfer(from=zero)` / `Transfer(to=zero)` scan from creation through block 45,582,406 returned one mint and no burns, so the one-billion supply also held at the winner. For c3a0, the `totalSupply()` and `decimals()` reads and the birth-to-selected-block mint/burn scan were at winning block 49,435,080. Root independently read both tokens' decimals at their winning blocks and confirmed 18 decimals. These checks establish the event-block denominator without claiming future supply immutability.

## Scope and evidence

The source scan queried all PoolManager Swap logs for the 13 identified initial pools from the earliest indexed creation block through pinned RH head 87,808,917. It returned five aa539 pool events and four c3a0 events total; the creation-transaction swaps are excluded, leaving three and two respectively. These are complete only for those configured initial pools over their indexed birth-to-head ranges. Other pools, migrations, alternate DEXes, and token market history outside those pools remain unproven; do not set lifetime `ath_at` from these candidates.

A bounded read-only pass made 14 RH JSON-RPC calls: receipts for all three post-creation transactions, headers for the three event blocks, `totalSupply` and `decimals` at aa539 block 45,582,406 and c3a0 block 49,435,080, and two mint/burn log filters per token. It also made two public Coinbase OHLC GETs, one for each distinct UTC minute. Every event receipt succeeded and its exact pool/log-index/data match the retained aggregate Swap log; header hash and timestamp match the log. Response limit was 2 MiB, timeout 20 seconds, and automatic retries were disabled. No Codex, Birdeye, Solana Tracker, or database calls/writes were made.

Private raw evidence and exact-rational candidate data are under
`.scout-data/diagnostics/rh-aa539-c3a0-weth-reference-2026-10-14/`.
The review manifest is `candidate-review.json`, SHA-256
`3eb7ff79c93912eff0c03676589d359891e81d54e7d24f08ec04b519ce8b224e`.
It pins all five events, receipt/header checks, each matching Coinbase candle,
exact supply reads, and the original all-13-pool scan provenance. The aa539
full-window supply manifest SHA-256 is
`6b89d834968823f75913f11524ada44a35ac3150a021b18c2b83d3b7a5793958`; root's
independent verification SHA-256, which includes the winner-block decimals
checks, is
`7f55bcbc64b630602bd0448b6ee88a02a15485f18ac52e1843714f1afa9653e5`. Candidate
status is `candidate_for_root_review_not_promoted`; root review is required
before any guarded database write.
