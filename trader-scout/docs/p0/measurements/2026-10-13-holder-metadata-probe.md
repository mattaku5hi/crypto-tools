# Holder metadata probe for ATH unknowns (2026-10-13)

## Result

Two bounded, unauthenticated requests to the official Blockscout instances returned HTTP 403. The public endpoint could not be used to recover current holder counts for either sampled unknown. This establishes an access failure for these exact requests and time, not that the endpoint is universally private or that the tokens have no holders. No response bodies were retained, so body size, response schema, and any explanatory error text are unknown.

| Network | Frozen unknown token sampled | Request | Result |
|---|---|---|---|
| Base mainnet | `0x00001f97a5538fee8ec5f8797e499c934f02fc74` | `https://base.blockscout.com/api/v2/tokens/{address}` | HTTP 403 |
| Robinhood Chain mainnet (4663) | `0x0d15b27faa1a26bfe6cd9f97e5c3de251c0fe960` | `https://robinhoodchain.blockscout.com/api/v2/tokens/{address}` | HTTP 403 |

The sample addresses came from the frozen NULL-holder portion of `remaining-249.json`; this was a one-token-per-network feasibility check, not a cohort estimate. Robinhood's official network documentation identifies mainnet as chain ID 4663 and `robinhoodchain.blockscout.com` as its explorer, distinguishing testnet chain ID 46630 and its separate explorer ([Robinhood Chain: Connecting](https://docs.robinhood.com/chain/connecting/)).

Blockscout's official token API documentation describes token metadata (`getToken`) and paginated holder lists (`getTokenHolders`, with `page` and `offset`) on its token module API ([Blockscout Token API](https://docs.blockscout.com/devs/apis/rpc/token)). That documentation does not prove anonymous access on either deployment, nor does it establish that the tested v2 route is available without authentication. The 403 results leave that question unresolved.

## Interpretation and next step

Current holder count is a separate eligibility input; even a successful count would not establish holders at the historical price peak, an ATH, historical supply, or complete price coverage. Do not convert NULL to zero or use these probes to change the holder gate.

If holder metadata is still needed, first confirm instance-specific access/limits with the explorer operator or official deployment documentation. Then a bounded retry can test the documented token API route on these same exact addresses. Do not rotate credentials or infer that an API key is needed from a 403 alone.

## Evidence record

- Request manifest: [holder-metadata-probe-2026-10-13/manifest.json](../../../.scout-data/diagnostics/holder-metadata-probe-2026-10-13/manifest.json)
- Manifest SHA-256: `91ad446a46fe1ac88e2143cbd8a3d1e3e30a13e5014f3081d7d7e0a96bd8eab0`
- Frozen input SHA-256: `cb24e70420d4f5afc60a6b10d34c11eb487df4cd3d32c30f1a38fdc78209a1ea`
- Requests: 2 total; no authentication, no retries; 2 MiB response cap and 20-second timeout configured; both response bodies not retained.
- No database writes or application changes were made.
