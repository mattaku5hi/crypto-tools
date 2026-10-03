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
