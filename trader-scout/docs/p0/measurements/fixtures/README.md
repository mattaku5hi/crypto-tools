# Pinned schema sources

- `dflow_aggregator_v4_carbon_1e6e16b_events.rs.txt` — DFlow Aggregator v4 (`DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH`) event schema.
  Third-party (Codama-generated, NOT an official DFlow IDL). Repo `sevenlabs-hq/carbon`, commit
  `1e6e16b46e0efb0fc9cd6c8684ed9721e7da716a`, paths
  `decoders/dflow-aggregator-v4-decoder/src/{types,events}/{swap_event,fee_event}.rs`
  (concatenated, each preceded by a `//// FILE <path>` line; the tree has no IDL json),
  sha256 `7322e9994797cdd6ad5a1d7fde9cafb4fcb261400d360a0137f921ec3cfa1252`.
  Checked by `scout-dex-solana` (`dflow_event` tests) and `scout-engine/tests/dflow_swap_legs.rs`.
- `jupiter_v6_idl_12bc5f67.json` — Jupiter v6 IDL, `jup-ag/jupiter-cpi` `idl.json` @
  `12bc5f67b94a2c3edc74d6e721a19442124a0bad`, sha256
  `764ea6d71b77458fd33aeb308d6e6bb19e660fc5320c5359f3b9cac96eba5c50` (ADR-015).

## On-chain Anchor IDLs (fetched 2026-10-03 via Helius `getAccountInfo`, zlib-decompressed)

The IDL account address is Anchor's `create_with_seed(find_program_address([], program), "anchor:idl", program)`;
the account is owned by the program itself (published by its upgrade authority).

| File | Program | IDL account | sha256 |
|---|---|---|---|
| `jupiter_v6_onchain_idl_2026-10-03.json` | `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4` | `C88XWfp26heEmDkmfSzeXP7Fd7GQJ2j9dDTUsyiZbUTa` | `12a0856158b2b6927d683a2ba21566f82e39989aca476e23c02d845fc38cdca8` |
| `okx_dex_router_onchain_idl_2026-10-03.json` | `proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u` (OKX: DEX Router) | `8wXL8gQduvMr6pmzhJnbUqsnnegJnmnPiZVPzehLjoeT` | `c1f85197a5d96dd43fc2a1b981126cfe04c3eb6d2d68b3731129a259e45d54c9` |

Checked by: `scout-dex-solana` (`jupiter_event` and `okx_event` IDL-equality tests: event set, discriminators
`sha256("event:<Name>")[:8]`, field order and types; for OKX the whole `Dex` enum) and, over the committed live
fixtures, `scout-engine/tests/okx_router_legs.rs` (order events vs owner-keyed deltas, criterion: token side exact, quote side never better for the owner; hops vs venue CPIs; `SwapWithFeesCpiEvent2` FixtureVerified on 24 samples, other five order events IdlOnly) and
`jupiter_swap_legs.rs`.

## USD pricing (ADR-018)

- `coinbase_sol_usd_candles_1m_2026-10-02T1200Z_recorded.json` — RAW response body of
  `GET https://api.exchange.coinbase.com/products/SOL-USD/candles?granularity=60&start=2026-10-02T12:00:00Z&end=2026-10-02T12:05:00Z`
  (HTTP 200, User-Agent header required), recorded live by the orchestrator on 2026-10-03. Newest first,
  `[time, low, high, open, close, volume]`, JSON numbers. The recording was shortened to the three candles
  the orchestrator pasted (12:05, 12:04, 12:00 UTC; the minutes 12:01-12:03 are NOT in this file, which is also
  how the stale-price tests get a gap). Parsed exactly (no float) by `scout_pricing::parse_candles`; used by
  `scout-pricing` tests, the USD ledger goldens in `scout-engine/tests/solana_wallet_ledger.rs` and the
  wallet-stats / wallet-rank CLI wiremock tests. Not a committed live capture of any wallet.

## Venue IDLs for P4.9 direct-venue decoders (2026-10-03)

| File | Program | Source | sha256 |
|---|---|---|---|
| `orca_whirlpool_onchain_idl_2026-10-03.json` | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | on-chain Anchor IDL account `2KFqE4RWoPVbvodo8vbggCFeHPS8TDvgpwp79ALMrcyn` | `7afddfe8766bd24d30ff9ee5b12c7ee89bff5bc3b7b0396620e021a97a7ef63f` |
| `meteora_dlmm_onchain_idl_2026-10-03.json` | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | on-chain Anchor IDL account `7UZRobkzaKVm1RbCH5WdFaYCGzCRjnu3prziHAsYiSyr` | `57ee0b91fb1505f9af4be8d073ecdea65adc395bae49c96a707a263b257eca84` |
| `raydium_clmm_idl_e7e0c96f.json` | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | official `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359` (no on-chain IDL account) | `040a8c4866317fa028be8a81db54325ce6d9b92aeb10582d89992855bbbce5c1` |
| `raydium_cpmm_idl_e7e0c96f.json` | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | official `raydium-io/raydium-idl` @ `e7e0c96f…` | `1202f6dc8e1c3216598f2ad5c620b9aa8c64ac584563fafed68125c27fb6df81` |

Checked by (P4.9, ADR-013 section 2b): `scout-dex-solana` (`whirlpool_event`, `dlmm_event`, `raydium_clmm_event`,
`raydium_cpmm_event`: IDL-equality tests for the full event set with discriminators, the swap-event layouts and the swap
instruction discriminators/account positions; DLMM `Swap`/`Swap2Evt` via `emit_cpi!` with event authority `D1ZN9Wj1...`,
the other three via `emit!` `Program data:` log lines attributed by the invoke/success stack, `venue_log`) and, over the
committed live fixtures, `scout-engine/tests/venue_swap_legs.rs`: 275 events (Whirlpool `Traded` 20, DLMM `Swap` 111 and
`Swap2Evt` 111, CLMM `SwapEvent` 19, CPMM `SwapEvent` 14); every event passes at least one side against the pool-owned
token-account deltas AND against the vault accounts named by the swap instruction, whose raw mints equal the leg's mints
(input side exact 264, output side exact 273). The deployed Raydium CLMM `SwapEvent` is 213 bytes, the pinned IDL layout
(197 bytes) plus 16 unpinned trailing bytes (19/19 samples); 213 is FixtureVerified, 197 IdlOnly.

## EVM live fixtures

- `evm_robinhood_token_aiden_v4_2026-10-03.json` — `evm-capture --chain robinhood --token 0x15e853bc1c69529bd0a16bab1a742645a3607e1f` (symbol `Aiden`), window 2026-10-03T17:54:46Z..18:04:46Z (blocks 79,276,983..79,282,918), public RPC `rpc.mainnet.chain.robinhood.com`, preflight ok: 48 txs, 330 logs, 45 Uniswap v4 `Swap` (gated), 44 extracted trades of which 19 with `NativeLegNotObserved` (native-ETH sells; no trace/archive source). Robinhood quote assets seen in the busiest v4 swaps: `USDG` `0x5fc5360d0400a0fd4f2af552add042d716f1d168`, `WETH` `0x0bd7d308f8e1639fab988df18a8011f41eacad73`.
  Step 2 (2026-10-04): the 2 `malformed_log` are ERC-721 PositionManager NFT mint/burn (now `nft_transfer_logs`); Uniswap v4 PoolManager verified against all 45 swaps, see `../2026-10-04-uniswap-v4-robinhood-verification.md`; replayed offline by `scout_providers::evm_replay` in the engine and CLI tests.

- `evm_robinhood_token_pwplt_v2v3v4_2026-10-04.json` — token `PWPLT` 0x04cc671f…, 2-hour window via the public RPC: 11 txs, swaps: 1 Uniswap v2 (pair `0x8803c117…`), 1 Uniswap v3 (pool `0x52e65b17…`), 5 Uniswap v4 (gated).
