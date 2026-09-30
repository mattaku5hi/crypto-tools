# Live provider measurements — 2026-09-27

Per ADR-006: rows in `docs/p0/source-capability-matrix.md` only become
`live_verified` after an actual dated successful call, recorded here.
API keys are redacted in every request/response shown.

## Helius (Solana, free tier)

Key source: `SCOUT_HELIUS_API_KEY`, loaded from `.env` (gitignored, never
committed).

### `getTransactionsForAddress` — signatures mode

Request:
```json
POST https://mainnet.helius-rpc.com/?api-key=<redacted>
{"jsonrpc":"2.0","id":1,"method":"getTransactionsForAddress",
 "params":["5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1",
           {"transactionDetails":"signatures","limit":1}]}
```
Response: `200 OK`, one signature record returned with
`paginationToken`. **Verdict: works on free tier, no plan-gate error.**

### `getTransactionsForAddress` — full mode

Same address, `{"transactionDetails":"full","limit":2}` → `200 OK`,
full transaction objects (signatures, message, account keys) returned.
**Verdict: full mode also works on free tier** — this is the 10
credits/100-tx path, not the 1-credit-per-tx `getTransaction` path.

### `getSignaturesForAddress` (baseline, standard RPC)

`200 OK`, one signature returned. Baseline confirmed working
(expected — standard method, not Helius-exclusive).

### Archival depth — `getBlock` at slot 1,000,000

```json
{"jsonrpc":"2.0","id":1,"method":"getBlock",
 "params":[1000000,{"maxSupportedTransactionVersion":0,"transactionDetails":"none"}]}
```
`200 OK`, real block returned (blockhash, previousBlockhash present).
Slot 1,000,000 is deep into Solana mainnet's early history (~March
2020 era). **Verdict: free tier archival reach is not shallow-cut at
this depth** — does not by itself prove full genesis-to-present
retention; only this one point was tested.

### Webhooks — management endpoint

```
GET https://api.helius.xyz/v0/webhooks?api-key=<redacted>
```
`200 OK`, body `[]`. **Verdict: endpoint is live on free tier** (an
empty list, not an auth/plan error) — webhook *creation* was not
tested (out of scope while watchlist monitoring is deprioritized).

## Blockscout PRO API

Key source: `SCOUT_BLOCKSCOUT_API_KEY`, loaded from `.env`.

### Base (chain_id=8453)

```
GET https://api.blockscout.com/v2/api?chain_id=8453&module=account&action=balance&address=0x4200000000000000000000000000000000000006&apikey=<redacted>
```
`200 OK`, `{"message":"OK","result":"268365075953806562677616","status":"1"}`.
**Verdict: Base is live on PRO API as of this test**, despite a
vendor announcement about free-tier Base access ending Oct 1 — that
change had not yet taken effect at test time.

### Robinhood Chain (chain_id=4663)

Same call shape, valid 40-hex-char address →
`{"message":"OK","result":"4994906732124031958","status":"1"}`.
**Verdict: live.** (First attempt used a malformed 42-zero address and
correctly got `"Invalid address hash"` — not a chain-support failure,
a request-format error on our side.)

### BSC (chain_id=56)

Same call shape → `{"error":"Network not supported","source":"internal"}`.
**Verdict: confirmed unsupported** — hard, explicit rejection, not a
timeout or ambiguous error. Cross-checked against Blockscout's own
712-chain registry (`chains.blockscout.com/api/chains`): no BNB/BSC
match by name, direct `/api/chains/56` lookup returns `{"error":"Chain
not found"}`, and both `bsc.blockscout.com`/`bnb.blockscout.com`
return HTTP 404.

### Fallback: public `base.blockscout.com` (no PRO key)

```
GET https://base.blockscout.com/api?module=account&action=balance&address=0x4200000000000000000000000000000000000006
```
`200 OK`, real balance returned, **no API key required**. Same
Etherscan-compatible shape as the PRO endpoint (`module`/`action`
params, not `chain_id`). **Verdict: confirmed working free fallback**
if the PRO API's Oct 1 Base cutoff takes effect — same integration
code, just swap base URL and drop the `apikey`/`chain_id` params.

## Public EVM RPC — Base (no key)

```
POST https://mainnet.base.org
{"jsonrpc":"2.0","id":1,"method":"eth_getLogs",
 "params":[{"address":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"],
            "fromBlock":"latest","toBlock":"latest"}]}
```
`200 OK`, real USDC Transfer logs returned for the latest block, no
key required. **Verdict: point-indexing via `eth_getLogs` against a
known pool/token address works today on the public Base RPC** — this
is the mechanism the "own point index, not a full indexer" plan in
`docs/PROVIDERS.md`/`TICKETS.md` P4.x relies on.

## Codex (`graph.codex.io`, key registered, free "Almost free" tier)

Key source: `SCOUT_CODEX_API_KEY`, loaded from `.env`. Auth header is
`Authorization: <key>` with **no** `Bearer` prefix (confirmed working —
`Bearer` is reserved for short-lived JWTs from `createApiTokens`).

### `getNetworks` — coverage check

`200 OK`, 124 networks returned. **Confirmed present**: Solana
(`1399811149`), BNB/BSC (`56`), Base (`8453`), **Robinhood (`4663`)**.
This was the single highest-uncertainty question going in — Robinhood
Chain is obscure enough that "80+ networks" marketing copy could not
be trusted — and it resolves cleanly: all four networks this workspace
targets are covered.

### `filterTokens` — market-cap-floor filter on Solana

Query: `filters: {network: [1399811149], marketCap: {gt: 1000000},
liquidity: {gt: 10000}}, rankings: {attribute: trendingScore24,
direction: DESC}, limit: 25`. Numeric filter values must be raw
numbers, not quoted strings (`"Float cannot represent non numeric
value"` on first attempt with `"1000000"`).

`200 OK`, 25 results returned. **Caveat: this is not yet a "runners"
list** — the result set is dominated by long-established
majors/wrapped-assets (SOL, USDC, PUMP, MEW, wrapped WETH/BTC/ZEC), not
newly-graduated bonding-curve tokens. `trendingScore24` ranks by
current trading activity, not by launch recency — an additional
age/creation-time filter (or a different ranking attribute) is needed
to isolate actual daily graduations/runners. **Not yet a solved query
for the discovery use case** — the filter shape works, the specific
runner-isolation query does not exist yet.

### `tokenTopTraders` — schema exploration, not yet a working call

Introspection is disabled by Codex's router
(`"GraphQL introspection is disabled by Cosmo Router"`), so the exact
`TradingPeriod` enum values and `TokenTopTrader` field names could not
be discovered cheaply. Four guesses were tried and rejected by the
server's own validation errors (not auth/plan errors — schema
mismatches): `ONE_DAY`, `DAY_1`, `D1` all rejected as invalid
`TradingPeriod` enum values; `wallet` needs a subfield selection
(`wallet { address }` accepted the shape); `profitAndLossUsd` does not
exist on `TokenTopTrader`. **Stopped after 4 failed guesses rather
than continuing to probe blindly against the free-tier monthly
ceiling** — the exact enum/field names need the published schema
(`docs.codex.io/api-reference/queries/tokentoptraders` +
`docs.codex.io/api-reference/enums/tradingperiod`) read directly before
the next attempt, not further guessing.

**Verdict so far: key is valid and unblocked, network coverage is
confirmed complete for all 4 target chains, but neither of the two
higher-value discovery queries (isolated runner list, top-traders
shape) is proven working yet.** This is a partial result, not a dead
end — the fix is reading two schema reference pages before the next
probe, not more trial and error.

## Not yet measured (explicitly open)

- Webhook *creation* on Helius free tier (address-count limits per
  webhook, churn cost)
- `getTransactionsForAddress` actual credit consumption at scale (the
  10-credits/100-tx rate is documented, not independently metered here)
- dRPC/Ankr `eth_getLogs` chunking limits against BSC specifically
- Dune Solana public-table query cost/rate limits
- Axiom, GMGN skill-repo programmatic call reliability
