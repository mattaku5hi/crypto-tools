# Public token-page ATH check for three Solana mints — 2026-10-11

## Finding

The three exact-mint Solana Tracker pages are publicly readable without an account, but their returned HTML does not expose a numeric ATH value. This did not produce an independent ATH source for the current unresolved set. OpenDEX's direct ATH and token overview schemas do contain exact-string ATH fields, but the prior official-host unauthenticated probes returned HTTP 401; its own access guide describes plan-backed `odx_b2b_` keys. There is no evidence here that a free subscription exists or is enabled. No calls were repeated to OpenDEX.

| Mint | Public Solana Tracker page | HTTP | Page title from response | Numeric ATH in saved HTML |
|---|---|---:|---|---|
| `48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump` | [Elon Coin page](https://www.solanatracker.io/tokens/48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump) | 200 | Elon Coin (ELON) | Not found |
| `81kiaJSjaU9w6FVwRVT2K5TP1EhDghAXCm6Y4zmRpump` | [Sprite page](https://www.solanatracker.io/tokens/81kiaJSjaU9w6FVwRVT2K5TP1EhDghAXCm6Y4zmRpump) | 200 | Sprite (SPRITE) | Not found |
| `JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump` | [Pepsi page](https://www.solanatracker.io/tokens/JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump) | 200 | Pepsi (PEPSI) | Not found |

The downloaded pages have normal HTML page titles but no recognized numeric ATH fields in their returned source. This says only that a plain unauthenticated page GET did not yield a machine-readable ATH value; it does not prove the rendered interactive site never displays one after client-side requests. Those client internals were not probed.

## Provider boundary

Solana Tracker's official [ATH endpoint](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) documents `GET /tokens/{tokenAddress}/ath`, requires `x-api-key`, and defines the high relative to when its Data API started recording. Its official [pricing page](https://docs.solanatracker.io/pricing) lists a €0 Data API plan with 10,000 REST requests/month and 3 requests/second. The plan is a concrete candidate for an owner-provided key, but the docs do not explicitly establish this particular endpoint's Free-plan entitlement, each mint's recording start, or multi-pool/migration completeness. No API key was present in the current process environment and no key was created or requested.

OpenDEX's official [ATH reference](https://portal.opendex.ws/api-reference/token_ath) documents exact decimal price and market-cap fields and token-address resolution. Its official [Platform API access guide](https://portal.opendex.ws/docs/portal/api-access) says market-data calls use plan-backed `odx_b2b_` keys and that 401 means missing/invalid key while 402 means no active paid or free subscription. The existing [Solana/Robinhood source audit](2026-10-11-solana-robinhood-ath-sources.md) records two prior no-auth 401 responses on the documented production host. An owner key could make a test possible only if it has the required scope and an active subscription; the documents do not establish that such access is free. ATH `marketCap` semantics also remain unproven as equivalent to Codex's price-times-current-total-supply FDV.

## Provenance and limits

Exactly three no-auth GETs were made, one per official Solana Tracker token-page URL. Responses, status, titles, timestamps, byte lengths, and SHA-256 hashes are preserved under `.scout-data/diagnostics/solana-st-handle-pages-2026-10-11/`. The HTML was inspected as untrusted input; no scripts or token-provided content were executed. No API key, signup, paid request, Codex call, RPC call, or database write was used. No ATH value is promoted by this check.

## One rendered-page check

A separate, normal headless Chrome navigation opened only the public Solana Tracker page for `48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump` in a new temporary profile, with the browser sandbox enabled, no saved user profile/cookies, and all `SCOUT_*` environment variables removed. After a 10-second virtual-time budget, the rendered DOM still contained no ATH label or structured ATH price fields. This is stronger evidence than the earlier plain HTML fetch for this one page only; it does not cover the other two pages or prove the site lacks an ATH view behind later user interaction.

The rendered DOM is saved at `.scout-data/diagnostics/solanatracker-rendered-ath-2026-10-11/rendered-dom.html`, SHA-256 `c17184323ae9acf9fee5dfe14488951f7a28f1abab2843d997272ab5522b5ae6`. Run metadata is in the adjacent `manifest.json`. The separate plain-HTML response manifest remains unchanged. No page controls were clicked and no frontend API endpoints were inspected or called directly.
