# Robinhood Doppler ATH supply/source audit — 2026-10-11

## Result

The six Robinhood candidates below have strong, source-linked evidence for an
initial supply of 1,000,000,000 tokens at 18 decimals. Their pinned-block
runtime is the same 44-byte delegate-proxy pattern and embeds the address of
the canonical Robinhood `DopplerERC20V1` implementation. The official Doppler
deployment table assigns that implementation and its factory to Robinhood
mainnet (chain 4663), commit `bda077cf`. At that commit, initialization mints
the configured initial supply; the implementation exposes `burn()` but no
public mint or rebase path was found. The saved token-scoped Transfer results
show one initial mint and no burn for each token, and the current `totalSupply`
matches 1B at the pinned block.

A second, explicitly ranged log query now covers all six tokens from block
41,698,793 (before their earliest creation at block 41,734,914) through the
pinned anchor. It returns exactly one 1B initial mint per token and zero burn
logs. Together with the verified implementation source and pinned current
supply, this establishes 1B supply through the captured anchor for these six
tokens. It does not make supply globally immutable: the public `burn()`
function can reduce it after the anchor.

This closes the main source/runtime question, but **does not establish a
lifetime ATH or justify an ATH FDV write**. Each token has one saved Birdeye
launch-date daily candle. Six unauthenticated DexPaprika token lookups returned
null `price_stats.ath`, `price_stats.ath_date`, current USD price, and FDV;
six pool searches returned no pool IDs. A bounded on-chain Swap-log check below
independently corroborates launch-day execution prices for five WETH pools, but
the evidence is limited to two launch-transaction swaps per pool and broad
same-minute Coinbase ETH/USD ranges. Both range endpoints imply USD prices
within 0.12% of the Birdeye launch-day highs for all five WETH pools. This is
strong launch-minute cross-provider corroboration, but not a lifetime ATH.

## Tokens and observed state

| Address | Symbol | Created (UTC) | Birdeye candle day | Birdeye daily high (USD/token) | `totalSupply` at block 85,674,112 |
|---|---|---:|---:|---:|---:|
| `0x6f0f78fc461ac6df9b663c71ff8dab8c59b9fee1` | TST | 2026-08-25 18:00:04 | 2026-08-25 | `0.000010064078484692887` | 1,000,000,000 (18 decimals) |
| `0x6fdbbfe8ba5778f46b05480f02d318eb6391fee1` | BANANA | 2026-08-21 00:11:04 | 2026-08-21 | `0.000010121365369902998` | 1,000,000,000 (18 decimals) |
| `0xa35b32456ffe354ac140deb48c20e65454cafee1` | KANAPE | 2026-08-20 21:13:44 | 2026-08-20 | `0.000010032732206030907` | 1,000,000,000 (18 decimals) |
| `0xbb6ee8de2ae40d6b4f190f3f4caa1dc9eae9fee1` | ERZHAN2 | 2026-08-23 21:58:53 | 2026-08-23 | `0.000010058889963219904` | 1,000,000,000 (18 decimals) |
| `0xbdb0ce555c126c1868d04e439aca019c1a1ffee1` | SMOGGY3 | 2026-08-26 22:02:23 | 2026-08-26 | `0.000010037876121664267` | 1,000,000,000 (18 decimals) |
| `0xfdd89ad1faa305d1fd10c96bddc2a769a680fee1` | TT3 | 2026-08-26 13:09:56 | 2026-08-26 | `0.000010175515385731835` | 1,000,000,000 (18 decimals) |

The single candle for each token falls on its launch UTC day. A launch-day
daily candle is not lifetime history and has no intraday ATH timestamp.

## Evidence and bounded requests

The official [Doppler contract address table](https://docs.doppler.lol/reference/contract-addresses.md)
lists Robinhood Mainnet (4663), `DopplerERC20V1` at
`0x3be8b97fd0e713b5abe0649fa830223b6b4bc599`, and
`DopplerERC20V1Factory` at `0x1b37d3a72082029c44b35b604ea473617580b69a`,
both at commit `bda077cf`. Doppler warns that contracts absent from this
table are not canonical. The linked
[implementation source at that exact commit](https://github.com/whetstoneresearch/doppler/blob/bda077cf/src/tokens/DopplerERC20V1.sol)
shows the one-time initializer minting `initialSupply` (including vested
allocations); its public [`burn(uint256)`](https://github.com/whetstoneresearch/doppler/blob/bda077cf/src/tokens/DopplerERC20V1.sol#L1898-L1901)
burns from the caller. The corresponding
[factory source](https://github.com/whetstoneresearch/doppler/blob/bda077cf/src/tokens/DopplerERC20V1Factory.sol)
is the source version named by the deployment table.

At the pinned block number 85,674,112, hash
`0x4f2df60a5b4d827f31e31d9664fd7f3fe4009e5ea2ccb026e53b238cab61e138`, eight
read-only `eth_getCode` calls (six candidates, implementation, factory) all
returned HTTP 200. Each candidate runtime was exactly
`0x3d3d3d3d363d3d37363d733be8b97fd0e713b5abe0649fa830223b6b4bc5995af43d3d93803e602a57fd5bf3`
(44 bytes; SHA-256
`006870377d147cf66d7e15949f19a89773097d4806b67f9743a01e440e48d4fb`),
which embeds the official implementation address. This is a Doppler
delegate-proxy pattern; it is not claimed to be the byte-for-byte canonical
EIP-1167 runtime. The implementation code returned at that address was
13,927 bytes, SHA-256
`0706fe921e1dbdae7b9c46e28a278c14caab0cd86128d6b46f0832287079f33f`.
The chain ID read was `0x1237`.

The pinned-block RPC snapshot records `totalSupply()` =
`1000000000000000000000000000` and `decimals()` = 18 for all six. It records
one mint Transfer for each token, zero burn Transfers returned, and the
minted raw amount matching current supply. The explicit log queries use
`eth_getLogs`, `fromBlock=0x27c45e9` (41,698,793), `toBlock=0x51b4880`
(85,674,112), address filter containing all six candidates, and Transfer
topics indexed from zero (mint) or to zero (burn). Both returned HTTP 200;
raw parameters and responses are retained with SHA-256. The first-party
[Sourcify API documentation](https://docs.sourcify.dev/docs/api/) documents
`GET /server/v2/contract/{chainId}/{address}?fields=all`. The correct v2
lookup returned 404/no match for all six token clones. A separate correct v2
lookup of the implementation returned `runtimeMatch=match`, `match=match`,
verified at `2026-08-08T06:52:36Z`, with no immutable references or runtime
transformations. Its Sourcify on-chain and recompiled runtime SHA-256 both
match the pinned RPC code (`0706fe…f33f`); compiler is solc
`0.8.26+commit.8a97fa7a`. This confirms the official implementation source
against deployed runtime, while clone source matches remain unavailable.
Blockscout returned 403 on the first attempt and was not retried.

## Creation-transaction pool evidence

Six saved `eth_getTransactionReceipt` responses (all receipt status `0x1`)
were inspected without another lookup. Each creation transaction contains
exactly one Uniswap v4 `Initialize` event emitted by
`0x8366a39cc670b4001a1121b8f6a443a643e40951`, the Robinhood PoolManager in
[Uniswap's official deployment table](https://developers.uniswap.org/docs/protocols/v4/deployments).
The event signature and decoder match the project's existing
[`V4Initialize` decoder](../../../crates/scout-dex-evm/src/uniswap.rs) and
the pool manager is already `FixtureVerified` for Robinhood in
[`gate.rs`](../../../crates/scout-dex-evm/src/gate.rs). The project identifies
`0x0bd7…ad73` as Robinhood WETH in its chain configuration and prior
[network audit](../evm-p0-research-2026-10-03.md).

| Token | Pool ID from `Initialize` | Currency paired with token |
|---|---|---|
| TST | `0x7e20b781ef38b88d28d953219dc1fdd17c9f0e87b84d7b3a3b553ab3cc18542a` | WETH `0x0bd7d308f8e1639fab988df18a8011f41eacad73` |
| BANANA | `0x2659fb47f7e7389f9244bb61c6c88067621c7722120c32bbad52b147cab50838` | WETH `0x0bd7d308f8e1639fab988df18a8011f41eacad73` |
| KANAPE | `0xf97fb785e02f8e00f00cfdd7eee5f2af28a1b300691f968884fe96a7a8d6657d` | WETH `0x0bd7d308f8e1639fab988df18a8011f41eacad73` |
| ERZHAN2 | `0xee31d0c1676404c1556d2732087ce8debaeee036f2986ea16100c61809e08153` | WETH `0x0bd7d308f8e1639fab988df18a8011f41eacad73` |
| SMOGGY3 | `0x8415358f75e4ad6bf6483baee586709f56ecb832e472e3d68d8f9447adb09936` | `0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec` (`NVIDIA • Robinhood Token`, symbol `NVDA`; no USD quote conversion) |
| TT3 | `0x38233cd641d47b9fa952fde12a581f0af9c5b800a97d39a5a148d3a78dfbce18` | WETH `0x0bd7d308f8e1639fab988df18a8011f41eacad73` |

This establishes the initial pool route for each launch transaction, not that
it is the only pool over the token's lifetime. Receipt raw bodies and per-file
SHA-256 values are in the private `creation-receipts/manifest.json`. The earlier
receipt helper did not retain HTTP status codes, so only the successful
JSON-RPC receipt contents and on-chain receipt status are asserted.


## Bounded Swap-log and USD-range crosscheck

The six initial pool IDs were queried at the PoolManager from each pool's
Initialize block through anchor block 85,674,112. Two indexed Initialize
queries searched for all six token addresses in either currency position;
they returned exactly six events, one per token. Each per-pool Swap query
returned exactly two logs. The five WETH-pool logs are in the token creation
transaction; SMOGGY3's two logs are in a later transaction on its launch day.
The event signature and signed amount fields follow the project's existing
[`V4_SWAP` decoder](../../../crates/scout-dex-evm/src/uniswap.rs). For the
five WETH pools, each pair of signed deltas has opposite signs; exact integer
raw amounts yield the WETH/token execution ratios below. Cached successful
creation receipts independently match the winning Swap log's transaction,
block hash, log index, and amounts for all five.

One public Kraken daily ETH/USD candle was fetched for each UTC launch day
from its cached year-history response. Its low/high range bounds the possible
USD value of each WETH/token ratio, but it cannot recover the ETH/USD rate at
the swap timestamp. `Required ETH/USD` is the rate needed to exactly equal the
Birdeye launch-day high. None of the five cases is within 1% at both endpoints.

| Token | UTC day | Maximum observed WETH/token | Required ETH/USD for Birdeye high | Kraken daily low/high | Difference at low/high |
|---|---|---:|---:|---:|---:|
| TST | 2026-08-25 | `4.0738173948564027415e-9` | 2470.4295 | 2415.31 / 2531.92 | -2.23% / +2.49% |
| BANANA | 2026-08-21 | `4.3411033602918765965e-9` | 2331.5191 | 2324.05 / 2546.62 | -0.32% / +9.23% |
| KANAPE | 2026-08-20 | `4.3257200345367477690e-9` | 2319.3207 | 2221.35 / 2357.88 | -4.22% / +1.66% |
| ERZHAN2 | 2026-08-23 | `4.0745767199514205005e-9` | 2468.6957 | 2355.47 / 2484.00 | -4.59% / +0.62% |
| TT3 | 2026-08-26 | `4.1560934886735692837e-9` | 2448.3365 | 2432.08 / 2514.64 | -0.66% / +2.71% |
| SMOGGY3 | 2026-08-26 | `4.5794883542494217079e-8` NVDA-token/token | — | tokenized-equity quote | not converted |

This initial daily comparison was subsequently narrowed with exact-minute
Coinbase Exchange ETH/USD candles; the refinement is recorded below. The
SMOGGY3 pool's other currency is `0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec`;
anchor metadata reads name `NVIDIA • Robinhood Token`, symbol `NVDA`, and 18
decimals. Sourcify verifies its 283-byte OpenZeppelin `BeaconProxy` runtime
(SHA-256 `399ec4bc5b43db03486ceae11f9a6fc5c126427f8f8e10c90fed0c773f4325c6`),
which matches the pinned on-chain code. At the winning Swap block and anchor,
the beacon slot points to `0xe10b6f6b275de231345c20d14ab812db62151b00`; its
verified `AccessControlsRegistry.implementation()` returns the same
`0xb35490d6f9163de4f80d88dc75c3516eb64c5ae2` implementation at both blocks.
That implementation's pinned code matches Sourcify on-chain runtime for
`src/Stock.sol:Stock` (11,614 bytes; SHA-256
`0ac99e9cc7a3738645af72f6d9e8fb8242660b63134d8fb0c44ba6fac39deec2`);
Sourcify reports `runtimeMatch=exact_match` with its recorded immutable
substitutions. The
verified `Stock.terms()` returns Robinhood's stock-token terms path. `Stock`
contains no USD price getter; `OraclePausable` exposes only a pause flag. The
contract has role-gated mint/burn/adminBurn and UI-multiplier updates. At the
Swap block the UI multiplier was `1e18`; at the anchor it was
`1000775159164630595`. Verified source shows this multiplier scales UI balances
and total supply, not the raw ERC20 amounts used in the pool Swap. The token's
on-chain stock-price/underlying conversion is not provided here, so NVDA cannot
be treated as USD and no quote conversion was attempted. The EIP-1967
implementation slot returned zero; this `BeaconProxy` instead uses its beacon
slot. The Sourcify source responses and twelve bounded RPC calls are archived
with raw hashes in
`.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/`.
The SMOGGY3 maximum is in a later transaction than the cached creation receipt
and was not receipt-verified.

These logs show launch-transaction seed/initialization context rather than
independent trader activity: only two Swap events were present in each known
initial pool, and all five WETH-pool maxima are in the creation transaction.
The queries do not cover other pool managers or venues, so they cannot establish
market-wide or lifetime maxima. The raw RPC and Kraken response bodies, exact
request parameters, ratios, and per-file SHA-256 values are recorded in
`.scout-data/diagnostics/robinhood-doppler-price-crosscheck-2026-10-11/manifest.json`
and its sibling manifests. Kraken's [OHLC API documentation](https://docs.kraken.com/api-reference/market-data/get-ohlc-data)
notes that the last candle is current and uncommitted; comparisons use the five
closed daily candles.

## Minute-level ETH/USD refinement

To narrow the broad Kraken daily bounds, six public Coinbase Exchange GETs
retrieved `ETH-USD` product metadata once and one three-minute request window
around each of the five WETH Swap event minutes. The metadata identifies the
market's base as `ETH` and quote as `USD`; each response included the exact
UTC minute candle containing the Swap timestamp. Coinbase documents the
candle tuple as `[time, low, high, open, close, volume]`, permits 60-second
candles, and warns that historical intervals with no ticks are omitted; all
five requested event minutes had a returned candle. The paired RH address is
the WETH address listed in [Robinhood's protocol-contract reference](https://docs.robinhood.com/chain/protocol-contracts)
and the repository's Robinhood chain profile. The corrected Sourcify v2 lookup and pinned-block `eth_getCode` match the
2,202-byte transparent-proxy runtime exactly (SHA-256
`eb5a5f69…d360a7`). Sourcify resolves the proxy to implementation
`0xC6B81b429797E0f555440b70cD99e032D7AE947e` (`aeWETH`). A separate verified
Sourcify reports `runtimeMatch=match` for this implementation, with a recorded
CBOR metadata-only recompilation difference. The pinned RPC runtime exactly
matches Sourcify's on-chain runtime; its
pinned-anchor runtime is 6,961 bytes, SHA-256
`55b626d0ffe56312da49df2fda047bf9d89ad7c565cad341efe76a496fcad043`. At each
of the five observed Swap blocks and the pinned anchor, the proxy's EIP-1967
implementation slot contains this same address. The earlier HTML 404 was from
an incorrect lookup URL and is retained only as a failed request record.

The verified `aeWETH` source makes the quote conversion explicit: `deposit`
and `depositTo` mint one token unit per wei of native ETH; `withdrawTo` burns
the same amount and transfers that amount of native ETH. Its bridge mint path
reverts, while the gateway-only bridge burn sends the burned amount of ETH.
This establishes a 1:1 native-ETH conversion at the observed blocks, where the
historical proxy slot points to that verified implementation. Multiplying the
exact token/WETH execution ratio by the Coinbase minute low and high therefore
produces an independently grounded range for token USD price; both endpoints
are within 1% of Birdeye's daily high in all five cases. This is close
launch-minute corroboration, but a two-event launch pool history is not a
complete lifetime ATH source.

| Token | Swap UTC minute | Coinbase ETH/USD low/high | Implied token USD low/high | Difference from Birdeye high |
|---|---|---:|---:|---:|
| TST | 2026-08-25 17:59 | 2470.45 / 2471.48 | 0.00001006416 / 0.00001006835 | +0.0008% / +0.0425% |
| BANANA | 2026-08-21 00:11 | 2330.79 / 2332.86 | 0.00001011820 / 0.00001012719 | -0.0313% / +0.0575% |
| KANAPE | 2026-08-20 21:13 | 2320.00 / 2321.99 | 0.00001003567 / 0.00001004427 | +0.0293% / +0.1151% |
| ERZHAN2 | 2026-08-23 21:58 | 2466.61 / 2470.22 | 0.00001005039 / 0.00001006510 | -0.0845% / +0.0617% |
| TT3 | 2026-08-26 13:09 | 2447.80 / 2449.60 | 0.00001017329 / 0.00001018077 | -0.0219% / +0.0516% |

Raw Coinbase bodies, request parameters, metadata, exact Decimal results, and
SHA-256 values are in
`.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/manifest.json`.
The API semantics used here are documented by Coinbase's [product candles](https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-candles)
and [single product metadata](https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-single-product) references. The RH WETH proxy source and runtime manifests are retained in
`.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/sourcify-weth-corrected-manifest.json`
and `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/robinhood-weth-code-manifest.json`. The verified implementation source and seven-call historical slot/runtime check are recorded in `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/sourcify-aeweth-manifest.json` and `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/aeweth-implementation-slot-manifest.json`. The earlier incorrect-URL response remains separately preserved in `sourcify-weth-manifest.json`.

For independent price discovery, six no-auth DexPaprika token-latest GETs
returned HTTP 200 but had null `price_stats.ath`, `price_stats.ath_date`,
`summary.price_usd`, and `summary.fdv`. Six no-auth token pool-search GETs
returned HTTP 200 with `results=[]` and `has_next_page=false`. Those empty
provider results mean this provider pass supplied no price or pool data; they
do not prove that the tokens have no pools. No Codex, GeckoTerminal, Birdeye,
database write, or additional Blockscout call was made in this pass.

Private raw response bodies and manifests (with per-response SHA-256):

- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/rpc-runtime-manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/rpc-diagnostic-manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/sourcify-v2-remaining/manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/sourcify-implementation-manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/rpc-transfer-range-manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/creation-receipts/manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/dexpaprika/manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/dexpaprika-pool-index/manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-price-crosscheck-2026-10-11/manifest.json`
- `.scout-data/diagnostics/robinhood-doppler-price-crosscheck-2026-10-11/kraken-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/sourcify-weth-manifest.json` (incorrect URL; retained as a failed attempt)
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/sourcify-weth-corrected-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/robinhood-weth-code-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/sourcify-aeweth-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/aeweth-implementation-slot-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/aeWETH.sol.verified-source.sol`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/L2GatewayToken.sol.verified-source.sol`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-quote/sourcify-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-quote/rpc-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/sourcify-manifest.json`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/implementation-Stock.sol`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/beacon-AccessControlsRegistry.sol`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/implementation-src__OraclePausable.sol`
- `.scout-data/diagnostics/robinhood-minute-fx-crosscheck-2026-10-11/smoggy-beacon/implementation-src__ERC20ScaledUIUpgradeable.sol`
- Earlier candidate/window summary: `.scout-data/diagnostics/robinhood-doppler-source-verification-2026-10-11/manifest.json`

All saved response SHA-256 values in the RPC/Sourcify/DexPaprika manifests,
including the Coinbase minute-FX, corrected WETH implementation, and SMOGGY3
beacon/stock-token checks, were rechecked against their preserved raw transport
files on 2026-10-11; all matched. The RH WETH proxy's implementation source and per-observation implementation
slots were checked; this validates the 1:1 wrapper conversion at the five
observed swap blocks, not completeness of the token's lifetime price history.

## Public historical price source for the SMOGGY3 NVDA quote

The no-auth Robinhood stock-token API documents `/rhj/assets` (asset identity
and current multiplier), `/rhj/prices/{symbol}` (live bid/ask), and
`/rhj/corporate-actions` (corporate-action records). The documented price
endpoint is live and explicitly returns raw underlying-equity bid/ask, not a
historical series; the docs describe its currency as USD at launch. The exact
contract match in a read-only `GET https://api.robinhood.com/rhj/assets` was
`NVDA`, `NVIDIA • Robinhood Token`, address
`0xd0601CE157Db5bdC3162BbaC2a2C8aF5320D9EEC`, chain ID 4663, 18 decimals,
current multiplier `1.000775159164630595`. Response: HTTP 200, 162,103 bytes,
SHA-256 `f8c058b28cd8697494ff1c40574014492953fc4594b622601dcf66e6fc1944df`;
preserved at
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/robinhood-rhj-assets.response.json`.
This confirms the endpoint's exact asset mapping, but not an historical quote.
[Robinhood Stock Token APIs](https://docs.robinhood.com/chain/stock-token-apis/)
describe only live prices and corporate actions.

Robinhood and Chainlink document a separate on-chain Chainlink V3 feed for each
Robinhood stock token. Its answer is the stock token's total-return value
(underlying equity price multiplied by the token's `uiMultiplier`); historical
round reads use `getRoundData(roundId)`. This is a plausible historical source
when its exact feed address and an archive-capable RPC or indexed rounds are
available. [Robinhood Oracles & Price Feeds](https://docs.robinhood.com/chain/oracles-and-price-feeds/),
[Chainlink's Robinhood feed description](https://docs.chain.link/data-feeds/tokenized-equity-feeds/robinhood),
and [Chainlink historical-round documentation](https://docs.chain.link/data-feeds/historical-data)
establish the semantics and read method. The Robinhood docs' available-feed
table is client-rendered and did not expose the NVDA feed address in the
retrieved text; the initial docs-only pass did not find a first-party off-chain
historical chart/API.

For a capability check, one unauthenticated call to Robinhood's documented
public RPC at the SMOGGY3 winning Swap block (`0x2cc0ffe`) requested
`decimals()`, `description()`, and `latestRoundData()` from the candidate NVDA
feed address `0x379EC4f7C378F34a1B47E4F3cbeBCbAC3E8E9F15`. The RPC returned
`historical state ... is not available` for all three calls. A bounded
`eth_getLogs` query over blocks `0x2cbc1de`–`0x2cc5e1e` returned no logs for
that address; this sample is not evidence that no feed updates exist outside
that block range. Both raw responses and hashes are in
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/`. The RPC URL
is the public no-auth URL listed by [Robinhood's network setup docs](https://docs.robinhood.com/chain/add-network-to-wallet/).

A non-Robinhood candidate does provide public, keyless, chain-derived NVDA/USDG
candles: [StockKit Market API](https://docs.stockkit.dev/products/market-api/)
documents `/v1/history/{symbol}` as five-minute samples from the deepest
Uniswap V3 `TOKEN/USDG` pool, aggregated into OHLC candles. Its `/v1/stats/NVDA`
endpoint explicitly binds the symbol to this exact NVDA contract and chain ID.
Both documented GETs returned HTTP 200 without credentials in this pass. Stats
response: 1,124 bytes, SHA-256
`85d91257e3fc8ad9be7fdf3460718f1213991658a44cff54aaf67573213c252b`; exact
token address matched and four TOKEN/USDG pools were listed. The 300-candle
history request (`interval=5m&limit=300`) returned 43,888 bytes, SHA-256
`3f19fa041974a2b05916a9d719bf178199cdecfba4c49c987ed65820515e9002`; its
`historySince` was `2026-09-16T05:00:00.000Z`, after SMOGGY3's 2026-08-26 Swap
date. The provider documents no backfill before September 16. Therefore it is a
useful keyless source for later exact-token market history, but cannot produce
the requested dated quote. The candle values are JSON numbers and the
five-minute sampling may miss between-sample spikes, and the series is quoted
against USDG (not direct USD); no USDG/USD conversion was assumed. This endpoint
alone does not prove a lifetime ATH. Responses are archived in
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/stockkit-nvda-stats.response.json`
and `stockkit-nvda-history.response.json`.

### Chainlink historical-round follow-up

The first-party [Chainlink Robinhood feed directory](https://reference-data-directory.vercel.app/feeds-robinhood-mainnet.json)
resolves the NVDA proxy as `0x379EC4f7C378F34a1B47E4F3cbeBCbAC3E8E9F15`.
Its row is named `Robinhood NVDA / USD`, identifies the asset as “Nvidia
(Robinhood Tokenized Equity),” has `baseAsset=NVDA`, `quoteAsset=USD`,
`decimals=8`, `heartbeat=86400`, and 24/5 equity market hours. The directory's
`multiply=100000000` field is separate from the token's `uiMultiplier`;
Chainlink's docs say its token price already includes the on-chain multiplier.
The separate
Robinhood asset registry response above binds NVDA on chain 4663 to the exact
quote token `0xd0601CE157Db5bdC3162BbaC2a2C8aF5320D9EEC`. The directory response
was HTTP 200, 84,653 bytes, SHA-256
`714038abef5e6290ce09d4f02279382ff0dcd36e573b8d745ba38d9d316bf776`, archived
at `.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/chainlink-rdd-robinhood-mainnet.json`.

At SMOGGY3's winning Swap block `0x2cc0ffe` (block timestamp
`2026-08-26T22:02:26Z`), a historical `eth_call` through the configured archive
RPC returned `description() = RHNVDA / USD`, `decimals() = 8`, and
`latestRoundData()` round `18446744073709552491`, answer raw `21888650000` =
`$218.8865`, `updatedAt=2026-08-26T21:56:38Z`. The round was published 5m48s
before the Swap; it is a USD oracle mark at the Swap block, not a same-second
market execution, so the age is material. The block-pinned response is 696
bytes with SHA-256
`4558c032f452ab29b4c068f6ed44528c12e09c6039f93f37cd5587b27da0782c`. A
current-state `getRoundData(18446744073709552491)` on Robinhood's public no-auth
RPC returned the same answer and timestamp (360 bytes; SHA-256
`0d3680dbcf8ace9ffdfd6268ecc93b14b47f0abd89ccaede499c67c000ffc330`). Thus
historic rounds remain callable without archive-state access. Both responses
are archived in `.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/`.

Applying this oracle answer once to the observed pool ratio
`4.5794883542494217079e-8 NVDA/token` gives
`$0.0000100238817765241604466625335` per SMOGGY3 token, 0.1394% below Birdeye's
launch-day high `0.000010037876121664267`. The Chainlink answer already includes
the NVDA token's multiplier; do not apply it again. This is an oracle-based USD
conversion of the observed Swap ratio, not a USD-denominated Swap. The actual
pair traded SMOGGY3 against NVDA, so a fiat execution price was not directly
observed. The separate StockKit candles remain a useful post-September
USDG-market source but do not cover this August event.

### NVDA/USDG pool check at the SMOGGY3 swap

StockKit's saved NVDA stats response identifies the deepest current NVDA/USDG
pool as `0xd4EB21209C4D6093f80B5b84f5C45cc093EA14a3` (500 fee tier). Its
historical endpoint's September 16 start is an index start, not proof of pool
birth. An archive `eth_getCode` at the SMOGGY3 block `0x2cc0ffe` returned
nonempty code (22,142 bytes; response SHA-256
`4b99550e4fe42e7eee1905890b79c731b46ae9cba75a08cf0da98df8bf2ac287`). At that
block the pool reported factory
`0x1f7d7550b1b028f7571e69a784071f0205fd2efa`, token0
`0x5fc5360d0400a0fd4f2af552add042d716f1d168`, token1 equal to the exact NVDA
token `0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec`, fee 500 and tick spacing
10. A call to the factory's `getPool(USDG,NVDA,500)` returned this same pool;
at the block, USDG had 6 decimals and NVDA 18. The factory matches Uniswap's
official [Robinhood Chain v3 deployment](https://developers.uniswap.org/deployments).
The pool metadata response is 519 bytes (SHA-256
`bfe9267d6bfe9d60cda589e804061674deab8bbb60ef57df12eea9e79e432a0d`); the
factory lookup and token-decimal response is 310 bytes (SHA-256
`4a89507018c64e9a8686a15f3895b15fd04fc729127a956d1c910874e19a50d7`).

A direct `eth_getLogs` request over ±30,000 blocks failed with HTTP 400; a
10,000-block request exceeded the 2 MiB response cap and was discarded. The
bounded ±2,000-block query succeeded with 1,631 Swap logs in 1,451,742 bytes
(SHA-256
`c00bb06a768791bbabc42cc1a92cfecc991b9b0434f1d34a9e11ec6be602e7e7b`). The
nearest pool Swap before the target block is at `0x2cc0ffb`, timestamp
`2026-08-26T22:02:25Z`; the nearest after is at `0x2cc1003`, timestamp
`2026-08-26T22:02:26Z`. Their block hashes, plus the query's nearby block
headers, are archived at
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/stockkit-usdg-pool-nearpeak-blocks.response.json`
(46,649 bytes; SHA-256
`c76d6fca3864137c70a391fa91ac3866cf8faa8cf82d3a2d86066e285d242eb8`). The
nearby Swap response is at
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/stockkit-usdg-pool-swaps-near-peak.response.json`.
Uniswap's v3 core interface defines Swap `amount0` and `amount1` as signed
pool-balance deltas ([official event ABI](https://github.com/Uniswap/v3-core/blob/main/contracts/interfaces/pool/IUniswapV3PoolEvents.sol)).
With token0=USDG and token1=NVDA, the nearest pre-swap event has deltas
`-48,743,613` USDG base units and `222,601,496,439,278,724` NVDA base units;
the two nearest post-swap events have `(-29,569,025,
135,035,833,331,963,918)` and `(-29,162,858,
133,181,331,594,850,372)`. Their absolute executed ratios are respectively
`218.97253064197747416999045823961413274745924220711`,
`218.97169270107215882365152186073728842363640890928`, and
`218.97106486903168587059976431746718564212516034240` USDG per NVDA. These
confirm nearby real pool trades on both sides, but do not themselves denominate
the quote in USD.

The first-party [Chainlink Robinhood feed directory](https://reference-data-directory.vercel.app/feeds-robinhood-mainnet.json)
lists a USDG/USD feed with 8 decimals and proxy
`0x61B7e5650328764B076A108EFF5fa7282a1B9aD2`. Its block-pinned answer at the
SMOGGY swap block was raw `1600282720` (directory-scaled `$16.0028272`), with
`updatedAt=2026-08-26T15:27:20Z`, 23,706 seconds before the target swap. This is
far outside the five-minute window and is not a plausible conversion to use
without resolving the feed's value/semantics. A keyless Coinbase Exchange
product lookup for `USDG-USD` returned HTTP 404 (`{"message":"NotFound"}`;
22 bytes, SHA-256
`78b49965ff92e946ac350d326d6f398777ec3da2e89ddacf20d04ece20efff1e`). No
dated market FX conversion was established. Keyless one-minute Coinbase
candle requests for `USDG-USDT` also returned HTTP 404 with the same NotFound
body and hash; `USDT-USD` returned HTTP 200, and the requested 22:02Z candle
reported low `0.99992`, high `0.99997`, open `0.99996`, close `0.99996`. Without
a USDG-USDT market, those data cannot form the needed cross. Coinbase documents
its public [product-candle endpoint](https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-candles)
as historical grouped buckets; the missing USDG leg leaves conversion unknown.
StockKit documents that its
historical candles are five-minute samples of its selected TOKEN/USDG pool and
that its history begins September 16, 2026 ([Market API documentation](https://docs.stockkit.dev/products/market-api/));
this explains the gap in that API's retained series but does not fill this
August price window. The exact pool trades therefore remain a USDG-denominated
corroboration only, and do not qualify the SMOGGY3 observation as a USD ATH.

The metadata, log, block-header, Chainlink and keyless market-product responses
are preserved under
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/`. This check
used the remaining 30-call read-only RPC budget; the two over-cap/rejected log
requests were not retried. No database or ATH record was changed.

Four neighboring rounds were queried through the archive RPC at the pinned
Swap block and through the public RPC at `latest`, using `getRoundData` for
rounds 874–878 (the peak is round 875). At the pinned block, round 874 exists
but rounds 876–878 return the proxy's no-data sentinel (zero answer and
timestamps), as expected for rounds not yet published at that block. At
`latest`, rounds 874 and 876–878 all return positive answers with
`answeredInRound == roundId` and `startedAt <= updatedAt`:

| Round | Answer (USD) | `updatedAt` (UTC) | Relative to swap |
| --- | ---: | --- | ---: |
| 874 | 220.66955 | 2026-08-26 21:54:37 | 7m49s before |
| 875 | 218.88650 | 2026-08-26 21:56:38 | 5m48s before |
| 876 | 217.65435 | 2026-08-26 22:23:08 | 20m42s after |
| 877 | 218.78415 | 2026-08-26 22:46:38 | 44m12s after |
| 878 | 219.89000 | 2026-08-27 01:27:11 | 3h24m45s after |

Multiplying the two preceding oracle marks by the same observed
`4.5794883542494217079e-8 NVDA/token` ratio gives respectively
`$0.00001010553634362460476042524445` (0.6741% above) and
`$0.0000100238817765241604466625335` (0.1394% below) Birdeye's
`$0.000010037876121664267` launch-day high. These are nearby oracle marks, not
an event-time price bracket: both precede the Swap by more than five minutes,
while the first subsequent round is 20m42s later. This evidence does not
satisfy the existing five-minute freshness policy and does not justify
changing that policy after observing the match. It is independent corroboration
of the approximate USD level only; it does not turn the token/NVDA Swap into a
USD execution or establish a canonical lifetime ATH.

The latest-state neighbor response is archived at
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/chainlink-nvda-neighbor-latest.response.json`
(1,441 bytes, SHA-256
`21c9bee2c33ce04bf2771af5c86669225a2caaa6cd0f98764d899897d8246e93`), with
decoded fields alongside it. The block-pinned neighbor response is archived at
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/chainlink-nvda-neighbor-smoggy-block.response.json`
(1,439 bytes, SHA-256
`6402da516a7f85f170527a388e94eb372f1b02076bd98ecd5253da3c7baf8ad9`); its
decoded artifact records zero sentinels for future rounds. All queried IDs
carry phase 1 in the high round-ID bits; no separate phase-aggregator lookup
was needed for these same-phase reads.

### USDG identity and dated-market FX follow-up (2026-10-11)

The exact Robinhood quote contract is Global Dollar, not merely a token with a
USDG ticker. Paxos' [mainnet deployment table](https://docs.paxos.com/guides/stablecoin/usdg/mainnet)
lists Ethereum USDG at `0xe343167631d89B6Ffc58B88d6b7fB0228795491D`, Robinhood
USDG at `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`, and the separate Robinhood
OFT at `0x0d54755f5106BfdB43f7a35f5D49a23F940628d1`. At the SMOGGY swap block,
the USDG proxy returned `name=Global Dollar`, `symbol=USDG`, and `decimals=6`;
its EIP-1967 implementation slot resolved to
`0x68184c449e1a8f34fa18d289737129fd27b66f8f`. The official OFT's `token()`
returned the exact USDG proxy, `sharedDecimals()` returned 6, and
`decimalConversionRate()` returned 1. The [LayerZero OFT core](https://github.com/LayerZero-Labs/LayerZero-v2/blob/main/packages/layerzero-v2/evm/oapp/contracts/oft/OFTCore.sol)
defines the conversion rate as `10 ** (localDecimals - sharedDecimals)`;
therefore this token's local and shared units convert one-for-one. The OFT
reported interface `0x02e49c2c`, version 1. Calls to `l1Address()` and
`l2Gateway()` on the USDG proxy reverted, so this evidence does not identify it
as an Arbitrum `L2GatewayToken`.

This identity makes Kraken's actual USDG/USD market applicable without assuming
a peg. Kraken's public [asset-pairs endpoint](https://api.kraken.com/0/public/AssetPairs?pair=USDGUSD)
reported `USDGUSD`, `wsname=USDG/USD`, `base=USDG`, `quote=ZUSD`, and
`status=online`. Its public [OHLC endpoint](https://api.kraken.com/0/public/OHLC?pair=USDGUSD&interval=1440&since=1787616000)
returned the closed 2026-08-26 UTC daily candle at timestamp `1787702400`:
low `0.9999`, high `1.0001`, volume `565396.41367` USDG, and 994 trades.
This daily range is the conversion bracket; it is not an event-minute FX quote.

Applying that range to the three nearest exact Uniswap v3 USDG/NVDA executions
around the target SMOGGY swap (one second before and two executions in the same
timestamp second after) gives token USD price intervals:

| Side | Observed USDG/NVDA | Implied token USD interval | Difference from Birdeye high |
| --- | ---: | ---: | ---: |
| Before | 218.97253064197747416999 | `$0.00001002681875760062943–$0.00001002882432190858035` | `-0.1102%` to `-0.0902%` |
| After 1 | 218.97169270107215882365 | `$0.00001002678038803179564–$0.00001002878594466506533` | `-0.1105%` to `-0.0906%` |
| After 2 | 218.97106486903168587060 | `$0.00001002675163941176745–$0.00001002875719029473810` | `-0.1108%` to `-0.0908%` |

The comparison target is Birdeye's launch-day high `$0.000010037876121664267`.
The independent market evidence supports the same USD price level to within
0.111% using real pool executions and dated USDG/USD trades. Scope remains the
identified pool and nearby executions; this does not prove all-pool or
lifetime-history completeness and does not by itself establish a canonical
lifetime ATH. The earlier stale Chainlink USDG/USD observation and Coinbase
product 404 remain recorded above as separate, weaker/failed paths.

This follow-up used 12 logical Robinhood RPC calls, two public Kraken GETs, and
one Sourcify GET, all within the authorized bounds; no database or ATH record
was written. Response bodies and SHA-256 hashes are recorded in
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/usdg-market-crosscheck-manifest.json`.

### Receipt and pool-source verification for USDG/NVDA spot checks (2026-10-11)

The three nearby USDG/NVDA Swap events used in the FX comparison were checked
against individual canonical receipts. Each receipt has `status=0x1`; its
transaction hash, block number/hash, pool address, Uniswap v3 Swap topic, log
index, and event data match the already archived `eth_getLogs` response. The
receipt block hashes also match the separately archived block headers. The
block-pinned pool code is nonempty and byte-for-byte equals Sourcify's onchain
runtime bytecode for the same Robinhood Chain pool address; Sourcify reports
`match`, `creationMatch=match`, and `runtimeMatch=match` for chain ID 4663.

This adds transaction and deployment provenance to the independent spot-price
cross-check. It is limited to the identified USDG/NVDA pool and those nearby
trades. It does not register Robinhood v3 as supported by the application's
runtime decoder gate, nor establish complete pool/lifetime ATH coverage. The
receipt, code, source response, block-hash comparisons, and SHA-256 values are
archived in
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/smoggy-nearby-trade-receipts-and-poolcode.manifest.json`
and its referenced raw response files. This used four read-only Robinhood RPC
calls and one Sourcify GET; no database or ATH record was changed.

### SMOGGY3 maximum v4 Swap receipt (2026-10-11)

The maximum observed SMOGGY3 launch-pool execution is the later of the two
PoolManager Swap logs in transaction
`0x2d2c053158a83cb5af079355850fecf6a29bf929e18077a53fcc852c9c05d51f`.
Its receipt has `status=0x1`, block `0x2cc0ffe`, and block hash
`0xb5b744624dfe20b66c44b77867fc92dca2c4c36b6aaf875d7439062281fbf962`.
The selected event is log index `0x0f`, emitted by Robinhood's PoolManager
`0x8366a39cc670b4001a1121b8f6a443a643e40951` for pool ID
`0x8415358f75e4ad6bf6483baee586709f56ecb832e472e3d68d8f9447adb09936`.
Its complete topics and data exactly match the existing raw `eth_getLogs`
record. Signed raw deltas are amount0 `-9701813616465735219516` and amount1
`444293424717033`; the reduced NVDA-per-SMOGGY3 ratio is
`148097808239011/3233937872155245073172`. The raw receipt and an independent
manifest are saved as
`.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/smoggy-v4-maximum-execution-receipt.response.json`
and `.scout-data/diagnostics/robinhood-stocktoken-prices-2026-10-11/smoggy-v4-maximum-execution-receipt.manifest.json`.
This one-call receipt check does not change the previously documented decoder
support or lifetime-coverage limits; no database or ATH record was changed.
