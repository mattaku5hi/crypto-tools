# Solana low-burn provider-peak supply audit

This read-only audit checks whether exact supply at five cached Solana Tracker provider price maxima can be recovered from complete mint-address history. It does not validate the provider price itself, upgrade provider-window coverage to lifetime ATH, or write ATH/FDV records.

## Method and scope

The candidate set was selected from the cached 100-token exact provider-price/current-supply join, choosing five initial-supply 1,000,000,000-token mints with small nonzero supply deltas and noncollapsed lower/upper cent bounds. Prior birth receipts prove six decimals, one initial mint, and mint-authority revocation in the creation transaction.

For each mint, a finalized `getSignaturesForAddress` page (`limit=1000`) returned fewer than 1,000 signatures and included the independently validated creation signature as its oldest row. Paginated Helius `getTransactionsForAddress` full JSON (`limit=100`, successful only, no token-account expansion, version 1) continued to a terminal empty page. The successful signature set matched exactly; every full transaction had `meta.err=null` and a slot no later than supply-snapshot slot 455686275. The raw decoder inspected outer and inner SPL Token / Token-2022 instructions, recognized Burn (opcode 8) and BurnChecked (15), retained zero-amount calls, and summed raw amounts by unique transaction/instruction identity.

The exact burn sum equals birth supply minus finalized snapshot supply for all five candidates. A separate raw MintTo/MintToChecked pass uses the correct mint account index (0): each creation receipt’s minted amount equals the validated initial supply, and no successful post-birth MintTo appears in the full history. Burns are ordered against the cached provider Pmax millisecond timestamp using transaction `blockTime` seconds. Thus historical supply at the provider peak is initial supply minus any positive burn whose block time is at or before that timestamp. USD FDV uses the exact decimal provider Pmax and integer raw supply/10^6, then floors cents.

The standard RPC signature pages showed 858, 710, 983, 460, and 463 rows. The corresponding gTFA histories returned 590, 507, 580, 360, and 408 successful full transactions; all five sets matched. The two separate Enhanced Transactions `type=BURN` discovery probes returned empty arrays and were excluded from the proof.

| Mint | Provider Pmax UTC | Pmax USD/token | Burn raw sum = delta | Burn raw at/before Pmax | Supply raw at Pmax | Exact Pmax FDV floor |
|---|---|---:|---:|---:|---:|---:|
| `5b2BsBEVvsjBegVaKik9vByFJ8YGcDbk8R3A96RTpump` | 2026-03-14T21:07:16.990000+00:00 | 0.00012133750413201461 | 87946658 | 0 | 1000000000000000 | $121337.50 |
| `7DGZSvNASBwjVZbFL9UwGtimdgitQu7Pq6RVb23pump` | 2026-01-08T15:51:09.699000+00:00 | 0.000059125282926240427 | 97083577 | 0 | 1000000000000000 | $59125.28 |
| `8bp3yea4ZyT8KkHJQAQCHEMC1AhRHx7bYQfJD2uye6Bd` | 2026-02-05T00:51:05.146000+00:00 | 0.00007616407294683686 | 97083578 | 1 | 999999999999999 | $76164.07 |
| `jKWmQJgLfTRp2VeXHKtGK9DxJReBuuocus7ZU6Kpump` | 2026-02-25T00:28:33.268000+00:00 | 0.000032368803648389175 | 144602940 | 0 | 1000000000000000 | $32368.80 |
| `9dZkVHy3byQvSxw7h7gk5tScp9zfg8rizYDKq9XSpump` | 2026-01-25T11:26:50.559000+00:00 | 0.00008220527118895571 | 264790281 | 1 | 999999999999999 | $82205.27 |

The 8bp and 9d cases each have exactly one 1-raw-unit burn before Pmax; all material burns occur later. That unit is included in their exact peak supply, so no “all burns after peak” assumption is used. The other three cases have no positive burns at/before Pmax.

## Evidence and limitations

Primary evidence manifest: `.scout-data/diagnostics/solana-burn-sample-discovery-2026-10-11/burn-reconstruction.json` (SHA-256 `419c8f7f1d8064cc67b230d800e011f691257de9cd8853b66f41405fc9e3c143`). It references the raw requests and responses, response-body SHA-256 hashes, signature-page results, birth receipts, provider observations, and exact source joins. No secrets were present in persisted response bodies; all request metadata redacts credentials.

The full-data endpoint is Helius’ address-history RPC and the raw response includes transaction/message/meta fields; it was paginated at a 100-row limit with explicit finality and transaction-version settings. The account history is scoped to each mint address, and complete pagination was cross-checked against the standard mint-address signature listing. No post-snapshot transaction was included.

This verifies the supply multiplier at the cached provider Pmax observation for these five tokens. It does not independently corroborate Solana Tracker market prices, prove token/pool market integrity, establish that each provider recording is a canonical lifetime ATH, or authorize production FDV writes. Any existing independent-source price gate remains separate.

References: [Helius getTransactionsForAddress](https://www.helius.dev/docs/rpc/gettransactionsforaddress) documents full transaction history, pagination, and the full response shape; [Helius Enhanced Transactions migration guide](https://www.helius.dev/docs/parsed-events/guides/migrate-from-enhanced-transactions) documents the legacy type filter and states the replacement API has no server-side transaction-type filter.
