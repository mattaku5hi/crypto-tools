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
