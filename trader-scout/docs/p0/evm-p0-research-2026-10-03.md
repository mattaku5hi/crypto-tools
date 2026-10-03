# EVM P0 research: Base, BSC, Robinhood Chain (2026-10-03)

Status: research notes, not a deployment-registry entry. Per AGENTS.md invariants 3/4/5/16/19:
nothing here is "supported" until it has a pinned ABI source commit, an activation block and a golden
fixture (see `deployment-registry.md`). All dates below are verification date **2026-10-03** unless stated.
Labels: **[MEASURED]** = I ran a keyless request today; **[DOC]** = read in an official/vendor doc today;
**[3P]** = third-party/news, treat as unverified; **[UNVERIFIED]** = not confirmed.
Event topic0 values were computed locally with keccak256 (pycryptodome) from the canonical signature, not copied.

## 0. Headline findings

1. Chain ids are all correct: BSC 56, Base 8453, Robinhood Chain 4663. [MEASURED eth_chainId: 0x38 / 0x2105 / 0x1237].
2. Robinhood Chain is **live mainnet** (Arbitrum Orbit L2, ETH gas, ~0.1 s blocks), not testnet-only. Public mainnet launch 1 Jul 2026 [3P] (first block timestamp on chain is 2026-04-30 [MEASURED]).
3. Etherscan V2 free tier **does not cover BSC or Base** ("Paid Tier Only"). Robinhood is free only until 2026-10-15, then Lite ($49/mo+) from 2026-10-16 [DOC]. This confirms and sharpens the matrix note.
4. Blockscout has no BSC. Base and Robinhood are on Blockscout (explorer instances + PRO API with `chain_id`).
5. Public RPC `eth_getLogs`: Base public = max 2000-block range; BSC `bsc-dataseed.bnbchain.org` = "limit exceeded" even for 10 blocks; publicnode BSC/Base work for small windows (20000-result cap, archive needs token); Robinhood public RPC works with a 10000-log result cap and no tight range limit [all MEASURED, section 3].
6. Keyless 1-minute USD prices exist for both ETH and BNB (Coinbase has both ETH-USD and BNB-USD; Binance/OKX/Kraken/Bybit too) [MEASURED].
7. Most important design point: on EVM, pool `Swap` events are venue-specific and routers hide the trader. The cheapest venue-agnostic path that mirrors the Solana balance-delta approach is **ERC-20 `Transfer` logs + WETH Deposit/Withdrawal + native value/internal transfers per tx, netted per transaction `from`/beneficiary** (section 2.6), with venue decoders added only to classify/verify.

## 1. Chain identity

| | Base | BSC | Robinhood Chain |
|---|---|---|---|
| chain id | 8453 (0x2105) [MEASURED] | 56 (0x38) [MEASURED] | 4663 (0x1237) mainnet; testnet 46630 (0xb626) [MEASURED + DOC docs.robinhood.com/chain/connecting] |
| genesis hash (block 0) | `0xf712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd` [MEASURED mainnet.base.org] | `0x0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b` [MEASURED, identical on dataseed + publicnode] | `0xaad15f3d702aaea00caf3e9bb56395efe9127bc3b31b24921abf1eee3409305c` [MEASURED rpc.mainnet.chain.robinhood.com]. Block 0 timestamp is 0, block 1 timestamp 1777567931 (2026-04-30) |
| stack | OP Stack optimistic rollup, settles on Ethereum (Blockscout registry `chains.blockscout.com/api/chains/8453`) | L1 PoS (PoSA) | Arbitrum Orbit L2, ETH gas, blobs for DA ("an Arbitrum Layer-2 Chain built on Ethereum" docs.robinhood.com/chain/connecting; Blockscout registry `/api/chains/4663` says `rollupType: arbitrum`, `ecosystem: Arbitrum Orbit`) |
| native token | ETH | BNB | ETH |
| wrapped native | WETH `0x4200000000000000000000000000000000000006` (Uniswap Base deployments page) | WBNB `0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c` (Uniswap BNB deployments page) | WETH `0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73` (docs.robinhood.com/chain/protocol-contracts, matches Bags docs) |
| block time | ~2 s [MEASURED: 1000 blocks / 2000 s]; Blockscout `average_block_time` 2000 ms | ~0.45 s [MEASURED: 1000 blocks / 450 s]; Fermi hard fork 2026-01-14 cut 0.75s to 0.45s [3P: bnbchain.org blog, cointelegraph] | ~0.1 s [MEASURED: 1000 blocks / 102 s]. 79M blocks since 2026-04-30 |
| finality tags | `safe` and `finalized` return blocks [MEASURED] (OP L1-derived semantics; reorg risk before L1 inclusion) | `finalized` tag works on publicnode [MEASURED]; fast finality ~1.1 s [3P] | `safe` and `finalized` return blocks [MEASURED]; true finality = L1 confirmation of batch/assertion (Arbitrum semantics, [UNVERIFIED] numbers) |
| public RPC | `https://mainnet.base.org` (rate-limited, [MEASURED] works); `https://base-rpc.publicnode.com` | `https://bsc-dataseed.bnbchain.org`, `https://bsc-rpc.publicnode.com` | `https://rpc.mainnet.chain.robinhood.com` (docs say rate-limited, not for prod). Docs list Alchemy, Chainstack, QuickNode, Blockdaemon, dRPC, Validation Cloud, GlobalStake as providers |
| explorer | basescan.org (Etherscan), base.blockscout.com | bscscan.com | robinhoodchain.blockscout.com and robin.etherscan.io (Etherscan V2 chainlist) |

Fee-model quirks (for `fee_policy` in config):
- **Base**: fee = L2 execution (gasUsed x effectiveGasPrice) + **L1 data fee**, typically larger than L2 part. Receipt carries L1 fee fields (OP Stack: `l1Fee`, `l1GasUsed`, `l1GasPrice`, `l1FeeScalar` / blob scalars [UNVERIFIED which fields present, capture in fixture]). GasPriceOracle predeploy `0x420000000000000000000000000000000000000F`. Min base fee 0.005 gwei since Jovian; EIP-1559 elasticity 6, denominator 125 (docs.base.org/base-chain/network-information/network-fees, DOC). So `gasUsed * gasPrice` UNDERSTATES the user's cost; realized-PnL fee accounting must add the L1 fee from the receipt. Deposit (system) txs type 0x7e exist.
- **BSC**: plain gas model, gas paid in BNB; gas price floor is tiny. Fee in BNB, so USD conversion needs BNB-USD, not ETH. System/validator txs exist. [fee floor numbers UNVERIFIED]
- **Robinhood (Arbitrum Orbit)**: `gasUsed` in the receipt includes an L1-posting component (`gasUsedForL1` in Arbitrum receipts) priced via ArbOS L1 pricing; block headers include `l1BlockNumber` [MEASURED field present]. Fees paid in ETH. Arbitrum-style internal txs (retryable, 0x6a deposits) exist. Do not assume `effectiveGasPrice * gasUsed` equals paid fee without checking against balance deltas in a fixture. [UNVERIFIED details]

## 2. Where trading happens

Share data is third-party and volatile; use only for ordering. Base: Aerodrome (v2 + Slipstream) reported as the largest, roughly 50-63% of DEX volume, Uniswap v2/v3/v4 about 25-47% ([3P] wigwam.app, coinpaprika.com 2026-09-23 snapshot, eco.com). BSC: PancakeSwap ~95% of DEX share on 2026-09-23 [3P coinpaprika]; four.meme feeds it. Robinhood: memecoins >79% of DEX volume, Uniswap (v3/v4) + Pleiades AMM day-one venues, Pons launchpad dominant [3P: thedefiant.io, cryptoticker.io, altcoinbuzz]. Volume figures like "$1.49B/day" are 3P and not used for decisions.

### 2.1 Event signatures (computed, keccak256)

| Event | topic0 |
|---|---|
| ERC-20 `Transfer(address,address,uint256)` | `0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef` |
| WETH/WBNB `Deposit(address,uint256)` | `0xe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c` |
| WETH/WBNB `Withdrawal(address,uint256)` | `0x7fcf532c15f0a6db0bd6d0e038bea71d30d808c7d98cb3bf7268a95bf5081b65` |
| Uniswap V2 / Aerodrome V2 / Pancake V2 `Swap(address,uint256,uint256,uint256,uint256,address)` | `0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822` |
| V2 `Sync(uint112,uint112)` | `0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1` |
| V2 `PairCreated(address,address,address,uint256)` | `0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9` |
| Uniswap V3 / Slipstream `Swap(address,address,int256,int256,uint160,uint128,int24)` | `0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67` |
| Pancake V3 `Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)` (adds protocolFeesToken0/1; signature from pancake-v3-core, [UNVERIFIED against source commit]) | `0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83` |
| V3 `PoolCreated(address,address,uint24,int24,address)` (Uniswap/Pancake) | `0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118` |
| Slipstream `PoolCreated(address,address,int24,address)` | `0xab0d57f0df537bb25e80245ef7748fa62353808c54d6e528a9dd20887aed9ac2` |
| Uniswap V4 `Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)` (emitted by PoolManager) | `0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f` [MEASURED present in Robinhood PoolManager logs today] |
| V4 `Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)` | `0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438` |
| four.meme `TokenCreate(address,address,uint256,string,string,uint256,uint256,uint256)` | `0x396d5e902b675b032348d3d2e9517ee8f0c4a926603fbc075d3d282ff00cad20` |
| four.meme `TokenPurchase(address,address,uint256,uint256,uint256,uint256,uint256,uint256)` | `0x7db52723a3b2cdd6164364b3b766e65e540d7be48ffa89582956d8eaebe62942` (matches 3P-quoted hash) |
| four.meme `TokenSale(...same shape...)` | `0x0a5575b3648bae2210cee56bf33254cc1ddfbc7bf637c0af2ac18b14fb1bae19` |
| four.meme `LiquidityAdded(address,uint256,address,uint256)` | `0xc18aa71171b358b706fe3dd345299685ba21a5316c66ffa9e319268b033c44b0` |

Pancake Infinity (CL and Bin pool managers) swap event signatures: two candidates computed (`Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint16)` = `0x243635f4...`; Bin variant differs) but **not verified against infinity-core source**; do not use until read from `pancakeswap/infinity-core` at a pinned commit.

Trader semantics (cross-venue, applies to all):
- V2 `Swap(sender, amount0In, amount1In, amount0Out, amount1Out, to)`: `sender` = msg.sender of `swap()` (usually the router), `to` = recipient (may be a router or next pool in a multihop, or the user). Neither is reliably the trader.
- V3 `Swap(sender, recipient, amount0, amount1, ...)`: `sender` = caller (router), `recipient` = output receiver; amounts are signed from the pool's view (positive = pool received).
- V4 `Swap(id, sender, ...)`: `sender` is the router/locker that called PoolManager; PoolId is hashed; the user is not in the event at all. Settlement happens via Transfer logs on the tokens (and ETH native for ETH pairs, so native flows appear only in tx value/internal calls).
- Therefore the trader must be taken from the tx (`from`, or smart-wallet/AA owner via EntryPoint `UserOperationEvent.sender` [UNVERIFIED]) and amounts from net Transfer deltas of that address, not from pool events. Swaps routed through aggregator/Universal Router/bots will always show router as `sender`.

### 2.2 Base (ordered by expected relevance)

| Venue | Official addresses (source) | Notes |
|---|---|---|
| Aerodrome v2 (volatile/stable pools) | Router `0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43`, PoolFactory `0x420DD381b31aEf6683db6B902084cB0FFECe40Da`, Voter `0x16613524e02ad97eDfeF371bC883F2F5d6C480A5`, AERO `0x940181a94A35A4569E4529A3CDfB74e38FD98631` (github.com/aerodrome-finance/contracts README, DOC) | Pool is Velodrome-style V2 (Swap topic0 same as Uniswap V2). Fee-on-transfer handled by router "supportingFeeOnTransfer" variants [UNVERIFIED] |
| Aerodrome Slipstream (CL) | README lists several deployment generations; the initial PoolFactory `0x5e7BB104d84c7CB9B682AaC2F3d509f5F406809A`, SwapRouter `0xBE6D8f0d05cC4be24d5167a3eF062215bE6D18a5`, NFPM `0x827922686190790b37229fd06084350E74485b72`; later "Gauge Caps" deployment PoolFactory `0xaDe65c38CD4849aDBA595a4323a8C7DdfE89716a`, SwapRouter `0xcbBb8035cAc7D4B3Ca7aBb74cF7BdF900215Ce0D`; GitHub landing page also shows "Gauges V3" PoolFactory `0xf8f2eB4940CFE7d13603DDDD87f123820Fc061Ef`, SwapRouter `0x698Cb2b6dd822994581fEa6eA4Fc755d1363A92F` (github.com/aerodrome-finance/slipstream README, DOC) | **Multiple live CL factories**. Pool discovery must enumerate `PoolCreated` from every factory, not just one address. Swap topic0 identical to Uniswap V3. Which generations are active and activation blocks are [UNVERIFIED]; must read each factory's deployment tx |
| Uniswap v4 | PoolManager `0x498581ff718922c3f8e6a244956af099b2652b2b`, StateView `0xa3c0c9b65bad0b08107aa264b0f3db444b867a71`, PositionManager `0x7c5f5a4bbd8fd63184577525326123b519429bdc`, Universal Router `0x6ff5693b99212da76ad316178a184ab56d299b43` (plus 2.1.1 `0xfdf682f5...fbc7` and 2.1.2 `0xd6145b2D...9c40`), Permit2 `0x000000000022D473030F116dDEE9F6B43aC78BA3` (developers.uniswap.org/docs/protocols/v4/deployments, DOC) | All pools emit `Swap` from the single PoolManager; poolId hash, hook-dependent (Clanker, Zora and others use hooks). Pool-to-token map needs `Initialize` logs |
| Uniswap v3 | Factory `0x33128a8fC17869897dcE68Ed026d694621f6FDfD`, SwapRouter02 `0x2626664c2603336E57B271c5C0b26F421741e481`, UniversalRouter `0x6fF5693b99212Da76ad316178A184AB56D299b43`, QuoterV2 `0x3d4e44Eb1374240CE5F1B871ab261CD16335B76a` (developers.uniswap.org/docs/protocols/v3/deployments/v3-base-deployments; first column = Base mainnet, second = Base Sepolia, DOC) | |
| Uniswap v2 | [UNVERIFIED] not fetched (page returned empty); look up on the Uniswap v2 deployments page before use | |
| Clanker (launchpad, Uniswap v4 hook based) | Factory v4.0.0 `0xE85A59c628F7d27878ACeB4bf3b35733630083a9`, Hook Dynamic Fee v4.1.0 `0xd60D6B218116cFd801E28F78d011a203D2b068Cc`, Fee Locker `0xF3622742b1E446D92e45E22923Ef11C2fcD55D68`, Sniper Auction `0xFdc013ce003980889cFfd66b0c8329545ae1d1E8` (BaseScan labels via search + clanker docs "Deployed Contracts" page; [3P/UNVERIFIED], official docs page `clanker.gitbook.io/clanker-documentation/references/deployed-contracts` was not fetched) | Trades after launch are normal V4 pool swaps; factory only matters for token discovery |
| Zora coins, Virtuals, Flaunch, BaseSwap, PancakeSwap on Base | Not researched in depth. PancakeSwap on Base: V2 factory `0x02a84c1b3BBD7401a5f7fa98a384EBC70bB5749E`, V2 router `0x8cFe327CEc66d1C090Dd72bd0FF11d690C33a2Eb`, V3 factory `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865`, Infinity vault `0x238a358808379702088667322f80aC48bAd5e6c4`, Universal Router `0xd9C500DfF816a1Da21A48A732d3498Bf09dc9AEB` (developer.pancakeswap.finance addresses pages, DOC) | Treat Zora/Virtuals/Flaunch/BaseSwap as [UNVERIFIED] backlog |

Base aggregator/router note: OKX DEX routers on Base (`0x6b2C0c7be2048Daa9b5527982C29f48062B34D58`, `0x5e2F47bD7D4B357fCfd0Bb224Eb665773B1B9801`) are from a 3P search snippet only [UNVERIFIED]. 1inch, 0x/Matcha, KyberSwap, Odos, Uniswap UniversalRouter all exist on Base; none of their event shapes were verified here. They are why the Transfer-delta approach is preferred over decoding aggregator events.

### 2.3 BSC

| Venue | Official addresses (source) | Notes |
|---|---|---|
| PancakeSwap v2 | Factory `0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73`, Router `0x10ED43C718714eb63d5aA57B78B54704E256024E` (developer.pancakeswap.finance/contracts/v2/addresses, DOC; old docs.pancakeswap.finance page lists the same router "up-to-date") | Largest legacy venue, fee-on-transfer tokens common (many BSC memecoins) |
| PancakeSwap v3 | Factory `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865`, PoolDeployer `0x41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9`, SwapRouter `0x1b81D678ffb9C0263b24A97847620C99d213eB14`, SmartRouter `0x13f4EA83D0bd40E75C8222255bc855a974568Dd4` (developer.pancakeswap.finance/contracts/v3/addresses, DOC) | Pancake-specific V3 Swap event (extra protocol-fee fields) |
| PancakeSwap Infinity (v4) | Vault `0x238a358808379702088667322f80aC48bAd5e6c4`, CLPoolManager `0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b`, BinPoolManager `0xC697d2898e0D09264376196696c51D7aBbbAA4a9` (BNB and Base, search result of dev docs; full page `developer.pancakeswap.finance/contracts/infinity/resources/addresses` confirms Vault + Universal Router `0xd9C500DfF816a1Da21A48A732d3498Bf09dc9AEB` for BNB & Base; PoolManager addresses are from search snippet [partially UNVERIFIED]) | Swap event ABI not verified |
| four.meme (bonding-curve launchpad) | TokenManager2 (V2, tokens created after 2024-09-05) `0x5c952063c7fc8610FFDB798152D69F0B9550762b`; TokenManager V1 (before 2024-09-05, trading only) `0xEC4549caDcE5DA21Df6E6422d448034B5233bFbC`; Helper3 `0xF251F83e40a78868FcfA3FA4599Dad6494E46034` (github.com/four-meme-community/four-meme-ai skills references; points to official docs `four-meme.gitbook.io/four.meme/brand/protocol-integration` for ABIs, which I did not fetch; this repo is community-maintained so [partially UNVERIFIED]) | Events `TokenCreate`, `TokenPurchase(token, account, price, amount, cost, fee, offers, funds)`, `TokenSale(...)`, `LiquidityAdded(base, offers, quote, funds)` (topic0 in 2.1). `account` = the buyer/seller; **non-indexed fields**, so `eth_getLogs` can only filter by contract and topic0, not by token (token is in data). Must scan the whole TokenManager stream and filter client-side. X-Mode/TaxToken tokens exist. After graduation tokens trade on PancakeSwap (V2 per four.meme; [UNVERIFIED], also quote tokens other than BNB e.g. USD1, "curves quoted in tokenized stocks are priced as BNB" per DefiLlama issue #9736 [3P]) |
| Flap.sh (launchpad) | Portal `0xe2cE6ab80874Fa9Fa2aAE65D277Dd6B8e65C9De0` v5.23.1 (docs.flap.sh deployed contract addresses, DOC) | Event ABI not verified |
| Uniswap on BSC | v3 Factory `0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7`, SwapRouter02 `0xB971eF87ede563556b2ED4b1C0b0019111Dd85d2`, UniversalRouter `0x1906c1d672b88cd1b9ac7593301ca990f94eae07`; v4 PoolManager `0x28e2ea090877bf75740558f6bfb36a5ffee9e9df` (Uniswap docs, DOC) | Minor volume vs Pancake [3P] |
| Aggregators/other (1inch, OKX, Kyber, 0x, Binance Alpha/Wallet routers) | [UNVERIFIED] | Same router-hides-trader issue |

### 2.4 Robinhood Chain (verified live today: PoolManager emits V4 Swap logs [MEASURED])

| Venue | Official addresses (source) | Notes |
|---|---|---|
| Uniswap v4 | PoolManager `0x8366a39cc670b4001a1121b8f6a443a643e40951`, PositionManager `0x58daec3116aae6d93017baaea7749052e8a04fa7`, StateView `0xf3334192d15450cdd385c8b70e03f9a6bd9e673b`, Quoter `0x8dc178efb8111bb0973dd9d722ebeff267c98f94`, Universal Router `0x8876789976decbfcbbbe364623c63652db8c0904` (2.1.2: `0x204FAca1764B154221e35c0d20aBb3c525710498`), Permit2 canonical (developers.uniswap.org v4 deployments, DOC) | Main venue for graduated launchpad tokens (Bags, Pons V2 graduate into V4 behind hooks) |
| Uniswap v3 | Factory `0x1f7d7550b1b028f7571e69a784071f0205fd2efa`, SwapRouter02 `0xcaf681a66d020601342297493863e78c959e5cb2`, NFPM `0x73991a25c818bf1f1128deaab1492d45638de0d3`, QuoterV2 `0x33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7` (developers.uniswap.org v3 Robinhood deployments page, DOC) | Pons V1 launches one-sided V3 positions here |
| Pons (launchpad, dominant per 3P) | V2 (curve then Uniswap V4): Factory `0x7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e`, Meme Hook `0xE5e702641Ea86F4ae6cC3cDaeD2B886f976Be044`, Launch&Buy `0xe33E9E479dF8802cb0866d5d05258bEc4cF62948`, Locker `0x267444D099b10fB5Ed7c3Cc7B7c767AdcA574952`, Graduation Executor `0xC7819B64A1dAECD7eC19856d026cb14EfBd89046`. V1 (one-sided Uniswap V3 position): PonsLaunchFactory `0xA5aAb3F0c6EeadF30Ef1D3Eb997108E976351feB` (docs.ponsfamily.com/v2, github.com/ponsdotdev/pons-labs, docs.mobula.io; DOC/vendor). Mobula also lists a "Legacy Factory" `0x0c37a24F5D23A486FA692d1500881d698B1F77a4` [UNVERIFIED relation] | **Per-token curve contracts** created via CREATE2, so there is no single trading address; resolve via `TokenLaunched(token indexed, curve indexed, deployer indexed, pairToken, launchConfigId, graduationThreshold)`. Trade events on the curve: `CurveBuy(buyer indexed, recipient indexed, quoteIn, tokensOut, fee, tax)` and `CurveSell(seller indexed, recipient indexed, tokensIn, quoteOut, fee, tax)`; `buyer`/`seller` is indexed so the wallet is filterable by topic. Topic0 not computed (param types for `tax` ok; computable once ABI pinned). Graduation events: `CurveCompleted`, `LaunchSwept`, `PoolGraduated`, `PoolRegistered`. Thresholds 4.2 ETH for native-quoted [vendor docs] |
| Bags (launchpad) | BagsFactory `0xe8Cc4431adF8b5A847C113EF0c6af9043219Cb37`, BagsV4Hook `0x2380aBf72C17aABAb76480244759AC7E2932EEcC`, BagsLens `0xC82Db941dAf90B754aecb5F7D14c683dc608d595`, modified UniversalRouter `0x8876789976dEcBfCbBbe364623C63652db8C0904` (docs.bags.fm/robinhood/overview, vendor DOC) | Per-token `BagsBondingCurve`; events `TokenCreated`, `Migrated`; curve trade event names not captured [UNVERIFIED] |
| Pleiades AMM | Day-one partner with proprietary AMM [3P]; **no public address/ABI found** | Cannot be claimed as supported (invariant 16) |
| Uniswap Liquidity Launchpad | LiquidityLauncher `0x0000FffFBE8efE702c8703aE3477FF5dE3d319C0` (search snippet, developers.uniswap.org Liquidity Launchpad deployments; [UNVERIFIED]) | |
| PancakeSwap on Robinhood | V2 factory `0x02a84c1b...49E` and router `0x8cFe327C...a2Eb` (same as Base), V3 factory `0x0BFbCF9f...1865`, SmartRouter `0x13f4EA83...Dd4`, Infinity Vault `0x4F922d5B15e6691e0469663E4F5C4177f23c5FaF`, Universal Router `0x57fc55F719DF19B4b90A03F9D78E1177D002E504` (developer.pancakeswap.finance addresses pages, DOC) | Volume unknown |

Robinhood memecoin risk [3P]: reports of an $18.4M rug-pull ring across 53 launches (thecoinrepublic.com, cryptotimes.io 2026-09-28). Expect honeypots and mass launches (16-18k tokens/day reported in July [3P]).

### 2.5 What routers/aggregators emit
Not verified for any aggregator in this pass [UNVERIFIED]. Uniswap Universal Router and v2/v3 routers emit no swap-summary event of their own (only pool events + Transfers), which is exactly why pool `sender` is the router. Treat aggregator-specific events as optional enrichments; do not depend on them.

### 2.6 Recommended trade-extraction model (to validate in P1 with fixtures)
For each tx with a log involving the token of interest: collect all `Transfer` logs of the token, all quote-token Transfers (WETH/WBNB/USDC/USDT/USD1/stocks), `Deposit/Withdrawal` of the wrapped native, tx `value` and native internal transfers (needs traces or Etherscan `txlistinternal` / Blockscout internal-txs for ETH-paired V4 pools since V4 settles native without WETH logs). Net the flows per address; the wallet = tx `from` (EOA) or the address whose balances change in opposite directions across token and quote. Classify as buy/sell with a venue decoder when known, else "unclassified swap-like" and exclude from ranking (never silently drop, per quality gates). Known failure modes: fee-on-transfer (received != sent, and tax goes to a third address), rebasing/reflection tokens (Transfer amount != balance delta), router-forwarding (tokens end up at a second wallet), sandwich/MEV bots (high-frequency same-block in/out), multi-hop (one tx, several pools), tokens that mint/burn on transfer, bonding-curve phase (four.meme/Pons/Bags buys are not Transfer-from-pool: tokens come from the curve contract, quote is native ETH/BNB value so tx `value` is essential).

## 3. History sources

### 3.1 Etherscan V2 (single key, `chainid` param)
- Chainlist (`https://api.etherscan.io/v2/chainlist`, [MEASURED, 63 chains]) includes 56 (BscScan), 8453 (Base), 4663 ("Robinhood Chain", explorer `robin.etherscan.io`), all status 1.
- Tier availability (docs.etherscan.io/supported-chains, DOC): **BNB Smart Chain: Paid Tier Only. Base: Paid Tier Only. Robinhood: free until 2026-10-15, then Lite or above from 2026-10-16.** Source code/ABI endpoints are available on all chains for all plans.
- Limits (docs.etherscan.io/resources/rate-limits, DOC): Free 3 calls/s, 100,000/day, selected chains only; Lite 5/s, 100,000/day; Standard 10/s, 200,000/day; Advanced 20/s; Pro 30/s, 1M/day. PRO endpoints need Advanced+.
- `tokentx` (account module): needs `address` and/or `contractaddress`; `startblock/endblock/page/offset/sort`; max 10,000 records per request window and max 100 per page per the doc text I fetched [DOC; I did not verify with live key]. Use block-range windowing to walk past 10k. `contractaddress`-only gives token-centric history (all transfers of the token) but Transfer rows lack swap context (no receipts/logs), so a second call per tx hash (RPC `eth_getTransactionReceipt`) is needed.
- `logs/getLogs`: filter by `address` and `topic0..3` with block range, `page/offset`, 1000 results/page (my recollection; doc fetch did not state; [UNVERIFIED]); this is the closest to raw `eth_getLogs` with a pagination-friendly result and block timestamps in the response.
- `txlist`, `txlistinternal` (needed for ETH/BNB value legs and native-paired V4 settlement).
- Cost: for BSC and Base this requires the paid Lite plan ($49/mo per matrix note and pricing page; I did not re-fetch pricing today).

### 3.2 Blockscout
- Registry `chains.blockscout.com/api/chains/{id}` [MEASURED]: 8453 present (explorer base.blockscout.com), 4663 present (robinhoodchain.blockscout.com, `isTestnet:false`). 56 absent (per matrix, 2026-09-27).
- PRO API (docs.blockscout.com/rate-limits.md, DOC): Free 5 rps and 100K credits/day (~5,000 calls at 20 credits); Builder $49/mo 15 rps 100M credits/mo; Pro $199/mo 30 rps 500M/mo; Business $999 50 rps 3B/mo. Single key for 100+ chains via `chain_id`. Premium endpoints cost more (30-50 credits). Existing measurement in `2026-09-27-helius-blockscout.md` showed the matrix flagged an "Oct 1 Base cutoff" for free keys: **re-test Base on the owner's key now (today is after Oct 1)**; plan tier of `SCOUT_BLOCKSCOUT_API_KEY` is unknown to me.
- Public instance keyless: `base.blockscout.com/api/v2/stats` works keyless [MEASURED]; `robinhoodchain.blockscout.com` returned a Cloudflare challenge ("Just a moment...") to curl today [MEASURED], i.e. not usable keyless from scripts.
- Useful Blockscout REST v2 endpoints [UNVERIFIED in this pass; verify against docs before coding]: `/addresses/{addr}/token-transfers`, `/tokens/{addr}/transfers`, `/addresses/{addr}/transactions`, `/transactions/{hash}/logs`, `/addresses/{addr}/internal-transactions`, plus Etherscan-compatible `module=account/logs`. Keyset pagination (`next_page_params`), typically 50 items/page.

### 3.3 Raw RPC `eth_getLogs` (all [MEASURED] 2026-10-03, keyless)
| Endpoint | Result |
|---|---|
| `https://mainnet.base.org` | 10-block window OK; 1000-block window on USDC returned `-32020 backend response too large`; 10000-block window returned `-32614 eth_getLogs is limited to a 2,000 range` |
| `https://base-rpc.publicnode.com` | 10 blocks OK; 1000 blocks on USDC: `query exceeds max results 20000, retry with range ...` (helpful hint); 10000 blocks: `Archive requests require a personal token` |
| `https://bsc-dataseed.bnbchain.org` | `-32005 limit exceeded` for 10 and 1000 block windows (consistent with ADR-006) |
| `https://bsc-rpc.publicnode.com` | 10 blocks OK on WBNB; 1000: `query exceeds max results 20000` with retry range hint (so narrow topic filters work; WBNB is extremely busy) |
| `https://rpc.mainnet.chain.robinhood.com` | 10 and 1000 blocks OK on the Uniswap PoolManager; 100,000 blocks: `-32000 logs matched by query exceeds limit of 10000`. No tight range limit observed; the cap is on results |
- Alchemy (per Alchemy docs snippets, [3P/UNVERIFIED]): supports BNB and Robinhood; `eth_getLogs` free tier limited to **10 blocks** on BNB and Robinhood, unlimited range on PAYG; `alchemy_getAssetTransfers` reported working on free tier for Robinhood by a third-party research PR (github.com/1xmint/realorrug PR #76). Not measured by me (would need the key). Note the 10,000-result cap applies independently.
- QuoteNode/Ankr/dRPC/Chainstack free tiers for these chains: not measured.
- Mint-centric history on EVM has no equivalent of Solana `getTransactionsForAddress(mint)`. Token-centric options: (a) `eth_getLogs(address=token, topic0=Transfer)` windows (range/result-capped, then `eth_getTransactionReceipt`/`eth_getBlockReceipts` per tx hash), (b) Etherscan `tokentx&contractaddress=` or `logs&address=` (needs paid plan on BSC/Base), (c) Blockscout `/tokens/{addr}/transfers` + `/transactions/{hash}/logs`, (d) Codex `getTokenEvents` (below). `eth_getBlockReceipts` availability per provider not measured.
- Per-wallet history: no base RPC method. Use Etherscan `tokentx&address=` + `txlist` + `txlistinternal`, Blockscout `/addresses/{addr}/...`, `alchemy_getAssetTransfers`, or Codex `getTokenEventsForMaker`.
- Pool-centric alternative for launchpads: scan the single TokenManager2 (four.meme) or PoolManager (V4) event stream and filter client-side; Pons `CurveBuy/Sell` has indexed buyer/seller so wallet-filtered topic queries are possible per curve, but there are many curve addresses (use the factory-emitted list).

### 3.4 Codex (defined.fi), `SCOUT_CODEX_API_KEY`
- docs.codex.io/networks [DOC]: Base 8453, BNB 56, Robinhood 4663 all listed. Robinhood note: "Codex data on Robinhood Chain starts in mid-2026" (docs.codex.io/networks/robinhood).
- `getTokenEvents`: params `address` (token or pair, **top pair only** for a token; use `listPairsWithMetadataForToken` otherwise), filters `eventDisplayType` (Buy/Sell), `eventType`, `maker`, `timestamp`, `priceUsdTotal`, `networkId`; `limit` max 200, `cursor`, `direction`. Response includes `maker`, `transactionHash`, `blockNumber`, USD values, `priceUsd`, labels for sandwich/wash trade, `feeData`, `tradeSource` (docs.codex.io/api-reference/queries/gettokenevents, DOC).
- `getTokenEventsForMaker`: wallet-centric Swap/Mint/Burn history with `tokenAddress` filter, 200/page, cursor; "Token transfers aren't supported"; multi-hop swaps appear as separate events (correlate by tx hash) (docs.codex.io/api-reference/queries/gettokeneventsformaker, DOC).
- `getDetailedPairStats` for bucketed history. Codex plan limits/credits: not found in docs I fetched [UNVERIFIED]; must be measured with the owner key.
- Role under the AGENTS.md rules: a derived indexer; fine as a cross-check/discovery source, but ranking-critical PnL should come from decoded raw logs (provenance requirement) unless the owner explicitly accepts vendor-derived fills. Pair-coverage (top pair only) is a hidden-completeness risk.

## 4. USD / native price sources (keyless, 1-minute) [MEASURED 2026-10-03]
| Source | ETH-USD | BNB-USD | Notes |
|---|---|---|---|
| Coinbase Exchange `GET /products/{id}/candles?granularity=60&start&end` | OK (ETH-USD, returned 2026-09-30 candles, and 2025-10-03) | **OK (BNB-USD candles returned, sparse minutes: only 4 of 5 minutes had rows)** | Existing Solana path already uses this; max 300 candles/request; missing minutes when no trades. BNB-USD is thin, expect gaps; need a fallback policy |
| Binance `GET /api/v3/klines?symbol=BNBUSDT&interval=1m&startTime&limit` (also `data-api.binance.vision`) | ETHUSDT | BNBUSDT; history back to at least 2025-10-03 returned | USDT not USD; up to 1000 candles/call; geo-restrictions possible on api.binance.com (worked here); `data-api.binance.vision` is the public market-data mirror |
| OKX `/api/v5/market/history-candles` | yes | BNB-USDT returned | limit 100/call, `after` pagination |
| Kraken OHLC | ETHUSD | BNBUSD returned 1-min bars | Kraken returns only the latest ~720 bars for 1-minute intervals (documented behavior; not reverified), so no deep history |
| Bybit v5 kline | yes | BNBUSDT returned | |
Recommendation: ETH via Coinbase ETH-USD (already integrated). BNB via Coinbase BNB-USD primary with Binance BNBUSDT (USDT~USD assumption, record it as such) as gap filler, flag minutes filled from fallback. Tokens priced in stables (USDC/USDT/USD1) need their own depeg handling. Token-level USD should never come from these; only the native/quote leg is converted, consistent with the Solana "quote units + USD" approach.

## 5. Phased plan

Order: **1) Base, 2) Robinhood Chain, 3) BSC.**
- Base first: matches ARCHITECTURE.md's first slice, Blockscout covers it (key already owned), public RPC logs work for small windows, fee model understood, venues (Aerodrome + Uniswap) are standard V2/V3/V4 ABIs with official verified addresses.
- Robinhood second: fresh chain (history only since 2026-04-30, ~79M blocks), Etherscan V2 is free until 2026-10-15 (use that window for fixture capture and backfill if a key exists), few venues (Uniswap v4/v3, Pons, Bags) all with documented addresses; but Pons/Bags need per-token curve resolution.
- BSC last: PancakeSwap v2/v3 + four.meme; hardest history path (no Blockscout, Etherscan paid-only, public RPC logs blocked on dataseed, Alchemy free 10-block limit), fee-on-transfer-heavy token population, BNB-USD price gaps.

Minimal venue sets (cover most volume per 3P data):
- Base: Aerodrome v2 + Slipstream (all factories) + Uniswap v4 PoolManager + Uniswap v3. Then Clanker (discovery only, trades are V4).
- Robinhood: Uniswap v4 PoolManager + v3 + Pons (V1 via V3, V2 curve) + Bags curve; skip Pleiades until an ABI exists.
- BSC: PancakeSwap v2 + v3 + four.meme TokenManager2 (+V1 manager for old tokens) + Infinity later.
Common step first: generic Transfer/WETH/native net-flow engine (section 2.6), then venue classifiers.

History source per chain (to prove by measurement in P1, not assumed):
- Base: Blockscout PRO (`chain_id=8453`) for per-wallet and per-token transfer lists + `eth_getBlockReceipts`/receipt fetches via a paid-or-keyed RPC; public RPC only for fixtures. Re-test whether the owner key still serves Base after Oct 1.
- Robinhood: Etherscan V2 (free to Oct 15) and/or Blockscout PRO; raw RPC works well (10k-result cap).
- BSC: requires owner decision: Etherscan Lite+ or a paid/keyed RPC with usable `eth_getLogs` (Alchemy PAYG, QuickNode, Chainstack, dRPC, Ankr [none measured]); public dataseed unusable, publicnode usable only for small windows.

Fixtures to capture (invariant 19 style: real, with provenance, per deployment):
1. One Base Aerodrome v2 swap direct, one via router, one multihop; one Slipstream swap per factory generation; one Uniswap v4 swap via Universal Router (ETH pair, native settlement) and one Clanker-token swap; one Uniswap v3 swap.
2. One Robinhood V4 swap via UR, one V3 swap, one Pons V1 swap, one Pons V2 `CurveBuy`, `CurveSell`, graduation tx; one Bags curve buy and graduation.
3. One BSC Pancake v2 swap (including a fee-on-transfer token), one v3 swap, four.meme `TokenPurchase`/`TokenSale`/`LiquidityAdded`, and a post-graduation Pancake trade of a four.meme token.
4. For each chain: a tx receipt showing the fee fields (Base L1 fee, Robinhood gasUsedForL1) and a balance-delta cross-check; a tx paid via a smart-wallet/AA; a MEV sandwich tx pair; a rebasing/reflection token transfer.
5. Registry rows (activation block of every factory/manager, pinned ABI commit) before any "supported" claim.

Risks:
- Router hides trader; V4 hides pair in a hash and settles natively; many CL factories on Aerodrome; four.meme events have non-indexed token (full stream scan); per-token curve contracts (Pons, Bags); fee-on-transfer and reflection tokens (BSC especially); MEV bots polluting rankings; honeypots/rug rings on fresh chains; Robinhood data is only ~5 months old and moving fast (contracts upgrade, V1/V2 factories coexist); BNB-USD minute gaps; Etherscan/Blockscout policy changes (free tiers already shifting: Oct 1 Base notice, Oct 15 Robinhood cutoff); finality semantics differ per chain (L2 soft vs L1 finalized; BSC fast finality) while config says `finality = "finalized"`.
- Vendor claims in this doc not independently measured: Alchemy free limits, Codex limits, aggregator events, share-of-volume numbers.

Credentials/decisions the owner must provide:
- RPC URLs: `SCOUT_BASE_RPC_URL`, `SCOUT_BSC_RPC_URL`, `SCOUT_ROBINHOOD_RPC_URL`; at minimum a keyed provider with working `eth_getLogs`/`eth_getBlockReceipts` for BSC.
- A decision on Etherscan V2 key + Lite plan (needed for BSC and, after 2026-10-15, Robinhood and Base) versus Blockscout PRO (BSC unavailable). Confirm the current plan tier of `SCOUT_BLOCKSCOUT_API_KEY` and `SCOUT_CODEX_API_KEY`.
- Config fix: `[providers.evm_history]` uses one `SCOUT_EVM_HISTORY_API_KEY`; reality needs per-chain provider selection (Blockscout for Base/Robinhood, Etherscan for BSC).

## 6. Sources (all fetched 2026-10-03 unless noted)
- RPC calls [MEASURED]: mainnet.base.org, base-rpc.publicnode.com, bsc-dataseed.bnbchain.org, bsc-rpc.publicnode.com, rpc.mainnet.chain.robinhood.com, rpc.testnet.chain.robinhood.com
- https://docs.robinhood.com/chain/connecting ; https://docs.robinhood.com/chain/protocol-contracts/
- https://chains.blockscout.com/api/chains/4663 ; /8453 ; https://api.etherscan.io/v2/chainlist ; https://base.blockscout.com/api/v2/stats
- https://docs.etherscan.io/supported-chains ; https://docs.etherscan.io/resources/rate-limits ; https://docs.etherscan.io/api-reference/endpoint/tokentx ; /getlogs
- https://docs.blockscout.com/rate-limits.md ; https://docs.blockscout.com/devs/apis
- https://developers.uniswap.org/docs/protocols/v4/deployments ; .../v3/deployments/v3-base-deployments ; .../v3-bnb-deployments ; .../v3-robinhood-chain-deployments
- https://developer.pancakeswap.finance/contracts/v2/addresses ; /v3/addresses ; /infinity/resources/addresses ; /universal-router/addresses
- https://github.com/aerodrome-finance/contracts ; https://github.com/aerodrome-finance/slipstream
- https://github.com/four-meme-community/four-meme-ai (skills/four-meme-integration/references/contract-addresses.md, event-listening.md)
- https://docs.flap.sh deployed contract addresses page
- https://docs.ponsfamily.com/v2 ; https://github.com/ponsdotdev/pons-labs ; https://docs.mobula.io/almanac/robinhood-launchpads/pons ; https://docs.bags.fm/robinhood/overview
- https://docs.base.org/base-chain/network-information/network-fees
- https://docs.codex.io/networks ; /networks/robinhood ; /api-reference/queries/gettokenevents ; /gettokeneventsformaker
- 3P: coinpaprika.com/education/dex-market-share-by-chain, thedefiant.io (Pons/Robinhood volume), bnbchain.org blog (Fermi), Alchemy docs snippets (eth_getLogs 10-block free tier), github.com/1xmint/realorrug PR #76, thecoinrepublic.com (rug ring)
- Price APIs [MEASURED]: api.exchange.coinbase.com, api.binance.com, data-api.binance.vision, okx.com, api.kraken.com, api.bybit.com
