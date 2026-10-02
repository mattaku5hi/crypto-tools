# ADR-012: PumpSwap AMM trades in the SOL-quoted wallet ledger

Status: Accepted
Date: 2026-10-02
Extends: ADR-010 (SOL-quoted ledger) to a second venue; ADR-009 (variant verification) to the
PumpSwap program.

## Context

Live runs (`docs/p0/measurements/2026-10-02-wallet-rank-live.md`) show that wallets on public
leaderboards trade mostly on PumpSwap AMM (`pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`) after a
token migrates off the bonding curve. With only bonding-curve trades decoded, these exits appear as
unexplained outbound movements and the wallets' episodes are `ClosedUnknown`.

`scout-dex-solana::pump_amm` (commit `ea60431`) decodes `buy`, `buy_exact_quote_in`, `sell` against
the official IDL pinned at pump-fun/pump-public-docs `e0687ae9` (sha256
`2091433899b07d003d98118ae6cd3c628960fd393b40710b6e15bce6d0e7f2d1`) and pairs each trade 1:1 with its
`BuyEvent`/`SellEvent`. Evidence over 128 live trades
(`fixtures/pumpswap_variants_live_2026-10-02.json`, `pumpswap_wallet_page_2026-10-02.json`):
127 paired, 0 mismatched; of 126 (tx, user) groups 116 reconcile exactly on both legs, 4 have quote
residuals fully explained by rent (token account 1,513,840 / `user_volume_accumulator` 1,346,200),
a 970,453 platform-fee transfer + 10,000 tip, or a 34,999 third-party transfer; 3 are signers
receiving the base token while another account funded the quote; 3 are router-forwards (decoded
`user` not a signer, net zero on both legs). All three variants are `FixtureVerified`.

## Decision

1. **Consideration source = the paired event** (as ADR-010 §2), in raw quote units:
   - Buy (`buy`, `buy_exact_quote_in`): cost = `quote_amount_in_with_lp_fee + protocol_fee +
     coin_creator_fee` (72/72 exact). Not `user_quote_amount_in`: for `buy_exact_quote_in` it is only
     the net pool credit. `buyback_fee`, `cashback`, `holder_rewards` are carve-outs inside the fees
     and are not added.
   - Sell: proceeds = `user_quote_amount_out` = `quote_amount_out - lp_fee - protocol_fee -
     coin_creator_fee` (55/55 exact).
   - Base leg: `base_amount_out` (buy) / `base_amount_in` (sell) from the event.
   - Unpaired or mismatched event → `ConsiderationUnverified` (Unknown basis/proceeds), as ADR-010.
2. **Which asset is the traded token.** Mints come from the instruction accounts (`base_mint`,
   `quote_mint`); events carry none. Exactly one side must be wSOL
   (`So11111111111111111111111111111111111111112`):
   - quote = wSOL (normal pool): token = base mint; `buy` = acquire token for the SOL cost above,
     `sell` = dispose token for the SOL proceeds above.
   - base = wSOL (reversed pool): token = quote mint; the instruction `side` is inverted
     economically. `sell` (user gives base wSOL) = **acquire** token: SOL cost = `base_amount_in`,
     tokens received = `user_quote_amount_out`. `buy` (user receives base wSOL) = **dispose** token:
     SOL proceeds = `base_amount_out`, tokens given = the buy-cost formula above. Pool fees are then
     charged in token units and are already inside the token amounts.
   - neither side wSOL → trade recorded, PnL `Unknown { UnsupportedQuoteAsset }` (ADR-010 §3).
3. **Attribution (invariant #2).** A trade is attributed to the wallet only when the decoded
   `user` is the wallet **and** the wallet's own legs reconcile: base leg exact (owner-keyed token
   delta of the traded mint), quote leg exact or with a residual (rent, tips, platform fees) that is
   left to ADR-010 §5's `unexplained_native_flow` diagnostic. Quote leg for wSOL = owner-keyed wSOL
   token delta + native lamport delta (+ `fee_lamports` if the wallet is fee payer), because a wSOL
   account opened and closed inside the transaction leaves no token delta.
   - **Router-forward** (user not a signer, both legs net zero) → not attributed; the transfer that
     actually moves tokens to the economic owner is handled by inventory continuity (P0.18).
   - **Quote funded elsewhere** (base leg exact, wallet's quote movement 0) → the wallet receives
     the tokens but did not pay: acquisition lot with `BasisStatus::Unknown
     { QuoteFundedByAnotherAccount }`, never basis 0 and never the event cost.
4. **Fees.** Network fee: ADR-010 §4 unchanged (fee payer only, split across the wallet's trades in
   the transaction over both venues by consideration). Pool fees are already in the consideration.
5. **One ledger, two venues.** Bonding-curve and PumpSwap trades of the same mint feed the same
   FIFO per `(wallet, mint)` in canonical order; episodes span venues (buy on the curve, sell on the
   AMM is one episode). The report counts trades per venue and per variant verification status.
6. **Versions.** Ledger version bumps; the report scope names both programs and both IDL pins.
   Venues still not decoded (Raydium, Meteora, Orca, Jupiter-only routes not ending in these two
   programs) keep producing continuity breaks — named as such, never as zero PnL.

## Consequences

- Leaderboard-style wallets whose activity is curve-buy → AMM-sell or AMM-only become measurable.
- Reversed pools and non-SOL quote pools are rare but handled explicitly.
- Platform/bot fees remain outside trade PnL (ADR-010 §5); PnL of bot users is an upper bound on
  their trade-leg PnL, with the gap reported.
