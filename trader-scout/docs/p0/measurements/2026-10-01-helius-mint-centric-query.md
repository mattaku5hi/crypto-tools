# 2026-10-01 — Helius `getTransactionsForAddress` with a mint as the address argument

## What was measured

`docs/p0/measurements/fixtures/pump_mint{1,2}_full.json` were captured by calling Helius
`getTransactionsForAddress` with a **pump.fun mint address** (not a wallet) as the `address`
parameter, `limit=5` per mint, `transactionDetails: "full"`. This is the same endpoint
`HeliusProvider` already uses for `ScanRequest::WalletActivity`; the question this measurement
answers is whether the endpoint also returns usable results when an SPL mint is passed instead of
a wallet pubkey.

## Result: the endpoint accepts a mint address and returns mint-touching transactions

Both calls returned 5 transactions each (10 total), all of which contain SPL token balance
entries (`preTokenBalances`/`postTokenBalances`) referencing the queried mint. This is the load-
bearing fact: **Helius will serve transaction history for a mint pubkey through the same call
shape used for a wallet pubkey.** That is what makes `ScanRequest::TokenMarketActivity` answerable
by `HeliusProvider` at all.

## Secondary observation: where the mint appears in the resolved account-key space

Offline re-analysis of the already-committed 10 transactions checked whether the queried mint
address also appears in `message.accountKeys` or the ALT-resolved
`loadedAddresses.writable`/`.readonly` space (the space `decode_full_transaction_record` builds
for instruction/account resolution):

| Transaction | Mint location |
|---|---|
| mint1 tx[0] | ALT readonly |
| mint1 tx[1] | ALT readonly |
| mint1 tx[2] | ALT readonly |
| mint1 tx[3] | ALT readonly |
| mint1 tx[4] | ALT readonly |
| mint2 tx[0] | ALT readonly |
| mint2 tx[1] | static |
| mint2 tx[2] | static |
| mint2 tx[3] | static |
| mint2 tx[4] | ALT readonly |

**7 of 10 via ALT-readonly resolution, 3 of 10 via static keys — not 10 of 10 via any single
path.** Both paths are already handled by `decode_full_transaction_record`'s existing ALT
resolution (landed in `c013a7a`), so no decoder change is needed for this specific finding. This
does **not** establish that mint-address-in-resolved-key-space is a reliable signal for anything
by itself — the actual mechanism that matters is `preTokenBalances`/`postTokenBalances` entries
tagging `mint` directly as a string field, independent of whether the mint pubkey also happens to
sit in the account-key space (it does in all 10/10 cases here only because SPL instructions that
touch a mint's accounts routinely also reference the mint itself as a readonly/static account —
this is incidental, not load-bearing for decoding).

## What this does NOT establish

- **Completeness.** A 5-per-mint, 2-mint sample proves the endpoint responds, not that it returns
  every transaction touching a mint, or that `limit`+`sortOrder` behave the same way for a mint
  address as documented for a wallet address. `pagination_token` is not exercised here.
- **Query semantics.** It is not confirmed from Helius's own documentation that
  `getTransactionsForAddress` is *specified* to accept a non-wallet (program-derived or mint)
  address — only that it did not reject these two live calls. Treat this as an empirically
  observed behavior, not a documented contract, until checked against Helius's docs.
- **Ranking/buyer semantics.** Nothing here identifies "buyers" of a token. It establishes a raw
  transaction feed keyed by mint; attribution of who acquired the asset is a separate concern
  (`scout-normalize`'s `aggregate_solana_token_balance_changes`, P0.13), and that pipeline
  currently emits `ActionKind::Unknown`, never `Swap` (no confirmed decoder exists — P0.12).

## Implication for `HeliusProvider::capabilities()`

`token_market_activity` can be upgraded from `Unsupported` to a real (if narrow) capability: it
returns **candidate transactions referencing a mint**, not a verified buyer list. Naming matters
here — calling this "buyer discovery" in the capability string would misrepresent what the
underlying call does, given attribution still yields zero confident hits until P0.12 lands.
