//! Bounded concurrency + time slicing of the scan engines (invariants 11,
//! 12, 13): a fake provider with pseudo-random per-item delays shuffles the
//! completion order; results must not move. The fake also measures peak
//! live streams (= in-flight requests) and records every call.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::type_complexity
)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::stream::{self, BoxStream};
use scout_api::{
    HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest, ScanTask,
    SourceCapabilities,
};
use scout_core::{
    AddressBytes, AssetKey, RawPayload, RawSolanaInstruction, RawSolanaTransaction,
    SolanaExecutionStatus, SolanaPubkey, SolanaTokenBalanceChange,
};
use scout_dex_solana::{BUY_INSTRUCTION_DISCRIMINATOR, SELL_INSTRUCTION_DISCRIMINATOR};
use scout_engine::{
    AnalysisWindow, IntersectOptions, LedgerDecoders, PUMP_BONDING_CURVE_PROGRAM_ID,
    ScanFailureKind, ScanStop, SideFilter, SolanaBuyerIntersectReport, TokenScanStatus,
    WalletScanStatus, WindowSource, pump_amm_decoder, pump_bonding_curve_decoder,
    run_solana_trade_intersect, run_solana_wallet_stats_concurrent, solana_mainnet_chain,
};
use tokio_util::sync::CancellationToken;

// ---- synthetic data ------------------------------------------------------

fn pk(b: u8) -> SolanaPubkey {
    [b; 32]
}

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

fn token(b: u8) -> AssetKey {
    AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(pk(b)))
}

fn curve_tx(buy: bool, user: u8, mint: u8, sig: u8, slot: u64, t: i64) -> RawSolanaTransaction {
    let n = if buy { 16u8 } else { 14u8 };
    let mut accounts: Vec<SolanaPubkey> = (0..n).map(|i| pk(100 + i)).collect();
    accounts[2] = pk(mint);
    accounts[6] = pk(user);
    let mut data = if buy {
        BUY_INSTRUCTION_DISCRIMINATOR.to_vec()
    } else {
        SELL_INSTRUCTION_DISCRIMINATOR.to_vec()
    };
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&if buy { 2u64 } else { 0u64 }.to_le_bytes());
    if buy {
        data.push(1);
    }
    RawSolanaTransaction {
        block_time: Some(t),
        signature: [sig; 64],
        execution: SolanaExecutionStatus::Succeeded,
        slot,
        transaction_index: 0,
        instructions: vec![RawSolanaInstruction {
            program_id: pubkey(PUMP_BONDING_CURVE_PROGRAM_ID),
            accounts,
            data,
            instruction_index: 0,
        }],
        token_balance_changes: vec![SolanaTokenBalanceChange {
            mint: pk(mint),
            owner: Some(pk(user)),
            decimals: 6,
            pre_amount: if buy { None } else { Some(10) },
            post_amount: if buy { 10 } else { 0 },
            closed: false,
        }],
        fee_lamports: 5_000,
        fee_payer: pk(user),
        signers: vec![pk(user)],
        native_balance_changes: vec![],
    }
}

fn window(since: i64, until: i64) -> AnalysisWindow {
    AnalysisWindow {
        since,
        until,
        as_of: until + 1_000,
        source: WindowSource::Explicit,
    }
}

/// 8 tokens, distinct wallets/sides/times in `[1000, 5000)`; every wallet
/// trades several tokens so K=2 has matches.
fn dataset() -> BTreeMap<u8, Vec<RawSolanaTransaction>> {
    let mut out = BTreeMap::new();
    let mut sig = 0u8;
    for mint in 1..=8u8 {
        let mut txs = Vec::new();
        for i in 0..10u8 {
            sig += 1;
            let user = 20 + (mint + i) % 6;
            let buy = (mint + i) % 3 != 0;
            let t = 1000 + i64::from(mint) * 37 + i64::from(i) * 391;
            txs.push(curve_tx(
                buy,
                user,
                mint,
                sig,
                1_000 + u64::from(sig) * 10,
                t,
            ));
        }
        out.insert(mint, txs);
    }
    out
}

// ---- fake provider -------------------------------------------------------

type FailFn = Box<dyn Fn(u8, Option<i64>) -> Option<ProviderError> + Send + Sync>;

struct Fake {
    by_key: BTreeMap<u8, Vec<RawSolanaTransaction>>,
    /// Honor `scan_block_time_range` (filter server-side).
    honor_range: bool,
    /// Widen the honored range by this many seconds on both sides
    /// (boundary re-delivery).
    widen: i64,
    /// Delay before every item: `f(call_index, key, item_index)` ms.
    delay_ms: fn(u64, u8, usize) -> u64,
    /// `Some(err)` = the stream yields this error INSTEAD of data, at once.
    fail: Option<FailFn>,
    /// Mark the last envelope truncated when the call's `gte` equals this.
    truncate_when_gte: Option<i64>,
    /// Emit the first item at once (later items still use `delay_ms`), so a
    /// cut scan has already produced data.
    first_item_free: bool,
    /// Wait this long before a `fail` error is yielded (lets the other
    /// units start first).
    fail_delay_ms: u64,
    calls: Mutex<Vec<(u8, Option<i64>, Option<i64>)>>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    counter: AtomicU64,
}

impl Fake {
    fn new(by_key: BTreeMap<u8, Vec<RawSolanaTransaction>>) -> Self {
        Self {
            by_key,
            honor_range: true,
            widen: 0,
            delay_ms: |_, _, _| 0,
            fail: None,
            truncate_when_gte: None,
            first_item_free: false,
            fail_delay_ms: 0,
            calls: Mutex::new(Vec::new()),
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            counter: AtomicU64::new(0),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn calls(&self) -> Vec<(u8, Option<i64>, Option<i64>)> {
        self.calls.lock().unwrap().clone()
    }

    fn key_of(request: &ScanRequest) -> u8 {
        match request {
            ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(_, AddressBytes::Solana(m)),
            } => m[0],
            ScanRequest::WalletActivity { wallet } => match wallet.address {
                AddressBytes::Solana(w) => w[0],
                _ => 0,
            },
            _ => 0,
        }
    }

    fn open(
        &self,
        task: &ScanTask,
        range: Option<(Option<i64>, Option<i64>)>,
        cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        let key = Self::key_of(&task.request);
        let call = self.counter.fetch_add(1, Ordering::SeqCst);
        let (gte, lt) = range.unwrap_or((None, None));
        self.calls.lock().unwrap().push((key, gte, lt));
        let live = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(live, Ordering::SeqCst);
        let guard = Guard(self.active.clone());

        let mut items: VecDeque<Result<ScanEnvelope, ProviderError>> = VecDeque::new();
        let mut instant_error = false;
        if let Some(err) = self.fail.as_ref().and_then(|f| f(key, gte)) {
            items.push_back(Err(err));
            instant_error = true;
        } else {
            let mut txs: Vec<&RawSolanaTransaction> = self
                .by_key
                .get(&key)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            if self.honor_range && range.is_some() {
                txs.retain(|tx| {
                    let t = tx.block_time.unwrap();
                    gte.is_none_or(|g| t >= g - self.widen) && lt.is_none_or(|l| t < l + self.widen)
                });
            }
            let n = txs.len();
            for (i, tx) in txs.into_iter().enumerate() {
                let truncated = i + 1 == n && gte.is_some() && gte == self.truncate_when_gte;
                items.push_back(Ok(ScanEnvelope {
                    payload: RawPayload::SolanaTransaction(tx.clone()),
                    truncated,
                }));
            }
        }
        let delay_ms = self.delay_ms;
        let first_free = self.first_item_free;
        let fail_delay_ms = self.fail_delay_ms;
        Box::pin(stream::unfold(
            (items, guard, 0usize, cancel),
            move |(mut items, guard, idx, cancel)| async move {
                let item = items.pop_front()?;
                let ms = if instant_error {
                    fail_delay_ms
                } else if first_free && idx == 0 {
                    0
                } else {
                    delay_ms(call, key, idx)
                };
                if ms > 0 {
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_millis(ms)) => {}
                        () = cancel.cancelled() => {}
                    }
                }
                Some((item, (items, guard, idx + 1, cancel)))
            },
        ))
    }
}

struct Guard(Arc<AtomicUsize>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl HistoryProvider for Fake {
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
        cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        self.open(&task, None, cancel)
    }

    fn scan_block_time_range(
        &self,
        task: ScanTask,
        cancel: CancellationToken,
        gte: Option<i64>,
        lt: Option<i64>,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        self.open(&task, Some((gte, lt)), cancel)
    }
}

fn budget_error() -> ProviderError {
    ProviderError::Other(Box::new(scout_rpc::RequestBudgetExhausted { limit: 7 }))
}

fn opts(window: AnalysisWindow, concurrency: usize, slices: u32) -> IntersectOptions {
    IntersectOptions {
        side: SideFilter::Any,
        window,
        concurrency,
        slices,
    }
}

async fn run(
    provider: &Fake,
    tokens: &[u8],
    o: IntersectOptions,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    let assets: Vec<AssetKey> = tokens.iter().map(|t| token(*t)).collect();
    run_solana_trade_intersect(provider, &assets, 2, o, CancellationToken::new()).await
}

/// Everything the JSONL renderer reads from a report, in a stable text form
/// (all containers are ordered; `concurrency`/`slices` are run settings).
fn dump(r: &SolanaBuyerIntersectReport) -> String {
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        r.base.matches,
        r.per_token,
        r.side_hits,
        r.diagnostics,
        r.trade,
        r.malformed_samples,
        r.unknown_discriminator_samples,
        (r.cancelled, r.stop),
        r.incomplete_reasons(),
    )
}

// ---- determinism ---------------------------------------------------------

#[tokio::test]
async fn results_are_byte_identical_whatever_the_completion_order() {
    let tokens: Vec<u8> = (1..=8).collect();
    let w = window(1000, 5000);

    let baseline = {
        let p = Fake::new(dataset());
        dump(&run(&p, &tokens, opts(w, 1, 1)).await.unwrap())
    };
    assert!(baseline.contains("hit_count"), "the fixture must match");

    // Three delay schedules: pseudo-random, earlier-tokens-slower and
    // reversed; several concurrency/slice shapes each.
    let schedules: [fn(u64, u8, usize) -> u64; 3] = [
        |c, k, i| (c * 7919 + u64::from(k) * 104_729 + i as u64 * 31) % 6,
        |_, k, _| u64::from(9 - k),
        |_, k, i| u64::from(k) + (i as u64 % 2),
    ];
    for schedule in schedules {
        for (concurrency, slices) in [(2, 1), (4, 1), (8, 1), (16, 1), (4, 4), (16, 8)] {
            let mut p = Fake::new(dataset());
            p.delay_ms = schedule;
            let r = run(&p, &tokens, opts(w, concurrency, slices))
                .await
                .unwrap();
            let got = dump(&r);
            if slices == 1 {
                assert_eq!(got, baseline, "concurrency {concurrency}");
            } else {
                // Sliced runs count only in-slice transactions, but every
                // figure of the unsliced run is reproduced.
                let sliced = (r.base.matches.len(), format!("{:?}", r.side_hits));
                let plain = {
                    let p = Fake::new(dataset());
                    let r = run(&p, &tokens, opts(w, 1, 1)).await.unwrap();
                    (r.base.matches.len(), format!("{:?}", r.side_hits))
                };
                assert_eq!(sliced, plain, "slices {slices}");
                // ...and are themselves independent of completion order.
                let mut q = Fake::new(dataset());
                q.delay_ms = |c, k, i| (c * 13 + u64::from(k) * 7 + i as u64) % 5;
                let again = run(&q, &tokens, opts(w, concurrency, slices))
                    .await
                    .unwrap();
                assert_eq!(dump(&again), got, "slices {slices} shuffled");
            }
        }
    }
}

// ---- boundedness ---------------------------------------------------------

#[tokio::test]
async fn in_flight_scans_never_exceed_concurrency_slices_included() {
    let tokens: Vec<u8> = (1..=8).collect();
    for concurrency in [1usize, 2, 3, 5] {
        let mut p = Fake::new(dataset());
        p.delay_ms = |_, _, _| 3;
        run(&p, &tokens, opts(window(1000, 5000), concurrency, 4))
            .await
            .unwrap();
        assert!(
            p.peak() <= concurrency,
            "peak {} > concurrency {concurrency}",
            p.peak()
        );
        assert_eq!(p.calls().len(), 8 * 4, "every slice of every token scanned");
        if concurrency > 1 {
            assert!(p.peak() > 1, "concurrency must actually be used");
        }
    }
    // Library default stays sequential.
    let mut p = Fake::new(dataset());
    p.delay_ms = |_, _, _| 1;
    let assets: Vec<AssetKey> = tokens.iter().map(|t| token(*t)).collect();
    run_solana_trade_intersect(
        &p,
        &assets,
        2,
        IntersectOptions::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(p.peak(), 1);
}

// ---- slices --------------------------------------------------------------

#[tokio::test]
async fn slices_are_disjoint_equal_and_cover_the_window() {
    let p = Fake::new(dataset());
    run(&p, &[1], opts(window(1000, 1011), 1, 4)).await.unwrap();
    let mut calls = p.calls();
    calls.sort();
    let ranges: Vec<(i64, i64)> = calls
        .iter()
        .map(|(_, g, l)| (g.unwrap(), l.unwrap()))
        .collect();
    // 11 seconds / 4 slices: 2,3,3,3 (integer split), contiguous, exact ends.
    assert_eq!(
        ranges,
        vec![(1000, 1002), (1002, 1005), (1005, 1008), (1008, 1011)]
    );

    // A window shorter than the slice count never makes empty slices.
    let p = Fake::new(dataset());
    run(&p, &[1], opts(window(1000, 1003), 1, 16))
        .await
        .unwrap();
    assert_eq!(p.calls().len(), 3);
}

#[tokio::test]
async fn a_truncated_slice_makes_the_token_truncated_and_is_named() {
    let mut p = Fake::new(dataset());
    // Slices of [1000, 5000) are 1000 s wide: the second starts at 2000.
    p.truncate_when_gte = Some(2000);
    let r = run(&p, &[1, 2], opts(window(1000, 5000), 4, 4))
        .await
        .unwrap();
    for t in &r.per_token {
        assert!(t.truncated);
        assert_eq!(t.truncated_slices.len(), 1);
        assert_eq!(t.truncated_slices[0].index, 1);
        assert_eq!(t.truncated_slices[0].of, 4);
        assert_eq!(
            (t.truncated_slices[0].since, t.truncated_slices[0].until),
            (2000, 3000)
        );
        assert_eq!(t.status, TokenScanStatus::Ok);
    }
    let reasons = r.incomplete_reasons().join("\n");
    assert!(reasons.contains("slice 2/4 [2000, 3000)"), "{reasons}");
    assert!(r.is_coverage_incomplete());
    assert!(r.base.coverage_truncated);
}

#[tokio::test]
async fn boundary_transactions_redelivered_to_neighbour_slices_count_once() {
    // A provider that widens every range by 400 s re-delivers neighbours'
    // transactions; one that ignores the range delivers everything to every
    // slice. Both must give the unsliced figures (idempotence, invariant 11).
    let w = window(1000, 5000);
    let tokens: Vec<u8> = (1..=8).collect();
    let plain = run(&Fake::new(dataset()), &tokens, opts(w, 1, 1))
        .await
        .unwrap();
    for (honor, widen) in [(true, 400), (false, 0)] {
        let mut p = Fake::new(dataset());
        p.honor_range = honor;
        p.widen = widen;
        let r = run(&p, &tokens, opts(w, 4, 4)).await.unwrap();
        assert_eq!(r.base.matches, plain.base.matches);
        assert_eq!(r.side_hits, plain.side_hits, "honor={honor}");
        for (a, b) in r.per_token.iter().zip(&plain.per_token) {
            assert_eq!(a.transactions_scanned, b.transactions_scanned);
            assert_eq!(a.transactions_in_window, b.transactions_in_window);
            assert_eq!(a.qualified_wallets, b.qualified_wallets);
            assert_eq!(a.qualified_buyers, b.qualified_buyers);
            assert_eq!(a.qualified_sellers, b.qualified_sellers);
            assert_eq!(a.diagnostics, b.diagnostics);
            assert_eq!(a.trade, b.trade);
            assert!(!a.truncated);
        }
    }
}

// ---- typed stop / cancellation -------------------------------------------

#[tokio::test]
async fn stop_cuts_in_flight_and_marks_unstarted_deterministically() {
    // Token 1 fails at once (budget); tokens 2 and 3 are in flight (slow:
    // 5 s per item) and must be cut; tokens 4..6 never start.
    let mut p = Fake::new(dataset());
    p.delay_ms = |_, _, _| 5_000;
    p.first_item_free = true;
    p.fail_delay_ms = 40;
    p.fail = Some(Box::new(|key, _| (key == 1).then(budget_error)));
    let tokens: Vec<u8> = (1..=6).collect();
    let began = Instant::now();
    let r = run(&p, &tokens, opts(AnalysisWindow::none(0), 3, 1))
        .await
        .unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "stop must be prompt"
    );

    let stop = ScanStop::BudgetExhausted { limit: 7 };
    assert_eq!(r.stop, Some(stop));
    assert_eq!(r.per_token.len(), 6, "N never shrinks");
    let kind = ScanFailureKind::BudgetExhausted { limit: 7 };
    for i in 0..3 {
        assert!(
            matches!(&r.per_token[i].status, TokenScanStatus::Failed { kind: k, .. } if *k == kind),
            "token {i}: {:?}",
            r.per_token[i].status
        );
    }
    match &r.per_token[1].status {
        TokenScanStatus::Failed { message, .. } => {
            assert!(message.contains("interrupted"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    for i in 3..6 {
        assert_eq!(
            r.per_token[i].status,
            TokenScanStatus::NotScanned { reason: stop }
        );
    }
    let started: Vec<u8> = p.calls().iter().map(|c| c.0).collect();
    assert_eq!(started.len(), 3, "tokens 4..6 issued nothing: {started:?}");
    assert!(r.is_coverage_incomplete());
}

#[tokio::test]
async fn stop_inside_a_sliced_token_names_the_unstarted_slices() {
    // 2 tokens x 4 slices, concurrency 2: slice 1/4 of token 1 fails at
    // once, slice 2/4 is cut in flight, slices 3-4 never start, token 2
    // never starts.
    let mut p = Fake::new(dataset());
    p.delay_ms = |_, _, _| 5_000;
    p.first_item_free = true;
    p.fail_delay_ms = 40;
    p.fail = Some(Box::new(|key, gte| {
        (key == 1 && gte == Some(1000)).then(budget_error)
    }));
    let r = run(&p, &[1, 2], opts(window(1000, 5000), 2, 4))
        .await
        .unwrap();
    let stop = ScanStop::BudgetExhausted { limit: 7 };
    assert_eq!(r.stop, Some(stop));
    assert_eq!(r.per_token.len(), 2);
    match &r.per_token[0].status {
        TokenScanStatus::Failed { kind, message } => {
            assert_eq!(*kind, ScanFailureKind::BudgetExhausted { limit: 7 });
            assert!(message.starts_with("slice 1/4"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        r.per_token[1].status,
        TokenScanStatus::NotScanned { reason: stop }
    );
    assert_eq!(p.calls().len(), 2);
}

#[tokio::test]
async fn caller_cancel_returns_promptly_and_omits_unstarted_tokens() {
    let mut p = Fake::new(dataset());
    p.delay_ms = |_, _, _| 5_000;
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        c2.cancel();
    });
    let assets: Vec<AssetKey> = (1..=6).map(token).collect();
    let began = Instant::now();
    let r = run_solana_trade_intersect(&p, &assets, 2, opts(AnalysisWindow::none(0), 2, 1), cancel)
        .await
        .unwrap();
    assert!(began.elapsed() < Duration::from_secs(2));
    assert!(r.cancelled);
    assert_eq!(r.base.input_token_count, 6);
    // Only the two in-flight tokens are reported; the rest never started.
    assert_eq!(r.per_token.len(), 2);
    assert_eq!(p.calls().len(), 2);
    assert!(r.is_coverage_incomplete());
}

// ---- wallet runs ---------------------------------------------------------

fn wallet_data() -> BTreeMap<u8, Vec<RawSolanaTransaction>> {
    let mut out = BTreeMap::new();
    let mut sig = 0u8;
    for w in 1..=9u8 {
        let txs = (0..u64::from(w % 4 + 1))
            .map(|i| {
                sig += 1;
                curve_tx(true, w, 50 + w, sig, 100 + u64::from(sig), 1_000 + i as i64)
            })
            .collect();
        out.insert(w, txs);
    }
    out
}

async fn stats(
    p: &Fake,
    wallets: &[u8],
    concurrency: usize,
) -> scout_engine::SolanaWalletStatsReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let ws: Vec<SolanaPubkey> = wallets.iter().map(|w| pk(*w)).collect();
    run_solana_wallet_stats_concurrent(
        p,
        &ws,
        &LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        &AnalysisWindow::none(0),
        concurrency,
        CancellationToken::new(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn wallet_cards_are_identical_in_input_order_for_any_concurrency() {
    let wallets: Vec<u8> = (1..=9).collect();
    let baseline = format!(
        "{:?}",
        stats(&Fake::new(wallet_data()), &wallets, 1).await.wallets
    );
    for concurrency in [2usize, 4, 9, 16] {
        let mut p = Fake::new(wallet_data());
        p.delay_ms = |c, k, i| (c * 7919 + u64::from(k) * 31 + i as u64) % 13;
        let r = stats(&p, &wallets, concurrency).await;
        assert_eq!(
            format!("{:?}", r.wallets),
            baseline,
            "concurrency {concurrency}"
        );
        assert!(p.peak() <= concurrency);
        assert_eq!(r.concurrency, concurrency);
    }
}

#[tokio::test]
async fn wallet_stop_cuts_in_flight_and_marks_unstarted_not_scanned() {
    let mut p = Fake::new(wallet_data());
    p.delay_ms = |_, _, _| 5_000;
    p.first_item_free = true;
    p.fail_delay_ms = 40;
    p.fail = Some(Box::new(|key, _| (key == 1).then(budget_error)));
    let wallets: Vec<u8> = (1..=6).collect();
    let began = Instant::now();
    let r = stats(&p, &wallets, 3).await;
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(r.stop, Some(ScanStop::BudgetExhausted { limit: 7 }));
    assert_eq!(r.wallets.len(), 6);
    for w in &r.wallets[..3] {
        assert_eq!(w.status, WalletScanStatus::Error);
        assert_eq!(
            w.failure,
            Some(ScanFailureKind::BudgetExhausted { limit: 7 })
        );
    }
    assert!(
        r.wallets[1]
            .error
            .as_deref()
            .unwrap()
            .contains("interrupted")
    );
    for w in &r.wallets[3..] {
        assert_eq!(w.status, WalletScanStatus::NotScanned);
        assert!(w.not_scanned.is_some());
    }
    assert_eq!(p.calls().len(), 3);
}
