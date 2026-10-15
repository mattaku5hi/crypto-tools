# Base pool-scope completion: `0xa0ad…4b07`

As of 2026-10-11, a bounded GeckoTerminal public API v2 check fetched the one omitted pool history from the original hash-verified page-1 listing for Base token `0xa0ad74a3444b7f59c3ca48669057b844d0214b07`. The original frozen 53-token manifest was not changed.

The added pool is `0xbdac4729c348eb5e5141c3adb90804f1df3d1a9b8dbbd99a36cc7d1e5c367e32` (`Trump / USDC`); returned token identities place the target in the base side and USDC (`0x833589fcd6edb6e08f4c7c32d4f71b54bda02913`) in the quote side. Its two returned daily candles are 2026-04-26 and 2026-04-27. The 2026-04-27 OHLCV high is `$0.000004891868208119`.

Combining this with the two previously saved pool responses for the same page-1 listing gives a highest known-pool high of `$0.000004891868208119` on the Birdeye reference peak day, 2026-04-27. This is `-0.02794452213099977465092616370%` from the cached Birdeye high `$0.000004893235599424003`, within the existing 1% same-day and known-pool comparison bound. The result closes the omitted-third-listed-pool gap for this sample. It does not establish lifetime ATH or complete pool discovery: later listing pages, historical pools absent from the current listing, and migration coverage were not established.

The one added request was unauthenticated `GET https://api.geckoterminal.com/api/v2/networks/base/pools/{pool}/ohlcv/day?aggregate=1&limit=1000&before_timestamp=1791676799&currency=usd&token=base&include_empty_intervals=false`. The scope was the closed yearly interval 2025-10-10 through 2026-10-10 UTC. It returned HTTP 200, 2 in-scope rows, 605 bytes; response SHA-256 `67f7a8f6a90383b05b9088fc1af6999c86c1bd9d0d27345916c24cebd761e025`.

Private evidence and exact-precision three-pool composition: `.scout-data/diagnostics/geckoterminal-base-a0ad-pool-scope-2026-10-11/composed-three-pools.json`. It records raw-response hashes, pool identities, candle rows, frozen-manifest hash, and comparison inputs.
