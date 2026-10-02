//! Offline tests for the Solana wallet-rank engine: synthetic ledger
//! reports for every gate / ordering rule, plus one end-to-end run over
//! the committed live fixtures (decoded by the real `HeliusProvider` via
//! wiremock) through `run_solana_wallet_stats` into `rank_solana_wallets`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::StreamExt as _;
use futures::stream::{self, BoxStream};
use scout_analytics::RatioStatus;
use scout_api::{
    HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest, ScanTask,
    SourceCapabilities,
};
use scout_core::{AddressBytes, RawPayload, RawSolanaTransaction, SolanaPubkey, WalletKey};
use scout_dex_solana::{TradeEventPairing, pair_trades_with_events};
use scout_engine::{
    ActivityMetrics, ExclusionReason as R, OpenPosition, RankBy, RankPolicy, RankProfile,
    SolanaWalletStats, WalletScanStatus, build_solana_wallet_ledger, lamports_to_money,
    pump_bonding_curve_decoder, rank_solana_wallets, run_solana_wallet_stats, solana_mainnet_chain,
};
use scout_providers::{HeliusProvider, ScanOrder};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Synthetic complete wallet. `pnl`/`basis` in lamports (net == trade pnl).
struct Spec {
    b: u8,
    closed: u64,
    pnl: i128,
    basis: i128,
    days: u64,
    trades: u64,
    mint_days: u64,
}

impl Spec {
    fn new(b: u8) -> Self {
        Self {
            b,
            closed: 25,
            pnl: 1_000,
            basis: 10_000,
            days: 10,
            trades: 100,
            mint_days: 50,
        }
    }
    fn closed(mut self, n: u64) -> Self {
        self.closed = n;
        self
    }
    fn pnl(mut self, pnl: i128, basis: i128) -> Self {
        self.pnl = pnl;
        self.basis = basis;
        self
    }
    fn activity(mut self, days: u64, trades: u64, mint_days: u64) -> Self {
        self.days = days;
        self.trades = trades;
        self.mint_days = mint_days;
        self
    }
    fn build(self) -> SolanaWalletStats {
        let decoder = pump_bonding_curve_decoder().unwrap();
        let mut l = build_solana_wallet_ledger(&[self.b; 32], &[], &decoder).unwrap();
        l.closed_episodes_known = self.closed;
        l.realized_trade_pnl_lamports = self.pnl;
        l.realized_trade_pnl_exact = lamports_to_money(self.pnl).unwrap();
        l.realized_net_pnl_lamports = self.pnl;
        l.realized_net_pnl_exact = lamports_to_money(self.pnl).unwrap();
        l.consumed_acquisition_basis_lamports = self.basis;
        l.consumed_acquisition_basis_exact = lamports_to_money(self.basis).unwrap();
        l.activity = ActivityMetrics {
            timestamped_trades: self.trades,
            active_utc_days: self.days,
            mint_day_pairs: self.mint_days,
            ..ActivityMetrics::default()
        };
        l.profit_factor = RatioStatus::Value {
            value: lamports_to_money(2).unwrap(),
        };
        SolanaWalletStats {
            wallet: [self.b; 32],
            status: WalletScanStatus::Ok,
            transactions_scanned: Some(self.trades),
            truncated: false,
            unexpected_payloads: 0,
            error: None,
            ledger: Some(l),
            incomplete_reasons: Vec::new(),
            failure: None,
            not_scanned: None,
        }
    }
}

fn policy(profile: RankProfile, by: RankBy, top: usize) -> RankPolicy {
    RankPolicy::for_profile(profile, by, top)
}

fn ranked_ids(rep: &scout_engine::WalletRankReport) -> Vec<u8> {
    rep.ranked.iter().map(|r| r.observation.wallet[0]).collect()
}

fn excluded_of(rep: &scout_engine::WalletRankReport, b: u8) -> &scout_engine::ExcludedWallet {
    rep.excluded
        .iter()
        .find(|e| e.observation.wallet[0] == b)
        .unwrap()
}

fn assert_partition(rep: &scout_engine::WalletRankReport) {
    assert_eq!(rep.ranked.len() + rep.excluded.len(), rep.input_count);
}

#[test]
fn quality_gates_each_fire_and_reasons_are_ordered() {
    let wallets = vec![
        Spec::new(1).build(),                               // passes
        Spec::new(2).closed(19).build(),                    // sample
        Spec::new(3).activity(6, 60, 30).build(),           // days
        Spec::new(4).closed(3).activity(2, 20, 10).build(), // both
    ];
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![1]);
    assert_eq!(
        excluded_of(&rep, 2).reasons,
        vec![R::InsufficientClosedEpisodes]
    );
    assert_eq!(
        excluded_of(&rep, 3).reasons,
        vec![R::InsufficientActiveDays]
    );
    assert_eq!(
        excluded_of(&rep, 4).reasons,
        vec![R::InsufficientClosedEpisodes, R::InsufficientActiveDays]
    );
    assert_eq!(
        excluded_of(&rep, 4).primary_reason(),
        R::InsufficientClosedEpisodes
    );
    // Boundary: exactly 20 / 7 passes.
    let edge = vec![Spec::new(9).closed(20).activity(7, 70, 35).build()];
    let rep = rank_solana_wallets(
        &edge,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 5),
    );
    assert_eq!(ranked_ids(&rep), vec![9]);
    assert_partition(&rep);
    let counts = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    )
    .all_reason_counts();
    assert_eq!(counts[&R::InsufficientClosedEpisodes], 2);
    assert_eq!(counts[&R::InsufficientActiveDays], 2);
}

#[test]
fn status_gates_unknown_basis_and_open_exposure() {
    let mut inc = Spec::new(2).build();
    inc.status = WalletScanStatus::Incomplete;
    inc.truncated = true;
    let mut err = Spec::new(3).build();
    err.status = WalletScanStatus::Error;
    err.ledger = None;
    err.transactions_scanned = None;
    err.error = Some("boom".into());
    let mut idle = Spec::new(4).build();
    idle.status = WalletScanStatus::NoActivity;
    let mut nopump = Spec::new(5).build();
    nopump.status = WalletScanStatus::NoPumpActivity;
    let mut unk_ep = Spec::new(6).build();
    unk_ep.ledger.as_mut().unwrap().closed_episodes_unknown = 1;
    unk_ep.ledger.as_mut().unwrap().has_unknown_basis_inventory = true;
    let mut unk_inv = Spec::new(7).build();
    unk_inv.ledger.as_mut().unwrap().has_unknown_basis_inventory = true;
    let mut open = Spec::new(8).build();
    open.ledger.as_mut().unwrap().open_positions = vec![OpenPosition {
        mint: [200; 32],
        open_amount_raw: 600,
        unknown_basis_amount_raw: 0,
        opened_at: Some(1),
    }];
    let wallets = vec![
        Spec::new(1).build(),
        inc,
        err,
        idle,
        nopump,
        unk_ep,
        unk_inv,
        open,
    ];

    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert_partition(&rep);
    // Open exposure with known basis is INCLUDED by default and flagged.
    // Equal figures: wallet key ascending.
    assert_eq!(ranked_ids(&rep), vec![1, 8]);
    let o = rep
        .ranked
        .iter()
        .find(|r| r.observation.wallet[0] == 8)
        .unwrap();
    assert_eq!(o.observation.open_exposure.label(), "unvalued");
    assert_eq!(excluded_of(&rep, 2).reasons, vec![R::IncompleteCoverage]);
    assert_eq!(excluded_of(&rep, 3).reasons, vec![R::ProviderError]);
    assert_eq!(excluded_of(&rep, 4).reasons, vec![R::NoActivity]);
    assert_eq!(excluded_of(&rep, 5).reasons, vec![R::NoPumpActivity]);
    assert_eq!(excluded_of(&rep, 6).reasons, vec![R::UnknownBasis]);
    assert_eq!(excluded_of(&rep, 7).reasons, vec![R::UnknownBasis]);

    let mut strict = policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20);
    strict.require_no_open = true;
    let rep = rank_solana_wallets(&wallets, &strict);
    assert_eq!(ranked_ids(&rep), vec![1]);
    assert_eq!(excluded_of(&rep, 8).reasons, vec![R::OpenExposure]);
    assert_partition(&rep);

    // `none` profile: unknown-basis wallets are not excluded for it, but
    // scan-status gates still apply.
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::None, RankBy::RealizedNetPnl, 20),
    );
    let mut ids = ranked_ids(&rep);
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 6, 7, 8]);
    assert_eq!(excluded_of(&rep, 3).reasons, vec![R::ProviderError]);
    let six = rep
        .ranked
        .iter()
        .find(|r| r.observation.wallet[0] == 6)
        .unwrap();
    assert_eq!(six.observation.pnl_status.label(), "known_subset");
}

#[test]
fn none_profile_never_ranks_unknown_metric_wallets() {
    // No known closed episode and no failed fee: net PnL is N/A.
    let mut na = Spec::new(2).closed(0).build();
    {
        let l = na.ledger.as_mut().unwrap();
        l.closed_episodes_known = 0;
        l.closed_episodes_unknown = 2;
        l.realized_net_pnl_lamports = 0;
        l.profit_factor = RatioStatus::Undefined;
    }
    let wallets = vec![Spec::new(1).closed(1).activity(1, 2, 1).build(), na];
    for by in [
        RankBy::RealizedNetPnl,
        RankBy::RealizedCostRoi,
        RankBy::ProfitFactor,
    ] {
        let mut p = policy(RankProfile::None, by, 20);
        p.exclude_unknown_basis = false;
        let rep = rank_solana_wallets(&wallets, &p);
        assert_eq!(ranked_ids(&rep), vec![1], "{by:?}");
        assert_eq!(
            excluded_of(&rep, 2).reasons,
            vec![R::MetricUnknown],
            "{by:?}"
        );
    }
}

#[test]
fn sort_chain_pnl_then_roi_then_closed_then_key() {
    let wallets = vec![
        Spec::new(1).pnl(500, 5_000).closed(30).build(), // roi 10%
        Spec::new(2).pnl(500, 2_500).closed(25).build(), // roi 20% -> beats 1
        Spec::new(3).pnl(900, 90_000).build(),           // best pnl
        Spec::new(4).pnl(500, 2_500).closed(40).build(), // ties 2 on pnl+roi, more closed
        Spec::new(5).pnl(500, 2_500).closed(40).build(), // ties 4 fully -> key asc
    ];
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![3, 4, 5, 2, 1]);
    assert_eq!(rep.ranked[0].rank, 1);
    assert_eq!(rep.ranked[4].rank, 5);
    // By ROI: 2,4,5 (20%), then 1 (10%), then 3 (1%).
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedCostRoi, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![4, 5, 2, 1, 3]);
}

#[test]
fn roi_ties_broken_by_exact_rational_not_rounding() {
    // 1/3 vs 333_333/1_000_000: identical at 4 digits, A is strictly larger.
    let wallets = vec![
        Spec::new(1).pnl(333_333, 1_000_000).build(),
        Spec::new(2).pnl(1, 3).build(),
    ];
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::None, RankBy::RealizedCostRoi, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![2, 1]);
    assert_eq!(
        rep.ranked[0].observation.roi.unwrap().denominator,
        3 * 100_000_000
    );
}

#[test]
fn profit_factor_unbounded_above_finite_and_undefined_excluded() {
    let mk = |b: u8, pf: RatioStatus<scout_core::Money>| {
        let mut w = Spec::new(b).build();
        w.ledger.as_mut().unwrap().profit_factor = pf;
        w
    };
    let m = |n: i128| lamports_to_money(n * 100_000_000).unwrap();
    let wallets = vec![
        mk(1, RatioStatus::Value { value: m(3) }),
        mk(2, RatioStatus::NoObservedLosses),
        mk(3, RatioStatus::Undefined),
        mk(4, RatioStatus::Value { value: m(9) }),
    ];
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::ProfitFactor, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![2, 4, 1]);
    assert_eq!(excluded_of(&rep, 3).reasons, vec![R::MetricUnknown]);
    // A no_observed_losses wallet that fails a sample gate stays out.
    let mut small = mk(5, RatioStatus::NoObservedLosses);
    small.ledger.as_mut().unwrap().closed_episodes_known = 2;
    let rep = rank_solana_wallets(
        &[small],
        &policy(RankProfile::Quality, RankBy::ProfitFactor, 20),
    );
    assert!(rep.ranked.is_empty());
    assert_eq!(rep.excluded[0].reasons, vec![R::InsufficientClosedEpisodes]);
}

#[test]
fn top_n_truncates_and_never_pads_and_keeps_wallets_visible() {
    let wallets: Vec<_> = (1..=5u8)
        .map(|b| Spec::new(b).pnl(i128::from(b) * 100, 10_000).build())
        .collect();
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 2),
    );
    assert_eq!(ranked_ids(&rep), vec![5, 4]);
    assert_eq!(rep.eligible_count, 5);
    assert_eq!(rep.excluded.len(), 3);
    assert!(rep.excluded.iter().all(|e| e.reasons == vec![R::BelowTopN]));
    assert_eq!(excluded_of(&rep, 3).eligible_rank, Some(3));
    assert_partition(&rep);
    // More top than eligible: min(N, eligible), never padded with failures.
    let mut few = vec![Spec::new(1).build(), Spec::new(2).closed(1).build()];
    few.push(Spec::new(3).build());
    let rep = rank_solana_wallets(
        &few,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(rep.ranked.len(), 2);
    assert_eq!(rep.excluded.len(), 1);
    // Empty universe and all-excluded are normal, empty results.
    let rep = rank_solana_wallets(
        &[],
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert!(rep.ranked.is_empty() && rep.excluded.is_empty());
}

#[test]
fn insider_admits_concentrated_buyer_and_rejects_bot_quality_rejects_small_sample() {
    // Insider-like: 6 closed episodes over 4 days, 3 mints/day, 8 trades/day.
    let insider = Spec::new(1).closed(6).activity(4, 32, 12).build();
    // Bot-like: 400 closed, ~70 mints/day, ~150 trades/day over 10 days.
    let bot = Spec::new(2).closed(400).activity(10, 1_500, 700).build();
    // Bot exceeding only the trades ceiling (9 mints/day, 31 trades/day).
    let trade_heavy = Spec::new(3).closed(50).activity(10, 310, 90).build();
    let wallets = vec![insider, bot, trade_heavy];

    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Insider, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(ranked_ids(&rep), vec![1]);
    assert_eq!(
        excluded_of(&rep, 2).reasons,
        vec![
            R::ActivityCeilingTradesPerDay,
            R::ActivityCeilingMintsPerDay
        ]
    );
    assert_eq!(
        excluded_of(&rep, 3).reasons,
        vec![R::ActivityCeilingTradesPerDay]
    );

    // Under quality the insider fails on sample size (6 < 20); the bot passes
    // quality (no ceiling there), which is exactly why the cohort needs its own profile.
    let rep = rank_solana_wallets(
        &wallets,
        &policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(
        excluded_of(&rep, 1).primary_reason(),
        R::InsufficientClosedEpisodes
    );
    assert!(ranked_ids(&rep).contains(&2));

    // Overrides apply on top of the profile; ceiling exactly at the limit passes.
    let mut p = policy(RankProfile::Insider, RankBy::RealizedNetPnl, 20);
    p.max_trades_per_day = Some(31);
    p.max_mints_per_day = Some(9);
    let rep = rank_solana_wallets(&wallets, &p);
    assert!(ranked_ids(&rep).contains(&3));
}

#[test]
fn ceiling_without_timestamped_trades_is_activity_unknown() {
    let w = Spec::new(1).closed(6).activity(0, 0, 0).build();
    let rep = rank_solana_wallets(
        &[w],
        &policy(RankProfile::Insider, RankBy::RealizedNetPnl, 20),
    );
    assert_eq!(
        rep.excluded[0].reasons,
        vec![R::InsufficientActiveDays, R::ActivityUnknown]
    );
}

#[test]
fn ranking_is_deterministic_under_input_permutation() {
    let wallets: Vec<_> = (1..=6u8)
        .map(|b| Spec::new(b).pnl(i128::from(b % 3) * 100, 1_000).build())
        .collect();
    let p = policy(RankProfile::Quality, RankBy::RealizedNetPnl, 20);
    let a = ranked_ids(&rank_solana_wallets(&wallets, &p));
    let mut rev = wallets.clone();
    rev.reverse();
    assert_eq!(a, ranked_ids(&rank_solana_wallets(&rev, &p)));
}

// ---------------------------------------------------------------------
// End to end over committed live fixtures
// ---------------------------------------------------------------------

async fn fixture_txs(name: &str) -> Vec<RawSolanaTransaction> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name);
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let data: Vec<serde_json::Value> = if let Some(pages) = fixture["pages"].as_array() {
        pages
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    } else {
        fixture["result"]["data"].as_array().unwrap().clone()
    };
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "data": data, "paginationToken": null }
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let provider =
        HeliusProvider::new_with_endpoint(scout_rpc::RpcEndpoint::new(server.uri()), 5_000, 1)
            .unwrap()
            .with_scan_order(ScanOrder::NewestFirst);
    let wallet = WalletKey {
        chain: solana_mainnet_chain(),
        address: AddressBytes::Solana([3; 32]),
    };
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::WalletActivity { wallet },
            description: "test".to_string(),
        },
        CancellationToken::new(),
    );
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        if let RawPayload::SolanaTransaction(tx) = item.unwrap().payload {
            out.push(tx);
        }
    }
    out
}

struct Stub {
    by_wallet: BTreeMap<SolanaPubkey, Vec<RawSolanaTransaction>>,
}

#[async_trait::async_trait]
impl HistoryProvider for Stub {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::empty()
    }
    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        Ok(ScanPlan {
            request_echo: format!("{request:?}"),
            capabilities: SourceCapabilities::empty(),
        })
    }
    fn scan(
        &self,
        task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let ScanRequest::WalletActivity { wallet } = &task.request else {
            panic!("expected wallet request");
        };
        let AddressBytes::Solana(addr) = wallet.address else {
            panic!("expected solana");
        };
        let items: Vec<Result<ScanEnvelope, ProviderError>> = self
            .by_wallet
            .get(&addr)
            .map(|txs| {
                txs.iter()
                    .map(|tx| {
                        Ok(ScanEnvelope {
                            payload: RawPayload::SolanaTransaction(tx.clone()),
                            truncated: false,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Box::pin(stream::iter(items))
    }
}

#[tokio::test]
async fn end_to_end_fixtures_stats_then_rank_accounts_for_every_wallet() {
    let mut txs = fixture_txs("pump_variants_live_2026-10-02.json").await;
    txs.extend(fixture_txs("pump_bonding_curve_buy_probe.json").await);
    let decoder = pump_bonding_curve_decoder().unwrap();
    let mut users: Vec<SolanaPubkey> = Vec::new();
    for tx in &txs {
        let rep =
            pair_trades_with_events(&decoder, &tx.instructions, tx.slot, tx.transaction_index);
        for p in &rep.trades {
            if matches!(p.pairing, TradeEventPairing::Paired(_)) && !users.contains(&p.trade.user) {
                users.push(p.trade.user);
            }
        }
    }
    assert!(users.len() >= 2);
    let idle: SolanaPubkey = [77; 32];
    let by_wallet: BTreeMap<_, _> = users.iter().map(|u| (*u, txs.clone())).collect();
    let stub = Stub { by_wallet };
    let mut input = users.clone();
    input.push(idle);
    let stats = run_solana_wallet_stats(&stub, &input, &decoder, CancellationToken::new())
        .await
        .unwrap();

    for profile in [
        RankProfile::Quality,
        RankProfile::Insider,
        RankProfile::None,
    ] {
        let rep = rank_solana_wallets(&stats.wallets, &policy(profile, RankBy::RealizedNetPnl, 20));
        assert_eq!(rep.input_count, input.len());
        assert_eq!(rep.ranked.len() + rep.excluded.len(), input.len());
        // The idle wallet is always visible with its status reason.
        assert_eq!(excluded_of(&rep, 77).reasons, vec![R::NoActivity]);
        // Rank and stats share one ledger result: figures are identical.
        for r in &rep.ranked {
            let s = stats
                .wallets
                .iter()
                .find(|w| w.wallet == r.observation.wallet)
                .unwrap();
            assert_eq!(
                r.observation.net_pnl_lamports,
                s.ledger.as_ref().map(|l| l.realized_net_pnl_lamports)
            );
        }
        // Tiny fixtures can never satisfy the 20/5-episode gates.
        if profile != RankProfile::None {
            assert!(rep.ranked.is_empty(), "{profile:?}");
        }
    }
}
