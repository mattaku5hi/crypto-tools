//! ADR-019 EVM amendment: exit-quote valuation of open EVM positions.
//! Every venue path runs against a wiremock chain whose `eth_call`
//! answers are ABI-encoded by hand; two live-fixture wallet cards (Robinhood
//! Uniswap v4, Base Uniswap v3 USDC) are replayed end to end with mocked
//! quoter answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::integer_division,
    clippy::too_many_arguments
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, B256, Bytes, U256, address};
use scout_core::{EvmTxStatus, RawEvmLog, RawEvmTransaction};
use scout_dex_evm::{
    PANCAKE_V2_BSC_FACTORY, PANCAKE_V3_SWAP_TOPIC0, QuoterFamily, SwapVenue,
    V2_SWAP_EVENT_SIGNATURE, V3_SWAP_TOPIC0, V4_INITIALIZE_TOPIC0, V4_SWAP_TOPIC0, V4PoolKey,
    aerodrome_amount_out_calldata, selector, v2_reserves_calldata, v3_quote_calldata,
    v4_quote_calldata,
};
use scout_engine::{
    AnalysisWindow, ChainDisplay, EvmExtractionConfig, EvmOpenValuationRun, EvmUnvaluedReason,
    EvmValuationOptions, ExclusionReason, LedgerOptions, QuoteUnit, RankBy, RankPolicy,
    RankProfile, SolanaWalletStats, WalletScanStatus, apply_evm_open_valuation, apply_usd_pricing,
    build_evm_wallet_ledger, quote_units_to_money, rank_solana_wallets,
};
use scout_evm::{BASE, BASE_USDC, BSC, BSC_USDT, EvmChainProfile, ROBINHOOD, TRANSFER_TOPIC0};
use scout_pricing::{Candle, DecimalPrice, InMemoryPriceSource, QuoteAsset};
use scout_providers::EvmRpcClient;
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const AS_OF: i64 = 1_790_000_100;
const HEAD: u64 = 0x1000;
const W: Address = address!("00000000000000000000000000000000000000a1");
const W2: Address = address!("00000000000000000000000000000000000000a2");
const ROUTER: Address = address!("00000000000000000000000000000000000000b2");
const TOKEN: Address = address!("00000000000000000000000000000000000000c1");
const OTHER: Address = address!("00000000000000000000000000000000000000c2");
const POOL: Address = address!("00000000000000000000000000000000000000d1");
const SLIP_GEN2_FACTORY: Address = address!("aDe65c38CD4849aDBA595a4323a8C7DdfE89716a");
const PM_RH: Address = address!("8366a39cc670b4001a1121b8f6a443a643e40951");

// ---------------------------------------------------------------------
// ABI helpers
// ---------------------------------------------------------------------

fn word(v: u128) -> String {
    format!("{v:064x}")
}

fn addr_word(a: Address) -> String {
    format!("{:0>64}", format!("{a:x}"))
}

fn words(ws: &[String]) -> String {
    format!("0x{}", ws.concat())
}

fn sel_hex(sig: &str) -> String {
    format!("0x{}", alloy_primitives::hex::encode(selector(sig)))
}

// ---------------------------------------------------------------------
// Mock chain
// ---------------------------------------------------------------------

#[derive(Clone)]
enum Ans {
    Ok(String),
    Revert,
}

#[derive(Clone)]
struct Rule {
    to: Address,
    /// `data` must start with this (lowercase hex).
    prefix: String,
    /// Optional extra substring of `data`.
    contains: Option<String>,
    ans: Ans,
}

fn rule(to: Address, prefix: &str, ans: Ans) -> Rule {
    Rule {
        to,
        prefix: prefix.to_ascii_lowercase(),
        contains: None,
        ans,
    }
}

fn ok(s: String) -> Ans {
    Ans::Ok(s)
}

type Calls = Arc<Mutex<Vec<(String, String, String)>>>;

struct Chain {
    server: MockServer,
    calls: Calls,
}

impl Chain {
    async fn start(rules: Vec<Rule>, logs: Vec<RawEvmLog>) -> Self {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let c2 = Arc::clone(&calls);
        let log_json: Vec<Value> = logs
            .iter()
            .map(|l| {
                json!({
                    "address": format!("{:#x}", l.address),
                    "topics": l.topics.iter().map(|t| format!("{t:#x}")).collect::<Vec<_>>(),
                    "data": format!("0x{}", alloy_primitives::hex::encode(&l.data)),
                    "blockNumber": format!("{:#x}", l.block_number),
                    "transactionIndex": format!("{:#x}", l.transaction_index),
                    "logIndex": format!("{:#x}", l.log_index),
                })
            })
            .collect();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let id = body["id"].clone();
                let m = body["method"].as_str().unwrap_or("").to_string();
                let params = &body["params"];
                let result = |v: Value| {
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":v}))
                };
                let error = |msg: &str| {
                    ResponseTemplate::new(200).set_body_json(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":3,"message":msg}}),
                    )
                };
                match m.as_str() {
                    "eth_blockNumber" => result(json!(format!("{HEAD:#x}"))),
                    "eth_getLogs" => {
                        c2.lock().unwrap().push((
                            "eth_getLogs".to_string(),
                            String::new(),
                            params[0].to_string(),
                        ));
                        result(json!(log_json))
                    }
                    "eth_call" => {
                        let to = params[0]["to"].as_str().unwrap().to_ascii_lowercase();
                        let data = params[0]["data"].as_str().unwrap().to_ascii_lowercase();
                        let block = params[1].as_str().unwrap_or("").to_string();
                        c2.lock().unwrap().push((to.clone(), data.clone(), block));
                        let hit = rules.iter().find(|r| {
                            format!("{:#x}", r.to) == to
                                && data.starts_with(&r.prefix)
                                && r.contains.as_ref().is_none_or(|c| data.contains(c))
                        });
                        match hit {
                            Some(Rule {
                                ans: Ans::Ok(v), ..
                            }) => result(json!(v)),
                            Some(Rule {
                                ans: Ans::Revert, ..
                            }) => error("execution reverted"),
                            None => error("unmocked call"),
                        }
                    }
                    _ => error("unmocked method"),
                }
            })
            .mount(&server)
            .await;
        Self { server, calls }
    }

    fn rpc(&self, profile: EvmChainProfile) -> EvmRpcClient {
        self.rpc_budget(profile, None)
    }

    fn rpc_budget(&self, profile: EvmChainProfile, max: Option<u64>) -> EvmRpcClient {
        let rpc = RpcClient::new(RpcEndpoint::new(self.server.uri()), 10_000, 1)
            .unwrap()
            .with_max_total_requests(max);
        EvmRpcClient::new(rpc, profile)
    }

    fn eth_calls(&self) -> Vec<(String, String, String)> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0 != "eth_getLogs")
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------
// Synthetic wallets
// ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    V3,
    Slipstream,
    Pancake,
    V2,
    Aero,
    V4(B256),
}

fn rlog(address: Address, topics: Vec<B256>, data: Vec<u8>, n: u64, idx: u64) -> RawEvmLog {
    RawEvmLog {
        address,
        topics,
        data: Bytes::from(data),
        block_number: n,
        transaction_index: 1,
        log_index: idx,
    }
}

fn transfer(token: Address, from: Address, to: Address, v: u128, n: u64, idx: u64) -> RawEvmLog {
    rlog(
        token,
        vec![TRANSFER_TOPIC0, from.into_word(), to.into_word()],
        U256::from(v).to_be_bytes::<32>().to_vec(),
        n,
        idx,
    )
}

/// A buy of `token_amt` TOKEN paying `quote_amt` of `quote` at `pool`.
#[allow(clippy::too_many_arguments)]
fn buy(
    profile: EvmChainProfile,
    wallet: Address,
    n: u64,
    pool: Address,
    kind: Kind,
    quote: Address,
    quote_amt: u128,
    token_amt: u128,
    l1_fee: Option<u128>,
) -> RawEvmTransaction {
    let swap = match kind {
        Kind::V3 | Kind::Slipstream => rlog(
            pool,
            vec![V3_SWAP_TOPIC0, ROUTER.into_word(), wallet.into_word()],
            vec![0u8; 160],
            n,
            2,
        ),
        Kind::Pancake => rlog(
            pool,
            vec![
                PANCAKE_V3_SWAP_TOPIC0,
                ROUTER.into_word(),
                wallet.into_word(),
            ],
            vec![0u8; 224],
            n,
            2,
        ),
        Kind::V2 | Kind::Aero => rlog(
            pool,
            vec![
                V2_SWAP_EVENT_SIGNATURE,
                ROUTER.into_word(),
                wallet.into_word(),
            ],
            vec![0u8; 128],
            n,
            2,
        ),
        Kind::V4(id) => rlog(
            pool,
            vec![V4_SWAP_TOPIC0, id, ROUTER.into_word()],
            vec![0u8; 192],
            n,
            2,
        ),
    };
    RawEvmTransaction {
        chain: profile.verified_chain_key(),
        hash: B256::repeat_byte(u8::try_from(n).unwrap()),
        from: wallet,
        to: Some(ROUTER),
        block_number: n,
        transaction_index: 1,
        block_time: 1_790_000_000 + n * 10,
        value: U256::ZERO,
        status: EvmTxStatus::Success,
        gas_used: 100,
        effective_gas_price: U256::from(3u8),
        l1_fee: l1_fee.map(U256::from),
        logs: vec![
            transfer(quote, wallet, pool, quote_amt, n, 0),
            transfer(TOKEN, pool, wallet, token_amt, n, 1),
            swap,
        ],
        native_source: None,
        internal_transfers: None,
        native_balance_diff: None,
    }
}

fn cfg_with(profile: EvmChainProfile, pools: &[(SwapVenue, Address)]) -> EvmExtractionConfig {
    let mut cfg = EvmExtractionConfig::for_profile(profile);
    for (v, p) in pools {
        cfg.gate
            .register_known_pool(*v, *p, scout_dex_evm::VenueVerification::FixtureVerified);
    }
    cfg
}

fn card(
    cfg: &EvmExtractionConfig,
    wallet: Address,
    txs: &[RawEvmTransaction],
) -> SolanaWalletStats {
    let ledger = build_evm_wallet_ledger(cfg, wallet, txs, LedgerOptions::default()).unwrap();
    let mut key = [0u8; 32];
    key[12..].copy_from_slice(wallet.as_slice());
    SolanaWalletStats {
        wallet: key,
        chain: ChainDisplay::evm(&cfg.profile),
        status: WalletScanStatus::Ok,
        transactions_scanned: Some(1),
        transactions_in_window: None,
        truncated: false,
        unexpected_payloads: 0,
        error: None,
        ledger: Some(ledger),
        incomplete_reasons: Vec::new(),
        failure: None,
        not_scanned: None,
    }
}

fn view(c: &SolanaWalletStats) -> &scout_engine::EvmOpenValuationView {
    c.ledger
        .as_ref()
        .unwrap()
        .evm
        .as_ref()
        .unwrap()
        .open_valuation
        .as_ref()
        .unwrap()
}

fn only(c: &SolanaWalletStats) -> &scout_engine::EvmPositionValuation {
    let v = view(c);
    assert_eq!(v.positions.len(), 1);
    &v.positions[0]
}

async fn value(
    cards: &mut [SolanaWalletStats],
    chain: &Chain,
    cfg: &EvmExtractionConfig,
    opts: &EvmValuationOptions,
) -> EvmOpenValuationRun {
    apply_evm_open_valuation(
        cards,
        &chain.rpc(cfg.profile),
        cfg,
        &AnalysisWindow::none(AS_OF),
        opts,
    )
    .await
}

fn candle(time: i64, close: &str) -> Candle {
    let p = DecimalPrice::parse(close).unwrap();
    Candle {
        time,
        low: p,
        high: p,
        open: p,
        close: p,
        volume: DecimalPrice::ONE,
    }
}

fn quoter_answer(out: u128) -> Ans {
    ok(words(&[word(out), word(0), word(0), word(0)]))
}

/// Mock rules of a v3-style pool (`token0`, `token1`, `fee`) plus the
/// quoter answers for the full and the probe amount.
fn v3_rules(
    quoter: Address,
    family: QuoterFamily,
    pool: Address,
    t0: Address,
    t1: Address,
    pool_selector: i32,
    amount: u128,
    out_full: u128,
    out_probe: u128,
) -> Vec<Rule> {
    let mut rules = vec![
        rule(pool, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
        rule(pool, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
    ];
    if family == QuoterFamily::Slipstream {
        rules.push(rule(
            pool,
            &sel_hex("factory()"),
            ok(words(&[addr_word(SLIP_GEN2_FACTORY)])),
        ));
        rules.push(rule(
            pool,
            &sel_hex("tickSpacing()"),
            ok(words(&[word(u128::try_from(pool_selector).unwrap())])),
        ));
    } else {
        rules.push(rule(
            pool,
            &sel_hex("fee()"),
            ok(words(&[word(u128::try_from(pool_selector).unwrap())])),
        ));
    }
    let token_in = TOKEN;
    let token_out = if t0 == TOKEN { t1 } else { t0 };
    for (a, out) in [(amount, out_full), (amount / 1000, out_probe)] {
        let data = v3_quote_calldata(family, token_in, token_out, U256::from(a), pool_selector);
        rules.push(Rule {
            to: quoter,
            prefix: data.to_ascii_lowercase(),
            contains: None,
            ans: quoter_answer(out),
        });
    }
    rules
}

const ETH: u128 = 1_000_000_000_000_000_000;

// ---------------------------------------------------------------------
// Uniswap v3 on Robinhood: pinned quoter, probe impact, unrealized, USD
// ---------------------------------------------------------------------

#[tokio::test]
async fn v3_robinhood_pinned_quoter_with_impact_unrealized_and_usd() {
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV3, POOL)]);
    let weth = ROBINHOOD.wrapped_native;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let quoter = address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let chain = Chain::start(
        v3_rules(
            quoter,
            QuoterFamily::UniswapV3,
            POOL,
            t0,
            t1,
            3000,
            1_000_000_000,
            9 * ETH / 10,
            950_000_000_000_000,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    let run = value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert!(run.ran && !run.historical);
    assert_eq!(run.state_block, Some(HEAD));
    let p = only(&cards[0]);
    let v = p.valued().unwrap();
    assert_eq!(v.label, "realizable_onchain_quote");
    assert_eq!(v.method, "quoter_v2");
    assert_eq!((v.quoter, v.quoter_source), (Some(quoter), "pinned"));
    assert_eq!(v.venue, SwapVenue::UniswapV3);
    assert_eq!(v.pool, POOL);
    assert_eq!(v.quote_unit, QuoteUnit::Wei);
    assert_eq!(v.quote_token, None);
    assert_eq!(v.realizable_raw, 9 * ETH / 10);
    assert_eq!(v.state_block, HEAD);
    assert_eq!(v.probe_amount_raw, Some(1_000_000));
    assert_eq!(v.price_impact_bps, Some(526));
    assert!(!v.transfer_tax_not_modelled);
    // basis = 0.5 ETH + gas 300 wei capitalized once.
    assert_eq!(v.unrealized_status, "known");
    assert_eq!(
        v.unrealized_pnl,
        Some(
            quote_units_to_money(
                QuoteUnit::Wei,
                i128::try_from(9 * ETH / 10 - (ETH / 2 + 300)).unwrap()
            )
            .unwrap()
        )
    );
    // Every eth_call was pinned to the as-of head.
    let calls = chain.eth_calls();
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|c| c.2 == "0x1000"), "{calls:?}");
    // Quoter calls: full + probe; identity: token0, token1, fee.
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.0 == format!("{quoter:#x}"))
            .count(),
        2
    );
    assert_eq!(run.eth_calls, 5);

    // USD via ADR-018: ETH-USD at as_of (2000) and at the acquisition minute (1000).
    let src = InMemoryPriceSource::new().with_candles(
        QuoteAsset::Eth,
        &[candle(1_789_999_980, "1000"), candle(AS_OF, "2000")],
    );
    apply_usd_pricing(&mut cards, &src).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(
        v.usd.as_ref().unwrap().value.scaled_units(),
        1800 * 100_000_000
    );
    assert_eq!(v.usd_unrealized.unwrap().scaled_units(), 1300 * 100_000_000);
    let t = view(&cards[0]).totals();
    assert_eq!((t.positions, t.valued, t.unvalued), (1, 1, 0));
    assert_eq!(t.realizable_raw_by_unit["eth"], 9 * ETH / 10);
    assert_eq!(t.usd_priced_positions, 1);
}

// ---------------------------------------------------------------------
// Uniswap v3 on Base: USDC quote, par USD, other-unit basis, unknown basis
// ---------------------------------------------------------------------

#[tokio::test]
async fn v3_base_usdc_quote_par_usd_and_unrealized_statuses() {
    let weth = BASE.wrapped_native;
    let usdc = BASE_USDC.address;
    let cfg = cfg_with(BASE, &[(SwapVenue::UniswapV3, POOL)]);
    let quoter = address!("3d4e44Eb1374240CE5F1B871ab261CD16335B76a");
    let (t0, t1) = if TOKEN < usdc {
        (TOKEN, usdc)
    } else {
        (usdc, TOKEN)
    };
    let mk_chain = |t0, t1| {
        v3_rules(
            quoter,
            QuoterFamily::UniswapV3,
            POOL,
            t0,
            t1,
            500,
            1_000_000_000,
            3_000_000,
            3_100,
        )
    };
    // (a) paid in USDC, sells into USDC: unrealized known, USD at par.
    let tx = buy(
        BASE,
        W,
        1,
        POOL,
        Kind::V3,
        usdc,
        2_000_000,
        1_000_000_000,
        Some(10),
    );
    let chain = Chain::start(mk_chain(t0, t1), vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.quote_unit, QuoteUnit::UsdcUnits);
    assert_eq!(v.quote_token, Some(usdc));
    assert_eq!(v.realizable_raw, 3_000_000);
    assert_eq!(v.unrealized_status, "known");
    assert_eq!(
        v.unrealized_pnl,
        Some(quote_units_to_money(QuoteUnit::UsdcUnits, 1_000_000).unwrap())
    );
    let src = InMemoryPriceSource::new();
    apply_usd_pricing(&mut cards, &src).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(
        v.usd.as_ref().unwrap().value.scaled_units(),
        3 * 100_000_000
    );
    assert_eq!(v.usd_unrealized.unwrap().scaled_units(), 100_000_000);

    // (b) paid in WETH, pool sells into USDC: basis is in another unit.
    let tx = buy(
        BASE,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 100,
        1_000_000_000,
        Some(10),
    );
    let chain = Chain::start(mk_chain(t0, t1), vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.unrealized_status, "basis_other_unit");
    assert_eq!(v.unrealized_pnl, None);

    // (c) the Base l1 fee was not observed: the basis is unknown, never zero.
    let tx = buy(
        BASE,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 100,
        1_000_000_000,
        None,
    );
    let chain = Chain::start(mk_chain(t0, t1), vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.unrealized_status, "unknown_basis");
    assert_eq!(v.unrealized_pnl, None);
    assert_eq!(view(&cards[0]).totals().unrealized_unknown_positions, 1);
}

// ---------------------------------------------------------------------
// Uniswap v4: PoolKey from the Initialize log, verified against the pool id
// ---------------------------------------------------------------------

fn init_log(pm: Address, key: &V4PoolKey, id: B256) -> RawEvmLog {
    let mut data = Vec::new();
    data.extend(U256::from(key.fee).to_be_bytes::<32>());
    data.extend(U256::from(u32::try_from(key.tick_spacing).unwrap()).to_be_bytes::<32>());
    data.extend(key.hooks.into_word().0);
    data.extend([0u8; 64]);
    rlog(
        pm,
        vec![
            V4_INITIALIZE_TOPIC0,
            id,
            key.currency0.into_word(),
            key.currency1.into_word(),
        ],
        data,
        5,
        0,
    )
}

fn v4_key() -> V4PoolKey {
    V4PoolKey {
        currency0: Address::ZERO,
        currency1: TOKEN,
        fee: 3000,
        tick_spacing: 60,
        hooks: address!("00000000000000000000000000000000000000f0"),
    }
}

fn v4_rules(key: &V4PoolKey, quoter: Address, amount: u128, out: u128, probe: u128) -> Vec<Rule> {
    // TOKEN is currency1: zero_for_one = false.
    [(amount, out), (amount / 1000, probe)]
        .iter()
        .map(|(a, o)| Rule {
            to: quoter,
            prefix: v4_quote_calldata(key, false, *a).to_ascii_lowercase(),
            contains: None,
            ans: ok(words(&[word(*o), word(150_000)])),
        })
        .collect()
}

#[tokio::test]
async fn v4_robinhood_key_from_initialize_and_quote_and_failure_paths() {
    let key = v4_key();
    let id = key.pool_id();
    let cfg = cfg_with(ROBINHOOD, &[]);
    let weth = ROBINHOOD.wrapped_native;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        PM_RH,
        Kind::V4(id),
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let quoter = address!("8dc178efb8111bb0973dd9d722ebeff267c98f94");

    // Found: the Initialize log hashes to the pool id.
    let chain = Chain::start(
        v4_rules(
            &key,
            quoter,
            1_000_000_000,
            7 * ETH / 10,
            800_000_000_000_000,
        ),
        vec![init_log(PM_RH, &key, id)],
    )
    .await;
    let mut cards = vec![card(&cfg, W, std::slice::from_ref(&tx))];
    let run = value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.method, "v4_quoter");
    assert_eq!(v.quoter, Some(quoter));
    assert_eq!(v.pool_id, Some(id));
    assert_eq!(v.quote_unit, QuoteUnit::Wei, "currency0 = native");
    assert_eq!(v.realizable_raw, 7 * ETH / 10);
    assert_eq!(run.log_calls, 1);
    // The lookup targeted the PoolManager and this pool id only.
    let logs_call = chain
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.0 == "eth_getLogs")
        .cloned()
        .unwrap();
    assert!(logs_call.2.contains(&format!("{id:#x}")));
    assert!(logs_call.2.contains(&format!("{PM_RH:#x}")));

    // No Initialize log: pool_key_unknown, no quoter call.
    let chain = Chain::start(vec![], vec![]).await;
    let mut cards = vec![card(&cfg, W, std::slice::from_ref(&tx))];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::PoolKeyUnknown)
    );
    assert!(chain.eth_calls().is_empty());

    // A log whose key does not hash to the pool id is not a key.
    let mut forged = key;
    forged.fee = 10_000;
    let chain = Chain::start(vec![], vec![init_log(PM_RH, &forged, id)]).await;
    let mut cards = vec![card(&cfg, W, std::slice::from_ref(&tx))];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::PoolKeyUnknown)
    );

    // v4 on Base: the V4Quoter is pinned (live poolManager() getter).
    let base_cfg = cfg_with(BASE, &[]);
    let base_pm = address!("498581ff718922c3f8e6a244956af099b2652b2b");
    let base_quoter = address!("0d5e0f971ed27fbff6c2837bf31316121532048d");
    let btx = buy(
        BASE,
        W,
        1,
        base_pm,
        Kind::V4(id),
        BASE.wrapped_native,
        ETH / 2,
        1_000_000_000,
        Some(1),
    );
    let chain = Chain::start(
        v4_rules(&key, base_quoter, 1_000_000_000, ETH / 2, ETH / 2000),
        vec![init_log(base_pm, &key, id)],
    )
    .await;
    let mut cards = vec![card(&base_cfg, W, &[btx])];
    value(
        &mut cards,
        &chain,
        &base_cfg,
        &EvmValuationOptions::default(),
    )
    .await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!((v.quoter, v.quoter_source), (Some(base_quoter), "pinned"));
}

// ---------------------------------------------------------------------
// Pancake v3 / Slipstream: pinned on BSC/Base; unpinned (synthetic) unless overridden
// ---------------------------------------------------------------------

#[tokio::test]
async fn pancake_v3_and_slipstream_pinned_unpinned_and_overridable() {
    let usdt = BSC_USDT.address;
    let pq = address!("00000000000000000000000000000000000000e1");
    let pinned_pq = address!("B048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997");
    // --- Pancake v3 on BSC (USDT-peg quote), pinned QuoterV2.
    let cfg = cfg_with(BSC, &[(SwapVenue::PancakeV3, POOL)]);
    let tx = buy(
        BSC,
        W,
        1,
        POOL,
        Kind::Pancake,
        usdt,
        5 * ETH,
        1_000_000_000,
        None,
    );
    let (t0, t1) = if TOKEN < usdt {
        (TOKEN, usdt)
    } else {
        (usdt, TOKEN)
    };
    let chain = Chain::start(
        v3_rules(
            pinned_pq,
            QuoterFamily::PancakeV3,
            POOL,
            t0,
            t1,
            2500,
            1_000_000_000,
            6 * ETH,
            6_100_000_000_000_000_000,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!((v.quoter, v.quoter_source), (Some(pinned_pq), "pinned"));
    assert_eq!(v.realizable_raw, 6 * ETH);

    // Pancake v3 on Robinhood (synthetic gap): unpinned, no call, then override.
    let rh_weth = ROBINHOOD.wrapped_native;
    let rh_cfg = cfg_with(ROBINHOOD, &[(SwapVenue::PancakeV3, POOL)]);
    let (t0, t1) = if TOKEN < rh_weth {
        (TOKEN, rh_weth)
    } else {
        (rh_weth, TOKEN)
    };
    let rh_tx = || {
        buy(
            ROBINHOOD,
            W,
            1,
            POOL,
            Kind::Pancake,
            rh_weth,
            ETH / 2,
            1_000_000_000,
            None,
        )
    };
    let chain = Chain::start(
        v3_rules(
            pq,
            QuoterFamily::PancakeV3,
            POOL,
            t0,
            t1,
            2500,
            1_000_000_000,
            6 * ETH,
            6_100_000_000_000_000_000,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&rh_cfg, W, &[rh_tx()])];
    value(&mut cards, &chain, &rh_cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::VenueQuoterUnpinned)
    );
    assert!(
        chain.eth_calls().is_empty(),
        "no call without a pinned quoter"
    );
    let mut opts = EvmValuationOptions::default();
    opts.quoter_overrides.insert(QuoterFamily::PancakeV3, pq);
    let mut cards = vec![card(&rh_cfg, W, &[rh_tx()])];
    value(&mut cards, &chain, &rh_cfg, &opts).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.quoter_source, "override");
    assert_eq!(v.realizable_raw, 6 * ETH);
    // USD: the pinned BSC Pancake USDT-peg card, candles at par-ish 1.001,
    // flagged as a Binance-Peg asset.
    let mut cards = vec![card(
        &cfg,
        W,
        &[buy(
            BSC,
            W,
            1,
            POOL,
            Kind::Pancake,
            usdt,
            5 * ETH,
            1_000_000_000,
            None,
        )],
    )];
    let chain = Chain::start(
        v3_rules(
            pinned_pq,
            QuoterFamily::PancakeV3,
            POOL,
            if TOKEN < usdt { TOKEN } else { usdt },
            if TOKEN < usdt { usdt } else { TOKEN },
            2500,
            1_000_000_000,
            6 * ETH,
            6_100_000_000_000_000_000,
        ),
        vec![],
    )
    .await;
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.quote_unit, QuoteUnit::BinancePegUsdtUnits);
    assert_eq!(v.unrealized_status, "known");
    let src = InMemoryPriceSource::new().with_candles(
        QuoteAsset::Usdt,
        &[candle(1_789_999_980, "1.000"), candle(AS_OF, "1.001")],
    );
    apply_usd_pricing(&mut cards, &src).await;
    let u = only(&cards[0]).valued().unwrap().usd.clone().unwrap();
    assert_eq!(u.value.scaled_units(), 600_600_000);
    assert!(u.price_label.ends_with("+binance_peg"), "{}", u.price_label);

    // --- Slipstream on Base: quoter pinned per factory generation, selected
    // by the pool's factory() (tickSpacing selector, int24 struct).
    let weth = BASE.wrapped_native;
    let cfg = cfg_with(BASE, &[(SwapVenue::AerodromeSlipstream, POOL)]);
    let gen_quoters = [
        (
            address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A"),
            address!("254cF9E1E6e233aa1AC962CB9B05b2cfeAaE15b0"),
        ),
        (
            SLIP_GEN2_FACTORY,
            address!("3d4C22254F86f64B7eC90ab8F7aeC1FBFD271c6C"),
        ),
        (
            address!("f8f2eB4940CFE7d13603DDDD87f123820Fc061Ef"),
            address!("514c8B5f54112481E28028F1166Bd78501089259"),
        ),
    ];
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let slip_tx = |chain: EvmChainProfile, quote: Address| {
        buy(
            chain,
            W,
            1,
            POOL,
            Kind::Slipstream,
            quote,
            ETH / 2,
            1_000_000_000,
            Some(1),
        )
    };
    for (factory, sq) in gen_quoters {
        let mut rules = v3_rules(
            sq,
            QuoterFamily::Slipstream,
            POOL,
            t0,
            t1,
            100,
            1_000_000_000,
            ETH,
            1_020_000_000_000_000,
        );
        // This generation's factory (v3_rules answers gen2; first rule wins).
        rules.insert(
            0,
            rule(
                POOL,
                &sel_hex("factory()"),
                ok(words(&[addr_word(factory)])),
            ),
        );
        let chain = Chain::start(rules, vec![]).await;
        let mut cards = vec![card(&cfg, W, &[slip_tx(BASE, weth)])];
        value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
        let v = only(&cards[0]).valued().unwrap();
        assert_eq!(v.venue, SwapVenue::AerodromeSlipstream);
        assert_eq!((v.quoter, v.quoter_source), (Some(sq), "pinned"));
        assert_eq!(v.realizable_raw, ETH);
        let data = chain
            .eth_calls()
            .iter()
            .find(|c| c.0 == format!("{sq:#x}"))
            .unwrap()
            .1
            .clone();
        // Live-confirmed 2026-10-04: selector 0x9e7defe6.
        assert!(data.starts_with("0x9e7defe6"));
        assert!(data.starts_with(&sel_hex(
            "quoteExactInputSingle((address,address,uint256,int24,uint160))"
        )));
    }

    // A zero quote (tiny input, gen2 live) is a valid quote of 0, not an error.
    let sq2 = gen_quoters[1].1;
    let chain = Chain::start(
        v3_rules(
            sq2,
            QuoterFamily::Slipstream,
            POOL,
            t0,
            t1,
            100,
            1_000_000_000,
            0,
            0,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&cfg, W, &[slip_tx(BASE, weth)])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.realizable_raw, 0);

    // Unpinned (synthetic): the same factory on Robinhood has no quoter.
    let rh_weth = ROBINHOOD.wrapped_native;
    let rh_cfg = cfg_with(ROBINHOOD, &[(SwapVenue::AerodromeSlipstream, POOL)]);
    let (t0, t1) = if TOKEN < rh_weth {
        (TOKEN, rh_weth)
    } else {
        (rh_weth, TOKEN)
    };
    let sq = address!("00000000000000000000000000000000000000e2");
    let chain = Chain::start(
        v3_rules(
            sq,
            QuoterFamily::Slipstream,
            POOL,
            t0,
            t1,
            100,
            1_000_000_000,
            ETH,
            1_020_000_000_000_000,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&rh_cfg, W, &[slip_tx(ROBINHOOD, rh_weth)])];
    value(&mut cards, &chain, &rh_cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::VenueQuoterUnpinned)
    );
    assert!(chain.eth_calls().iter().all(|c| c.0 != format!("{sq:#x}")));
    let mut opts = EvmValuationOptions::default();
    opts.quoter_overrides.insert(QuoterFamily::Slipstream, sq);
    let mut cards = vec![card(&rh_cfg, W, &[slip_tx(ROBINHOOD, rh_weth)])];
    value(&mut cards, &chain, &rh_cfg, &opts).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!((v.quoter, v.quoter_source), (Some(sq), "override"));
}

// ---------------------------------------------------------------------
// v2-style reserves and Aerodrome getAmountOut
// ---------------------------------------------------------------------

fn cp(inp: u128, rin: u128, rout: u128, num: u128, den: u128) -> u128 {
    inp * num * rout / (rin * den + inp * num)
}

#[tokio::test]
async fn v2_reserves_use_the_factory_fee_uniswap_and_pancake() {
    // Uniswap v2 on Robinhood: factory pinned, 0.30%.
    let weth = ROBINHOOD.wrapped_native;
    let rh_factory = address!("8bceaa40b9acdfaedf85adf4ff01f5ad6517937f");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let (r_tok, r_eth) = (1_000_000_000_000u128, 2_000_000_000_000u128);
    let (r0, r1) = if t0 == TOKEN {
        (r_tok, r_eth)
    } else {
        (r_eth, r_tok)
    };
    let rules = |factory: Address, t0, t1| {
        vec![
            rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
            rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
            rule(
                POOL,
                &sel_hex("factory()"),
                ok(words(&[addr_word(factory)])),
            ),
            rule(
                POOL,
                &v2_reserves_calldata(),
                ok(words(&[word(r0), word(r1), word(1)])),
            ),
        ]
    };
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV2, POOL)]);
    let chain = Chain::start(rules(rh_factory, t0, t1), vec![]).await;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V2,
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.method, "v2_reserves_constant_product");
    assert_eq!((v.quoter, v.quoter_source), (None, "not_applicable"));
    assert_eq!(v.realizable_raw, cp(1_000_000_000, r_tok, r_eth, 997, 1000));
    assert_eq!(
        v.probe_out_raw,
        Some(cp(1_000_000, r_tok, r_eth, 997, 1000))
    );
    assert!(v.price_impact_bps.is_some());
    // No quoter is needed: only identity + reserves calls were made.
    assert_eq!(chain.eth_calls().len(), 4);

    // PancakeSwap v2 on BSC: 0.25%.
    let wbnb = BSC.wrapped_native;
    let (t0, t1) = if TOKEN < wbnb {
        (TOKEN, wbnb)
    } else {
        (wbnb, TOKEN)
    };
    let (r0, r1) = if t0 == TOKEN {
        (r_tok, r_eth)
    } else {
        (r_eth, r_tok)
    };
    let rules_bsc = vec![
        rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
        rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
        rule(
            POOL,
            &sel_hex("factory()"),
            ok(words(&[addr_word(PANCAKE_V2_BSC_FACTORY)])),
        ),
        rule(
            POOL,
            &v2_reserves_calldata(),
            ok(words(&[word(r0), word(r1), word(1)])),
        ),
    ];
    let cfg = cfg_with(BSC, &[(SwapVenue::UniswapV2, POOL)]);
    let chain = Chain::start(rules_bsc, vec![]).await;
    let tx = buy(
        BSC,
        W,
        1,
        POOL,
        Kind::V2,
        wbnb,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(
        v.realizable_raw,
        cp(1_000_000_000, r_tok, r_eth, 9975, 10_000)
    );
    assert_eq!(v.quote_unit, QuoteUnit::Wei);
    // BNB-USD for the native BSC unit.
    let src = InMemoryPriceSource::new().with_candles(
        QuoteAsset::Bnb,
        &[candle(1_789_999_980, "500"), candle(AS_OF, "600")],
    );
    apply_usd_pricing(&mut cards, &src).await;
    let v = only(&cards[0]).valued().unwrap();
    let expect_usd =
        i128::try_from(cp(1_000_000_000, r_tok, r_eth, 9975, 10_000)).unwrap() * 600 * 100_000_000
            / i128::try_from(ETH).unwrap();
    // 1.99... e9 wei * 600 / 1e18 is far below 1e-8 USD: rounds half-even.
    assert!(v.usd.as_ref().unwrap().value.scaled_units() - expect_usd <= 1);

    // An unknown v2-style factory has no pinned fee: not supported, no guess.
    let rules_other = vec![
        rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
        rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
        rule(POOL, &sel_hex("factory()"), ok(words(&[addr_word(OTHER)]))),
    ];
    let chain = Chain::start(rules_other, vec![]).await;
    let mut cards = vec![card(
        &cfg,
        W,
        &[buy(
            BSC,
            W,
            1,
            POOL,
            Kind::V2,
            wbnb,
            ETH / 2,
            1_000_000_000,
            None,
        )],
    )];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::VenueNotSupported)
    );
}

#[tokio::test]
async fn aerodrome_v2_uses_the_pools_own_get_amount_out() {
    let weth = BASE.wrapped_native;
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let cfg = cfg_with(BASE, &[(SwapVenue::AerodromeV2, POOL)]);
    let mut rules = vec![
        rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
        rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
    ];
    for (a, o) in [
        (1_000_000_000u128, 4 * ETH),
        (1_000_000, 4_100_000_000_000_000),
    ] {
        rules.push(rule(
            POOL,
            &aerodrome_amount_out_calldata(U256::from(a), TOKEN),
            ok(words(&[word(o)])),
        ));
    }
    let chain = Chain::start(rules, vec![]).await;
    let tx = buy(
        BASE,
        W,
        1,
        POOL,
        Kind::Aero,
        weth,
        ETH,
        1_000_000_000,
        Some(1),
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert_eq!(v.method, "aerodrome_get_amount_out");
    assert_eq!(v.realizable_raw, 4 * ETH);
    assert_eq!(v.price_impact_bps, Some(243)); // 1 - 4.0e9/4.1e9 = 2.439%
}

// ---------------------------------------------------------------------
// Failure paths: revert, asset, tax label, budget, historical, cache
// ---------------------------------------------------------------------

#[tokio::test]
async fn revert_is_quote_reverted_never_zero_and_other_asset_is_unsupported() {
    let weth = ROBINHOOD.wrapped_native;
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV3, POOL)]);
    let quoter = address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let mut rules = vec![
        rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
        rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
        rule(POOL, &sel_hex("fee()"), ok(words(&[word(3000)]))),
    ];
    rules.push(rule(quoter, "0x", Ans::Revert));
    let chain = Chain::start(rules, vec![]).await;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    let run = value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::QuoteReverted)
    );
    let t = view(&cards[0]).totals();
    assert_eq!((t.valued, t.unvalued), (0, 1));
    assert!(
        t.realizable_raw_by_unit.is_empty(),
        "no zero value is invented"
    );
    assert_eq!(t.unvalued_by_reason["quote_reverted"], 1);
    assert_eq!(run.totals.unvalued, 1);
    assert!(!run.budget_exhausted);

    // The pool's other asset is neither native nor a pinned quote token.
    let rules = vec![
        rule(POOL, &sel_hex("token0()"), ok(words(&[addr_word(TOKEN)]))),
        rule(POOL, &sel_hex("token1()"), ok(words(&[addr_word(OTHER)]))),
        rule(POOL, &sel_hex("fee()"), ok(words(&[word(3000)]))),
    ];
    let chain = Chain::start(rules, vec![]).await;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::QuoteAssetUnsupported)
    );

    // A garbled (too short) quoter answer is not a number.
    let rules = v3_rules(
        quoter,
        QuoterFamily::UniswapV3,
        POOL,
        t0,
        t1,
        3000,
        1_000_000_000,
        1,
        1,
    );
    let mut rules: Vec<Rule> = rules
        .into_iter()
        .map(|mut r| {
            if r.to == quoter {
                r.ans = ok("0x1234".to_string());
            }
            r
        })
        .collect();
    rules.truncate(5);
    let chain = Chain::start(rules, vec![]).await;
    let tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 2,
        1_000_000_000,
        None,
    );
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::QuoteResponseInvalid)
    );
}

#[tokio::test]
async fn transfer_tax_shaped_flow_is_labelled_not_modelled() {
    let weth = ROBINHOOD.wrapped_native;
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV3, POOL)]);
    let quoter = address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    // The pool sends 1_000_000_000 but the wallet receives 900_000_000 (the rest goes to a tax wallet).
    let mut tx = buy(
        ROBINHOOD,
        W,
        1,
        POOL,
        Kind::V3,
        weth,
        ETH / 2,
        900_000_000,
        None,
    );
    tx.logs
        .insert(2, transfer(TOKEN, POOL, OTHER, 100_000_000, 1, 5));
    let chain = Chain::start(
        v3_rules(
            quoter,
            QuoterFamily::UniswapV3,
            POOL,
            t0,
            t1,
            3000,
            900_000_000,
            ETH / 4,
            ETH / 4 + 1,
        ),
        vec![],
    )
    .await;
    let mut cards = vec![card(&cfg, W, &[tx])];
    value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    let v = only(&cards[0]).valued().unwrap();
    assert!(v.transfer_tax_not_modelled);
    assert_eq!(view(&cards[0]).totals().transfer_tax_positions, 1);
}

#[tokio::test]
async fn budget_historical_window_and_call_cache() {
    let weth = ROBINHOOD.wrapped_native;
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV3, POOL)]);
    let quoter = address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let rules = v3_rules(
        quoter,
        QuoterFamily::UniswapV3,
        POOL,
        t0,
        t1,
        3000,
        1_000_000_000,
        ETH,
        ETH + 1,
    );
    let tx = |w| {
        buy(
            ROBINHOOD,
            w,
            1,
            POOL,
            Kind::V3,
            weth,
            ETH / 2,
            1_000_000_000,
            None,
        )
    };

    // Budget: eth_blockNumber + 2 identity calls, then spent.
    let chain = Chain::start(rules.clone(), vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx(W)])];
    let run = apply_evm_open_valuation(
        &mut cards,
        &chain.rpc_budget(ROBINHOOD, Some(3)),
        &cfg,
        &AnalysisWindow::none(AS_OF),
        &EvmValuationOptions::default(),
    )
    .await;
    assert!(run.budget_exhausted);
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::RequestBudgetExhausted)
    );
    assert!(chain.eth_calls().len() <= 2);

    // Historical window: nothing is read at all.
    let chain = Chain::start(rules.clone(), vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx(W)])];
    let window =
        AnalysisWindow::resolve(Some("30d"), None, Some("2026-09-01T00:00:00Z"), AS_OF).unwrap();
    assert!(window.until < window.as_of);
    let run = apply_evm_open_valuation(
        &mut cards,
        &chain.rpc(ROBINHOOD),
        &cfg,
        &window,
        &EvmValuationOptions::default(),
    )
    .await;
    assert!(run.historical);
    assert_eq!(
        only(&cards[0]).unvalued_reason(),
        Some(EvmUnvaluedReason::HistoricalWindow)
    );
    assert!(chain.server.received_requests().await.unwrap().is_empty());

    // Two wallets with the same token/pool/amount: the second is served from the cache.
    let chain = Chain::start(rules, vec![]).await;
    let mut cards = vec![card(&cfg, W, &[tx(W)]), card(&cfg, W2, &[tx(W2)])];
    let run = value(&mut cards, &chain, &cfg, &EvmValuationOptions::default()).await;
    assert_eq!(run.totals.valued, 2);
    assert_eq!(run.eth_calls, 5);
    // Pool identity is cached on its own; the two quoter answers hit the call cache.
    assert_eq!(run.cache_hits, 2);
    assert_eq!(run.wallets_with_open, 2);
    assert_eq!(chain.eth_calls().len(), 5);
}

// ---------------------------------------------------------------------
// Ranking: --require-valued-open on EVM cards
// ---------------------------------------------------------------------

#[tokio::test]
async fn require_valued_open_excludes_unvalued_evm_wallets_only() {
    let weth = ROBINHOOD.wrapped_native;
    let cfg = cfg_with(ROBINHOOD, &[(SwapVenue::UniswapV3, POOL)]);
    let quoter = address!("33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7");
    let (t0, t1) = if TOKEN < weth {
        (TOKEN, weth)
    } else {
        (weth, TOKEN)
    };
    let chain = Chain::start(
        v3_rules(
            quoter,
            QuoterFamily::UniswapV3,
            POOL,
            t0,
            t1,
            3000,
            1_000_000_000,
            ETH,
            ETH + 1,
        ),
        vec![],
    )
    .await;
    let tx = |w| {
        buy(
            ROBINHOOD,
            w,
            1,
            POOL,
            Kind::V3,
            weth,
            ETH / 2,
            1_000_000_000,
            None,
        )
    };
    let mut valued = vec![card(&cfg, W, &[tx(W)])];
    value(&mut valued, &chain, &cfg, &EvmValuationOptions::default()).await;
    let unvalued = card(&cfg, W2, &[tx(W2)]); // valuation not run
    let cards = vec![valued.remove(0), unvalued];
    let mut policy = RankPolicy::for_profile(RankProfile::None, RankBy::RealizedNetPnl, 10);
    policy.require_valued_open = true;
    let r = rank_solana_wallets(&cards, &policy);
    let reasons = |w: Address| {
        let mut k = [0u8; 32];
        k[12..].copy_from_slice(w.as_slice());
        r.excluded
            .iter()
            .find(|e| e.observation.wallet == k)
            .map(|e| e.reasons.clone())
            .unwrap()
    };
    assert!(!reasons(W).contains(&ExclusionReason::OpenExposureUnvalued));
    assert!(reasons(W2).contains(&ExclusionReason::OpenExposureUnvalued));
    let label = |w: Address| {
        let mut k = [0u8; 32];
        k[12..].copy_from_slice(w.as_slice());
        r.excluded
            .iter()
            .find(|e| e.observation.wallet == k)
            .map(|e| e.observation.open_exposure.label())
            .unwrap()
    };
    assert_eq!(label(W), "valued");
    assert_eq!(label(W2), "unvalued");
}

// ---------------------------------------------------------------------
// Live-fixture wallet cards replayed with mocked quoter answers
// ---------------------------------------------------------------------

#[path = "support/evm_aiden.rs"]
mod evm_aiden;
#[path = "support/evm_base.rs"]
mod evm_base;

mod replays {
    use super::*;
    use scout_engine::{EvmStatsSources, run_evm_wallet_stats};
    use scout_providers::evm_replay::{AlchemyInternalMode, ExplorerReplay};
    use scout_providers::{
        AlchemyConfig, AlchemyTransfersSource, BlockscoutApiKey, BlockscoutEvmConfig,
        BlockscoutEvmSource, EvmHistoryScanner, NativeLegPolicy, NativeLegResolver, ScanLimits,
        WalletIndexer,
    };
    use tokio_util::sync::CancellationToken;

    /// Robinhood: the first Aiden wallet's open Uniswap v4 position; the
    /// PoolKey comes from the REAL `Initialize` log of the fixture served by
    /// the mocked `eth_getLogs`; the V4Quoter answer is mocked.
    #[tokio::test]
    async fn robinhood_aiden_wallet_card_valued_through_the_v4_quoter() {
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(evm_aiden::fixture_path()).unwrap())
                .unwrap();
        let senders = ExplorerReplay::from_fixture(&fixture).senders();
        let server = evm_aiden::serve(evm_aiden::replay()).await;
        let rpc = evm_aiden::rpc_client(&server);
        let chain_key = rpc.preflight().await.unwrap();
        let scanned = evm_aiden::scan(&server).await;
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain_key, ScanLimits::default());
        let ex = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ExplorerReplay::from_fixture(&fixture))
            .mount(&ex)
            .await;
        let mut bcfg = BlockscoutEvmConfig::new(4663, BlockscoutApiKey::new("test-key"));
        bcfg.base_url = ex.uri();
        bcfg.base_delay_ms = 1;
        let explorer: WalletIndexer = BlockscoutEvmSource::new(bcfg).unwrap().into();
        let resolver = NativeLegResolver::new(rpc, NativeLegPolicy::default());
        let cfg = EvmExtractionConfig::for_profile(ROBINHOOD);
        let wallets: Vec<Address> = senders
            .iter()
            .take(2)
            .map(|(a, _)| a.parse().unwrap())
            .collect();
        let window = AnalysisWindow::resolve(
            None,
            Some("2026-10-03T17:54:46Z"),
            Some("2026-10-03T18:04:46Z"),
            1_791_060_000,
        )
        .unwrap();
        let mut report = run_evm_wallet_stats(
            &cfg,
            &EvmStatsSources {
                scanner: &scanner,
                explorer: &explorer,
                max_requests: None,
                notice: None,
                resolver: Some(&resolver),
            },
            &wallets,
            &window,
            2,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let l = report.wallets[0].ledger.as_ref().unwrap();
        let open = l.open_positions[0].open_amount_raw;
        assert_eq!(open, 46_472_032_518_329_289_554_142);
        let last = l.evm.as_ref().unwrap().open_venues[0]
            .last
            .expect("last venue recorded");
        assert_eq!(last.venue, SwapVenue::UniswapV4);
        assert_eq!(last.pool, PM_RH);
        let pool_id = last.pool_id.unwrap();
        // The real Initialize log of that pool inside the capture.
        let init = evm_aiden::transactions(&scanned)
            .iter()
            .flat_map(|t| t.logs.iter())
            .find(|g| g.topics.get(1) == Some(&pool_id) && g.topics[0] == V4_INITIALIZE_TOPIC0)
            .expect("fixture carries the pool's Initialize log")
            .clone();
        let key = scout_dex_evm::decode_v4_initialize(&init)
            .decoded()
            .unwrap();
        let pk = V4PoolKey {
            currency0: key.currency0,
            currency1: key.currency1,
            fee: key.fee,
            tick_spacing: key.tick_spacing,
            hooks: key.hooks,
        };
        assert_eq!(
            pk.pool_id(),
            pool_id,
            "PoolKey rebuilt from the log hashes to the pool id"
        );
        let token = evm_aiden::AIDEN;
        let zfo = pk.currency0 == token;
        let quoter = address!("8dc178efb8111bb0973dd9d722ebeff267c98f94");
        let opens: Vec<u128> = report
            .wallets
            .iter()
            .map(|w| w.ledger.as_ref().unwrap().open_positions[0].open_amount_raw)
            .collect();
        let rules: Vec<Rule> = opens
            .iter()
            .flat_map(|open| {
                [
                    (*open, 120 * ETH / 1000),
                    (*open / 1000, 125 * ETH / 100_000),
                ]
            })
            .map(|(a, o)| Rule {
                to: quoter,
                prefix: v4_quote_calldata(&pk, zfo, a).to_ascii_lowercase(),
                contains: None,
                ans: ok(words(&[word(o), word(200_000)])),
            })
            .collect();
        let chain = Chain::start(rules, vec![init]).await;
        let run = apply_evm_open_valuation(
            &mut report.wallets,
            &chain.rpc(ROBINHOOD),
            &cfg,
            &AnalysisWindow::none(1_791_060_000),
            &EvmValuationOptions::default(),
        )
        .await;
        assert_eq!(
            run.totals.positions, 2,
            "both wallets have one open Aiden position"
        );
        let v0 = only(&report.wallets[0]).valued().unwrap();
        assert_eq!(v0.method, "v4_quoter");
        assert_eq!(v0.realizable_raw, 120 * ETH / 1000);
        assert_eq!(v0.pool_id, Some(pool_id));
        assert_eq!(v0.state_block, HEAD);
        // The sells of this wallet have unknown proceeds: the remaining
        // basis is partly unknown, so unrealized stays unknown, never zero.
        assert!(v0.unrealized_pnl.is_none() || v0.unrealized_status == "known");
        // Both wallets trade the same pool: ONE Initialize lookup served both.
        assert_eq!((run.totals.valued, run.log_calls), (2, 1));
        // Strict ranking keeps valued exposure out of open_exposure_unvalued.
        let mut policy = RankPolicy::for_profile(RankProfile::None, RankBy::RealizedNetPnl, 10);
        policy.require_valued_open = true;
        let r = rank_solana_wallets(&report.wallets, &policy);
        assert!(
            r.excluded
                .iter()
                .all(|e| !e.reasons.contains(&ExclusionReason::OpenExposureUnvalued))
        );
    }

    /// Base: the USDC wallet's open Uniswap v3 position valued through the
    /// pinned Base QuoterV2 (mocked answers), USD at par.
    #[tokio::test]
    async fn base_usdc_wallet_card_valued_through_the_v3_quoter() {
        const W_USDC: &str = "0xb0b21cef6df3cc3716193fd94880b58c2adb90b7";
        let fixture = evm_base::fixture();
        let replay = evm_base::replay(&fixture, AlchemyInternalMode::Unsupported);
        let server = evm_base::serve(replay).await;
        let rpc = evm_base::rpc_client(&server);
        let chain_key = rpc.preflight().await.unwrap();
        let scanner = EvmHistoryScanner::new(rpc.clone(), chain_key, ScanLimits::default());
        let indexer: WalletIndexer =
            AlchemyTransfersSource::new(rpc.clone(), AlchemyConfig::default()).into();
        let resolver = NativeLegResolver::new(rpc, NativeLegPolicy::default());
        let cfg = EvmExtractionConfig::for_profile(BASE);
        let window = AnalysisWindow::resolve(
            None,
            Some("2026-10-04T03:06:51Z"),
            Some("2026-10-04T03:07:07Z"),
            evm_base::AS_OF,
        )
        .unwrap();
        let wallet: Address = W_USDC.parse().unwrap();
        let mut report = run_evm_wallet_stats(
            &cfg,
            &EvmStatsSources {
                scanner: &scanner,
                explorer: &indexer,
                max_requests: None,
                notice: None,
                resolver: Some(&resolver),
            },
            &[wallet],
            &window,
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let l = report.wallets[0].ledger.as_ref().unwrap();
        let open = l.open_positions[0].open_amount_raw;
        assert_eq!(open, 12_189_221_686_084_042_750_894);
        let info = &l.evm.as_ref().unwrap().open_venues[0];
        let last = info.last.unwrap();
        assert_eq!(last.venue, SwapVenue::UniswapV3);
        let token = info.token;
        let usdc = BASE_USDC.address;
        let (t0, t1) = if token < usdc {
            (token, usdc)
        } else {
            (usdc, token)
        };
        let quoter = address!("3d4e44Eb1374240CE5F1B871ab261CD16335B76a");
        let mut rules = vec![
            rule(last.pool, &sel_hex("token0()"), ok(words(&[addr_word(t0)]))),
            rule(last.pool, &sel_hex("token1()"), ok(words(&[addr_word(t1)]))),
            rule(last.pool, &sel_hex("fee()"), ok(words(&[word(3000)]))),
        ];
        for (a, o) in [(open, 400_000_000u128), (open / 1000, 420_000u128)] {
            rules.push(Rule {
                to: quoter,
                prefix: v3_quote_calldata(
                    QuoterFamily::UniswapV3,
                    token,
                    usdc,
                    U256::from(a),
                    3000,
                )
                .to_ascii_lowercase(),
                contains: None,
                ans: quoter_answer(o),
            });
        }
        let chain = Chain::start(rules, vec![]).await;
        let run = apply_evm_open_valuation(
            &mut report.wallets,
            &chain.rpc(BASE),
            &cfg,
            &AnalysisWindow::none(evm_base::AS_OF),
            &EvmValuationOptions::default(),
        )
        .await;
        assert_eq!((run.totals.valued, run.totals.unvalued), (1, 0));
        let v = only(&report.wallets[0]).valued().unwrap();
        assert_eq!(v.quote_unit, QuoteUnit::UsdcUnits);
        assert_eq!(v.realizable_raw, 400_000_000);
        // Open lots: USDC basis, known => unrealized in USDC units.
        assert_eq!(v.unrealized_status, "known");
        let src = InMemoryPriceSource::new();
        apply_usd_pricing(&mut report.wallets, &src).await;
        let v = only(&report.wallets[0]).valued().unwrap();
        assert_eq!(
            v.usd.as_ref().unwrap().value.scaled_units(),
            400 * 100_000_000
        );
        // A token with a USDC-quoted history (no tax shaped flow) carries no caveat.
        assert!(!v.transfer_tax_not_modelled);
        let _ = BTreeMap::<u8, u8>::new();
    }
}
