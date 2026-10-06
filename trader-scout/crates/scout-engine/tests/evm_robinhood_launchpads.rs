//! ADR-020 amendment 8 / ADR-009 style verification of the Robinhood Chain
//! launchpad CURVES (Pons V2 and Bags) against EVERY committed
//! `evm_robinhood_*.json` fixture (data-driven: an `evm-capture --chain
//! robinhood --swaps pons|bags|all` fixture with `curve_metadata` rows is picked
//! up by dropping it into the fixtures directory). With no admitted curve the
//! tests pass vacuously and say so.
//!
//! Promotion rule (as `evm_bsc_venues.rs`): a curve deployment may be
//! `FixtureVerified` ONLY with at least one admitted sample (n >= 1) and ALL
//! admitted samples exact (token side exactly, quote side exactly or, for the
//! native quote leg that logs cannot show, on the ADR-015/017 standard: never
//! better for the wallet than the event); any inexact sample fails the test
//! whatever the flag says. A passing run with n >= 1 for an `IdlOnly` row
//! prints `PROMOTABLE <venue> <factory>`.
//!
//! Admission is the gate's rule over the fixture's recorded `curve_metadata`
//! (pinned factory AND the factory's own record names the emitter for the
//! curve's token); only admitted curves are samples.
//!
//! Per (tx, curve) the equations (derived from the pinned sources; `buys` and
//! `sells` are sums over the curve's events in the tx):
//!
//! Pons V2 (`pons_PonsV2BondingCurve_44a3db91.sol.txt`), buy: the curve pulls
//! `received`, books `spent` (the event's `quoteIn`; `fee` and `tax` stay on the
//! curve until swept), sends `tokensOut` to `recipient`, refunds
//! `received - spent` to `msg.sender` (`CurveBuyRefunded`). Sell: the curve
//! pulls `tokensIn`, pays `quoteOut = gross - fee - tax` to `recipient`.
//!  - TOKEN side: curve net of its own token `= sells.tokensIn - buys.tokensOut`
//!    plus the named classes LAUNCH (`Transfer(0 -> curve)` of the token in
//!    the tx: the constructor mint), GRADUATION (`- CurveCompleted.tokenOut`:
//!    `graduate()` hands the tracked reserves to the factory) and BUYBACK
//!    (`- BuybackLocked.tokensLocked`).
//!  - QUOTE side, ERC-20 `pairToken`: curve net of the pair token `=
//!    buys.quoteIn - sells.quoteOut - (FeesSwept.protocolAmount +
//!    FeesSwept.creatorAmount) - CurveCompleted.quoteOut` (the buyback slice
//!    stays as reserve; the sweep inside `graduate()` pays protocol and creator
//!    out of the curve).
//!  - QUOTE side, native (`pairToken == 0`): invisible in logs. For a DIRECT
//!    buy (`tx.to == curve`, buyer == signer, single curve, buys only)
//!    `tx.value == buys.quoteIn + refunds` EXACTLY (`NativeValueMismatch`
//!    enforces `msg.value == quoteIn`); a value above that is a printed
//!    surcharge (accepted on the standard), below fails. Native sells stay
//!    unverified.
//!
//! Bags (ABI only: `bags_BagsBondingCurve_e55767f7.json`, sha256-asserted; no
//! source is pinned, so only the equations the ABI supports are used):
//! TOKEN side: curve net `= sells.tokensIn - buys.tokensOut` plus LAUNCH and
//! MIGRATION (`- Migrated.lpTokens`). QUOTE side (native; `buy` is payable):
//! direct buy `tx.value >= grossQuoteIn` (equal = exact; above = printed
//! surcharge; below fails). Sells unverified.
//!
//! DERIVED check (informational and a decoder guard): curve events of curves
//! that no fixture row admits (the older fixtures carry such events but no
//! `curve_metadata`) are checked on the token side only, the traded token
//! being the unique token whose curve net equals the event-derived value.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use alloy_primitives::{Address, B256, I256, U256, keccak256};
use scout_api::DecodeOutcome;
use scout_dex_evm::{
    AnchorRole, CurveFamily, CurveMetadata, CurveTrade, GateOutcome, LaunchpadSide, SwapVenue,
    SwapVenueGate, VENUE_DEPLOYMENTS, VenueVerification, decode_curve_trade,
    decode_pons_buy_refunded, decode_pons_curve_completed,
};
use scout_engine::{admit_recorded_curves, curve_venue, pinned_curve_factories};
use scout_evm::{ROBINHOOD, decode_erc20_transfer};
use scout_providers::evm_replay::EvmFixtureReplay;
use scout_providers::{CurveKind, CurveOnchainMetadata, EvmReceiptInfo, EvmRpcClient, EvmTxInfo};
use scout_rpc::{RpcClient, RpcEndpoint};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

const RH: u64 = 4663;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/p0/measurements/fixtures")
}

fn robinhood_fixtures() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(fixtures_dir()) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = dir
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("evm_robinhood_") && n.ends_with(".json"))
        })
        .collect();
    v.sort();
    v
}

fn hex_u64(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.strip_prefix("0x")?, 16).ok()
}

fn opt_addr(v: &Value) -> Option<Address> {
    v.as_str().and_then(|s| s.parse().ok())
}

struct Loaded {
    name: String,
    receipts: Vec<EvmReceiptInfo>,
    txs: BTreeMap<B256, EvmTxInfo>,
    recorded: Vec<CurveOnchainMetadata>,
    rpc: EvmRpcClient,
    _server: MockServer,
}

async fn load(path: &PathBuf) -> Loaded {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        fixture["chain_id"], RH,
        "{path:?} is not a Robinhood fixture"
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(EvmFixtureReplay::from_fixture(&fixture))
        .mount(&server)
        .await;
    let rpc = EvmRpcClient::new(
        RpcClient::new(RpcEndpoint::new(server.uri()), 10_000, 1).unwrap(),
        ROBINHOOD,
    );
    let mut receipts: BTreeMap<B256, EvmReceiptInfo> = BTreeMap::new();
    let mut txs: BTreeMap<B256, EvmTxInfo> = BTreeMap::new();
    for c in fixture["calls"].as_array().unwrap() {
        match c["method"].as_str() {
            Some("eth_getBlockReceipts") if !c["result"].is_null() => {
                let n = hex_u64(&c["params"][0]).expect("block number param");
                for r in rpc.block_receipts(n).await.unwrap() {
                    receipts.insert(r.tx_hash, r);
                }
            }
            Some("eth_getTransactionReceipt") if !c["result"].is_null() => {
                let h: B256 = c["params"][0].as_str().unwrap().parse().unwrap();
                receipts.insert(h, rpc.transaction_receipt(h).await.unwrap());
            }
            Some("eth_getTransactionByHash") if !c["result"].is_null() => {
                let h: B256 = c["params"][0].as_str().unwrap().parse().unwrap();
                txs.insert(h, rpc.transaction_by_hash(h).await.unwrap());
            }
            _ => {}
        }
    }
    let recorded = fixture
        .get("curve_metadata")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| CurveOnchainMetadata {
                    emitter: opt_addr(&r["emitter"]).expect("emitter"),
                    kind: CurveKind::from_label(r["kind"].as_str().unwrap())
                        .unwrap_or_else(|| panic!("kind {}", r["kind"])),
                    factory: opt_addr(&r["factory"]),
                    token: opt_addr(&r["token"]),
                    quote: opt_addr(&r["quote"]),
                    registered_curve: opt_addr(&r["registered_curve"]),
                })
                .collect()
        })
        .unwrap_or_default();
    Loaded {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        receipts: receipts.into_values().collect(),
        txs,
        recorded,
        rpc,
        _server: server,
    }
}

/// The account's per-token net ERC-20 flow in a transaction (to - from).
fn account_net(r: &EvmReceiptInfo, account: Address) -> BTreeMap<Address, I256> {
    let mut net: BTreeMap<Address, I256> = BTreeMap::new();
    for l in &r.logs {
        if let DecodeOutcome::Decoded(t) = decode_erc20_transfer(l) {
            let amt = I256::try_from(t.amount).unwrap();
            if t.to == account {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_add(amt).unwrap();
            }
            if t.from == account {
                let e = net.entry(t.token).or_insert(I256::ZERO);
                *e = e.checked_sub(amt).unwrap();
            }
        }
    }
    net
}

// --- event topics from the pinned sources (keccak at run time) ---------------

/// `(canonical signature)` of `event <name>(...)` in Solidity source text.
fn sol_event_sig(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("event {name}("))
        .unwrap_or_else(|| panic!("event {name} not in source"));
    let rest = &src[start + "event ".len() + name.len() + 1..];
    let inner = &rest[..rest.find(");").unwrap()];
    let types: Vec<&str> = inner
        .split(',')
        .map(|p| p.split_whitespace().next().unwrap())
        .collect();
    format!("{name}({})", types.join(","))
}

struct Topics {
    fees_swept: B256,
    buyback_locked: B256,
    bags_migrated: B256,
}

fn topics() -> Topics {
    let src =
        std::fs::read_to_string(fixtures_dir().join("pons_PonsV2BondingCurve_44a3db91.sol.txt"))
            .unwrap();
    let abi: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures_dir().join("bags_BagsBondingCurve_e55767f7.json"))
            .unwrap(),
    )
    .unwrap();
    let migrated = abi
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "event" && e["name"] == "Migrated")
        .unwrap();
    let sig = format!(
        "Migrated({})",
        migrated["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["type"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join(",")
    );
    Topics {
        fees_swept: keccak256(sol_event_sig(&src, "FeesSwept")),
        buyback_locked: keccak256(sol_event_sig(&src, "BuybackLocked")),
        bags_migrated: keccak256(sig),
    }
}

fn word_u256(data: &[u8], i: usize) -> U256 {
    U256::from_be_slice(&data[i * 32..i * 32 + 32])
}

fn i(x: U256) -> I256 {
    I256::try_from(x).unwrap()
}

// --- rows ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum QuoteForm {
    /// ERC-20 quote: the curve's net of the pair token equals the equation.
    TokenExact,
    /// ERC-20 quote that differs from the equation (fails).
    TokenMismatch { expected: I256, actual: I256 },
    /// Native, direct buy: `tx.value` equals the equation.
    NativeExact,
    /// Native, direct buy: `tx.value` above the event (printed, accepted).
    NativeSurcharge,
    /// Native, direct buy: `tx.value` BELOW the event (never-better violated: fails).
    NativeBelow,
    /// No claim (sell, router/bot account, several curves, no recorded quote).
    NotApplicable,
}

#[derive(Debug, Clone)]
struct Row {
    fixture: String,
    tx: B256,
    family: CurveFamily,
    curve: Address,
    /// Admitted by the gate (`false` = derived, informational).
    admitted: bool,
    token: Address,
    events: usize,
    account_is_signer: bool,
    receiver_differs: bool,
    expected_token: I256,
    curve_token_net: I256,
    launch: I256,
    graduated: I256,
    quote: QuoteForm,
    /// `(tx.value, equation value)` of a direct native buy.
    native_paid: Option<(U256, U256)>,
}

impl Row {
    fn token_exact(&self) -> bool {
        self.expected_token == self.curve_token_net
    }
    fn promotable(&self) -> bool {
        self.token_exact()
            && !matches!(
                self.quote,
                QuoteForm::TokenMismatch { .. } | QuoteForm::NativeBelow
            )
    }
}

/// Everything one (tx, curve) group needs, from the receipt only.
struct GroupFacts {
    buys_token: U256,
    sells_token: U256,
    buys_quote: U256,
    sells_quote: U256,
    gross_quote_in: U256,
    refunds: U256,
    completed_token: U256,
    completed_quote: U256,
    swept_quote: U256,
    locked_tokens: U256,
    migrated_tokens: U256,
}

fn group_facts(
    r: &EvmReceiptInfo,
    curve: Address,
    trades: &[CurveTrade],
    t: &Topics,
) -> GroupFacts {
    let mut f = GroupFacts {
        buys_token: U256::ZERO,
        sells_token: U256::ZERO,
        buys_quote: U256::ZERO,
        sells_quote: U256::ZERO,
        gross_quote_in: U256::ZERO,
        refunds: U256::ZERO,
        completed_token: U256::ZERO,
        completed_quote: U256::ZERO,
        swept_quote: U256::ZERO,
        locked_tokens: U256::ZERO,
        migrated_tokens: U256::ZERO,
    };
    for e in trades {
        match e.side {
            LaunchpadSide::Buy => {
                f.buys_token += e.token_amount;
                f.buys_quote += e.quote_amount;
                f.gross_quote_in += e.quote_amount;
            }
            LaunchpadSide::Sell => {
                f.sells_token += e.token_amount;
                f.sells_quote += e.quote_amount;
            }
        }
    }
    for log in r.logs.iter().filter(|l| l.address == curve) {
        if let DecodeOutcome::Decoded(x) = decode_pons_buy_refunded(log) {
            f.refunds += x.refund;
        }
        if let DecodeOutcome::Decoded(x) = decode_pons_curve_completed(log) {
            f.completed_token += x.token_out;
            f.completed_quote += x.quote_out;
        }
        let d = log.data.as_ref();
        match log.topics.first() {
            Some(x) if *x == t.fees_swept && d.len() == 96 => {
                f.swept_quote += word_u256(d, 0) + word_u256(d, 2);
            }
            Some(x) if *x == t.buyback_locked && d.len() == 64 => {
                f.locked_tokens += word_u256(d, 1);
            }
            Some(x) if *x == t.bags_migrated && d.len() == 128 && log.topics.len() == 4 => {
                f.migrated_tokens += word_u256(d, 1);
            }
            _ => {}
        }
    }
    f
}

/// Samples of one receipt. `gate` admits curves; `derive` also reports the
/// groups of un-admitted curves (token side only).
fn rows_of_receipt(
    fixture: &str,
    r: &EvmReceiptInfo,
    tx: Option<&EvmTxInfo>,
    gate: &SwapVenueGate,
    derive: bool,
    t: &Topics,
) -> Vec<Row> {
    // (curve) -> (admitted, trades)
    let mut groups: BTreeMap<Address, (bool, Vec<CurveTrade>)> = BTreeMap::new();
    for log in &r.logs {
        let DecodeOutcome::Decoded(trade) = decode_curve_trade(log) else {
            continue;
        };
        match gate.classify(log) {
            GateOutcome::Verified(v) if v.launchpad.is_some() => {
                groups
                    .entry(trade.curve)
                    .or_insert((true, Vec::new()))
                    .1
                    .push(trade);
            }
            GateOutcome::UngatedEmitter { .. } if derive => {
                groups
                    .entry(trade.curve)
                    .or_insert((false, Vec::new()))
                    .1
                    .push(trade);
            }
            GateOutcome::Verified(_) => panic!("curve event gated without launchpad evidence"),
            _ => {}
        }
    }
    let signer = tx.map(|t| t.from).or(r.from);
    let curves_in_tx = groups.len();
    let mut rows = Vec::new();
    for (curve, (admitted, trades)) in &groups {
        let family = trades[0].family;
        let facts = group_facts(r, *curve, trades, t);
        let net = account_net(r, *curve);
        // Expected curve net of `token` under the equation and its named classes.
        let launch_of = |token: Address| -> I256 {
            r.logs
                .iter()
                .filter_map(|l| match decode_erc20_transfer(l) {
                    DecodeOutcome::Decoded(x)
                        if x.token == token && x.from == Address::ZERO && x.to == *curve =>
                    {
                        Some(i(x.amount))
                    }
                    _ => None,
                })
                .fold(I256::ZERO, |a, b| a.checked_add(b).unwrap())
        };
        let graduated_tokens = facts.completed_token + facts.locked_tokens + facts.migrated_tokens;
        let expected_of = |token: Address| -> (I256, I256) {
            let launch = launch_of(token);
            let e = i(facts.sells_token)
                .checked_sub(i(facts.buys_token))
                .unwrap()
                .checked_add(launch)
                .unwrap()
                .checked_sub(i(graduated_tokens))
                .unwrap();
            (e, launch)
        };
        let (token, quote_cfg) = if *admitted {
            let id = gate
                .curve_identity(*curve)
                .expect("admitted curve has an identity");
            (id.token, id.quote)
        } else {
            // Derived: the unique token whose curve net equals the equation.
            let matching: Vec<Address> = net
                .keys()
                .copied()
                .filter(|tk| net[tk] == expected_of(*tk).0)
                .collect();
            let token = match matching.as_slice() {
                [one] => *one,
                // No (or several) match: report against the first moved token.
                _ => net.keys().next().copied().unwrap_or(Address::ZERO),
            };
            (token, None)
        };
        let (expected_token, launch) = expected_of(token);
        let curve_token_net = net.get(&token).copied().unwrap_or(I256::ZERO);
        let account_is_signer = trades.iter().all(|e| Some(e.account) == signer);
        let receiver_differs = trades.iter().any(|e| e.recipient != e.account);
        let direct = tx.is_some_and(|x| x.to == Some(*curve));
        let buys_only = trades.iter().all(|e| e.side == LaunchpadSide::Buy);
        let mut native_paid = None;
        // Native direct-buy claim (Bags is always native; Pons when pairToken == 0, or,
        // derived, when the curve moved no other ERC-20).
        let native_curve = match (family, quote_cfg, *admitted) {
            (CurveFamily::Bags, _, _) => true,
            (CurveFamily::PonsV2, Some(q), true) => q == Address::ZERO,
            (CurveFamily::PonsV2, _, false) => net.keys().all(|k| *k == token),
            (CurveFamily::PonsV2, None, true) => false,
        };
        let quote = if native_curve
            && direct
            && account_is_signer
            && !receiver_differs
            && buys_only
            && curves_in_tx == 1
        {
            let tx = tx.unwrap();
            let want = match family {
                CurveFamily::PonsV2 => facts.buys_quote + facts.refunds,
                CurveFamily::Bags => facts.gross_quote_in,
            };
            native_paid = Some((tx.value, want));
            match tx.value.cmp(&want) {
                std::cmp::Ordering::Equal => QuoteForm::NativeExact,
                std::cmp::Ordering::Greater => QuoteForm::NativeSurcharge,
                std::cmp::Ordering::Less => QuoteForm::NativeBelow,
            }
        } else if *admitted && family == CurveFamily::PonsV2 && !native_curve {
            let q = quote_cfg.expect("ERC-20 quote");
            let expected = i(facts.buys_quote)
                .checked_sub(i(facts.sells_quote))
                .unwrap()
                .checked_sub(i(facts.swept_quote))
                .unwrap()
                .checked_sub(i(facts.completed_quote))
                .unwrap();
            let actual = net.get(&q).copied().unwrap_or(I256::ZERO);
            if expected == actual {
                QuoteForm::TokenExact
            } else {
                QuoteForm::TokenMismatch { expected, actual }
            }
        } else {
            QuoteForm::NotApplicable
        };
        rows.push(Row {
            fixture: fixture.to_string(),
            tx: r.tx_hash,
            family,
            curve: *curve,
            admitted: *admitted,
            token,
            events: trades.len(),
            account_is_signer,
            receiver_differs,
            expected_token,
            curve_token_net,
            launch,
            graduated: i(graduated_tokens),
            quote,
            native_paid,
        });
    }
    rows
}

fn gate_for(l: &Loaded) -> SwapVenueGate {
    let mut gate = SwapVenueGate::new(RH);
    let report = admit_recorded_curves(&mut gate, &l.recorded);
    for (curve, why) in &report.refused {
        println!("  {} refused curve {curve:#x}: {why}", l.name);
    }
    gate
}

async fn all_rows(derive: bool) -> Vec<Row> {
    let t = topics();
    let mut rows = Vec::new();
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        let gate = gate_for(&l);
        for r in &l.receipts {
            rows.extend(rows_of_receipt(
                &l.name,
                r,
                l.txs.get(&r.tx_hash),
                &gate,
                derive,
                &t,
            ));
        }
    }
    rows
}

fn print_row(n: usize, r: &Row) {
    println!(
        "| {} | {} | `{:#x}` | {:?} `{:#x}` | token `{:#x}` | admitted {} | events {} | signer {} | receiver!=account {} | expected token net {} | curve net {} | launch {} | graduated {} | token exact {} | quote {:?} | paid/want {:?} |",
        n + 1,
        r.fixture,
        r.tx,
        r.family,
        r.curve,
        r.token,
        r.admitted,
        r.events,
        r.account_is_signer,
        r.receiver_differs,
        r.expected_token,
        r.curve_token_net,
        r.launch,
        r.graduated,
        r.token_exact(),
        r.quote,
        r.native_paid
    );
}

fn family_venue(f: CurveFamily) -> SwapVenue {
    match f {
        CurveFamily::PonsV2 => SwapVenue::PonsV2Curve,
        CurveFamily::Bags => SwapVenue::BagsCurve,
    }
}

// --- tests -----------------------------------------------------------------

#[tokio::test]
async fn curve_samples_match_the_curves_flows_and_promotion_needs_exact_evidence() {
    let fixtures = robinhood_fixtures();
    println!("evm_robinhood_* fixtures: {}", fixtures.len());
    let rows: Vec<Row> = all_rows(false).await;
    println!("admitted curve (tx, curve) samples: {}", rows.len());
    if rows.is_empty() {
        println!(
            "no curve is admitted by any committed fixture (no `curve_metadata` rows yet): \
             vacuous pass; capture with `evm-capture --chain robinhood --swaps pons|bags|all`"
        );
    }
    for (n, r) in rows.iter().enumerate() {
        print_row(n, r);
    }
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| !r.promotable())
        .map(|r| {
            format!(
                "{} tx {:#x} curve {:#x}: token expected {} vs {} (quote {:?}, paid/want {:?})",
                r.fixture,
                r.tx,
                r.curve,
                r.expected_token,
                r.curve_token_net,
                r.quote,
                r.native_paid
            )
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} curve sample(s) with an inexact token side or a quote side better for the wallet \
         than the event:\n{}",
        bad.len(),
        bad.join("\n")
    );
    let by = |f: &dyn Fn(&QuoteForm) -> bool| rows.iter().filter(|r| f(&r.quote)).count();
    println!(
        "quote forms: erc20-exact {} native-exact {} native-surcharge {} n/a {}",
        by(&|q| *q == QuoteForm::TokenExact),
        by(&|q| *q == QuoteForm::NativeExact),
        by(&|q| *q == QuoteForm::NativeSurcharge),
        by(&|q| *q == QuoteForm::NotApplicable),
    );
    println!(
        "samples whose event account is not tx.from (router/bot, never attributed): {}; \
         with recipient != account (never attributed): {}",
        rows.iter().filter(|r| !r.account_is_signer).count(),
        rows.iter().filter(|r| r.receiver_differs).count()
    );
    for d in VENUE_DEPLOYMENTS
        .iter()
        .filter(|d| d.chain_id == RH && d.venue.is_curve() && d.role == AnchorRole::PoolFactory)
    {
        let mine: Vec<&Row> = rows
            .iter()
            .filter(|r| family_venue(r.family) == d.venue)
            .collect();
        let n = mine.len();
        let unexplained = mine.iter().filter(|r| !r.promotable()).count();
        println!(
            "{} factory {:#x}: {n} sample(s), token side exact and quote never better for the \
             wallet, verification {}",
            d.venue.label(),
            d.anchor,
            d.verification.label()
        );
        if d.verification == VenueVerification::FixtureVerified {
            assert!(
                n >= 1 && unexplained == 0,
                "{} {:#x} is FixtureVerified without exact samples ({n} samples, {unexplained} not promotable)",
                d.venue.label(),
                d.anchor
            );
        } else if n >= 1 && unexplained == 0 {
            println!(
                "PROMOTABLE {} {:#x}: {n} sample(s); flip its `dep(` to `dep_fixture_verified(` in \
                 gate.rs and commit the fixture",
                d.venue.label(),
                d.anchor
            );
        } else if n == 0 {
            println!("no samples for {} yet: stays IdlOnly", d.venue.label());
        }
    }
}

/// Decoder guard on real data: curve events of curves no fixture admits are
/// checked on the token side (the curve's own ERC-20 net equals the event-derived
/// value for exactly one token). Informational for the venue flags.
#[tokio::test]
async fn derived_curve_events_match_the_curves_token_flow() {
    let rows: Vec<Row> = all_rows(true)
        .await
        .into_iter()
        .filter(|r| !r.admitted)
        .collect();
    println!(
        "derived (un-admitted) curve (tx, curve) groups: {}",
        rows.len()
    );
    for (n, r) in rows.iter().enumerate() {
        print_row(n, r);
    }
    let exact = rows.iter().filter(|r| r.token_exact()).count();
    println!("derived token side exact: {exact} of {}", rows.len());
    let bad: Vec<String> = rows
        .iter()
        .filter(|r| !r.token_exact())
        .map(|r| {
            format!(
                "{} tx {:#x} curve {:#x} ({:?}): expected {} vs curve net {}",
                r.fixture, r.tx, r.curve, r.family, r.expected_token, r.curve_token_net
            )
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} derived curve group(s) do not match the curve's token flow:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[tokio::test]
async fn recorded_curves_are_admitted_only_when_pinned_and_confirmed_by_the_factory() {
    let mut recorded_total = 0usize;
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        let mut gate = SwapVenueGate::new(RH);
        for m in &l.recorded {
            recorded_total += 1;
            let venue = curve_venue(m.kind);
            let pinned = pinned_curve_factories(RH, venue);
            let want = m.factory.is_some_and(|f| pinned.contains(&f))
                && m.token.is_some()
                && m.registered_curve == Some(m.emitter);
            let got = gate
                .admit_curve(
                    venue,
                    m.emitter,
                    &CurveMetadata {
                        factory: m.factory,
                        token: m.token,
                        quote: m.quote,
                        registered_curve: m.registered_curve,
                    },
                )
                .is_ok();
            assert_eq!(got, want, "{} {:#x}", l.name, m.emitter);
            if m.kind == CurveKind::PonsV2 {
                println!(
                    "  {} pons curve {:#x} factory {:?} quote {:?} admitted {got}",
                    l.name, m.emitter, m.factory, m.quote
                );
            }
        }
    }
    println!("recorded curve metadata rows: {recorded_total}");
}

#[tokio::test]
async fn recorded_eth_calls_replay_to_the_recorded_curve_metadata() {
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        for m in &l.recorded {
            let factories = pinned_curve_factories(RH, curve_venue(m.kind));
            let got = l
                .rpc
                .curve_metadata(m.emitter, m.kind, &factories, "latest")
                .await
                .unwrap();
            assert_eq!(&got, m, "{} {:#x}", l.name, m.emitter);
        }
    }
}

/// ADR-020 amendment 8: the launchpad event is evidence only; trader identity
/// and amounts come from the transaction (`tx.from` and its own deltas).
/// Runs the extraction over every admitted curve transaction of the fixtures
/// and prints the outcome counts (booked trades, remaining no-trade reasons,
/// router-account / other-recipient counters).
#[tokio::test]
async fn extraction_books_curve_trades_from_tx_from_whatever_the_event_account() {
    use scout_core::RawEvmTransaction;
    use scout_engine::{EvmExtractionConfig, EvmTxOutcome, extract_evm_trades};
    let mut total = scout_engine::EvmExtractionSummary::default();
    for path in robinhood_fixtures() {
        let l = load(&path).await;
        let gate = gate_for(&l);
        if l.recorded.is_empty() {
            continue;
        }
        let curve_txs: Vec<RawEvmTransaction> = l
            .receipts
            .iter()
            .filter(|r| {
                r.logs
                    .iter()
                    .any(|g| matches!(decode_curve_trade(g), DecodeOutcome::Decoded(_)))
            })
            .filter_map(|r| {
                let t = l.txs.get(&r.tx_hash)?;
                Some(RawEvmTransaction {
                    chain: ROBINHOOD.verified_chain_key(),
                    hash: r.tx_hash,
                    block_number: r.block_number,
                    transaction_index: r.transaction_index,
                    block_time: 1,
                    from: t.from,
                    to: t.to,
                    value: t.value,
                    status: r.status,
                    gas_used: r.gas_used,
                    effective_gas_price: r.effective_gas_price.unwrap_or_default(),
                    l1_fee: r.l1_fee,
                    logs: r.logs.clone(),
                    internal_transfers: None,
                    native_source: None,
                    native_balance_diff: None,
                })
            })
            .collect();
        let mut cfg = EvmExtractionConfig::new(ROBINHOOD, gate);
        cfg.quote_tokens = EvmExtractionConfig::for_profile(ROBINHOOD).quote_tokens;
        let (ex, sum) = extract_evm_trades(&curve_txs, &cfg, None, None);
        println!(
            "{}: txs {} trades {} unknown_consideration {} no_trade {:?} account_is_router {} recipient_other {}",
            l.name,
            sum.transactions,
            sum.trades,
            sum.unknown_consideration,
            sum.no_trade,
            sum.launchpad_account_is_router,
            sum.launchpad_recipient_other
        );
        for e in &ex {
            if let EvmTxOutcome::Trade(t) = &e.outcome {
                assert_eq!(
                    t.wallet,
                    curve_txs.iter().find(|x| x.hash == e.tx_hash).unwrap().from
                );
            }
        }
        total.transactions += sum.transactions;
        total.trades += sum.trades;
    }
    println!(
        "curve txs {} booked trades {}",
        total.transactions, total.trades
    );
}

/// Pons V1 launches Uniswap v3 pools (nothing to decode): the pinned V1 ABI
/// names the DEX factory per launch (`TokenLaunched.dexFactory`/`pool`), and a
/// pool of the official Robinhood Uniswap v3 factory is admitted by the
/// existing v3 admission (factory + CREATE2), a pool of any other DEX factory
/// is refused as before.
#[test]
fn pons_v1_tokens_trade_in_uniswap_v3_pools_the_existing_admission_covers() {
    let abi: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures_dir().join("pons_v1_factory_abi_44a3db91.json")).unwrap(),
    )
    .unwrap();
    let abi = abi.get("abi").unwrap_or(&abi);
    let launched = abi
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "event" && e["name"] == "TokenLaunched")
        .unwrap();
    let names: BTreeSet<&str> = launched["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert!(names.contains("dexFactory") && names.contains("pool"));
    let dex = abi
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "event" && e["name"] == "DexConfigAdded")
        .unwrap();
    assert!(
        dex["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["name"] == "factory")
    );
    // No V1-specific venue exists: V1 pools are Uniswap v3 pools.
    assert!(VENUE_DEPLOYMENTS.iter().all(|d| {
        d.chain_id != RH
            || !d.venue.is_curve()
            || d.anchor
                != "0xA5aAb3F0c6EeadF30Ef1D3Eb997108E976351feB"
                    .parse::<Address>()
                    .unwrap()
    }));
    let v3 = VENUE_DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == RH && d.venue == SwapVenue::UniswapV3)
        .unwrap();
    assert_eq!(v3.verification, VenueVerification::FixtureVerified);
}

// --- synthetic checks of the row builder (non-vacuous even without a fixture) ---

mod synthetic {
    use alloy_primitives::Bytes;
    use scout_core::{EvmTxStatus, RawEvmLog};
    use scout_dex_evm::{
        BAGS_TOKENS_BOUGHT_TOPIC0, PONS_V2_CURVE_BUY_REFUNDED_TOPIC0, PONS_V2_CURVE_BUY_TOPIC0,
        PONS_V2_CURVE_COMPLETED_TOPIC0, PONS_V2_CURVE_SELL_TOPIC0,
    };

    use super::*;

    const PONS_FACTORY: Address =
        alloy_primitives::address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e");
    const BAGS_FACTORY: Address =
        alloy_primitives::address!("e8Cc4431adF8b5A847C113EF0c6af9043219Cb37");
    const CURVE: Address = Address::repeat_byte(0xe1);
    const TOKEN: Address = Address::repeat_byte(0x70);
    const QUOTE: Address = Address::repeat_byte(0x55);
    const W: Address = Address::repeat_byte(0xa1);

    fn word(v: u64) -> [u8; 32] {
        U256::from(v).to_be_bytes::<32>()
    }

    fn log(addr: Address, topics: Vec<B256>, words: &[[u8; 32]], idx: u64) -> RawEvmLog {
        RawEvmLog {
            address: addr,
            topics,
            data: Bytes::from(words.concat()),
            block_number: 1,
            transaction_index: 0,
            log_index: idx,
        }
    }

    fn transfer(token: Address, from: Address, to: Address, v: u64, idx: u64) -> RawEvmLog {
        log(
            token,
            vec![scout_evm::TRANSFER_TOPIC0, from.into_word(), to.into_word()],
            &[word(v)],
            idx,
        )
    }

    fn buy(acct: Address, quote_in: u64, tokens_out: u64, idx: u64) -> RawEvmLog {
        log(
            CURVE,
            vec![PONS_V2_CURVE_BUY_TOPIC0, acct.into_word(), acct.into_word()],
            &[word(quote_in), word(tokens_out), word(3), word(1)],
            idx,
        )
    }

    fn receipt(logs: Vec<RawEvmLog>) -> EvmReceiptInfo {
        EvmReceiptInfo {
            tx_hash: B256::repeat_byte(1),
            block_number: 1,
            transaction_index: 0,
            status: EvmTxStatus::Success,
            gas_used: 1,
            effective_gas_price: None,
            l1_fee: None,
            logs,
            from: Some(W),
            to: Some(CURVE),
        }
    }

    fn tx(value: u64) -> EvmTxInfo {
        EvmTxInfo {
            hash: B256::repeat_byte(1),
            from: W,
            to: Some(CURVE),
            value: U256::from(value),
            block_number: 1,
            transaction_index: 0,
            gas_price: None,
            input_selector: None,
        }
    }

    fn gate(quote: Address) -> SwapVenueGate {
        let mut g = SwapVenueGate::new(RH);
        g.admit_curve(
            SwapVenue::PonsV2Curve,
            CURVE,
            &CurveMetadata {
                factory: Some(PONS_FACTORY),
                token: Some(TOKEN),
                quote: Some(quote),
                registered_curve: Some(CURVE),
            },
        )
        .unwrap();
        g
    }

    #[test]
    fn native_direct_buy_with_refund_is_exact_and_a_cheaper_value_fails() {
        let t = topics();
        let refunded = log(
            CURVE,
            vec![PONS_V2_CURVE_BUY_REFUNDED_TOPIC0, W.into_word()],
            &[word(20)],
            1,
        );
        let r = receipt(vec![
            transfer(TOKEN, CURVE, W, 500, 0),
            refunded,
            buy(W, 100, 500, 2),
        ]);
        // msg.value = spent 100 + refund 20.
        let rows = rows_of_receipt("syn", &r, Some(&tx(120)), &gate(Address::ZERO), false, &t);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].token_exact() && rows[0].quote == QuoteForm::NativeExact,
            "{:?}",
            rows[0]
        );
        let rows = rows_of_receipt("syn", &r, Some(&tx(130)), &gate(Address::ZERO), false, &t);
        assert_eq!(rows[0].quote, QuoteForm::NativeSurcharge);
        assert!(rows[0].promotable());
        let rows = rows_of_receipt("syn", &r, Some(&tx(110)), &gate(Address::ZERO), false, &t);
        assert_eq!(rows[0].quote, QuoteForm::NativeBelow);
        assert!(!rows[0].promotable());
        // A wrong token amount fails the token side.
        let r2 = receipt(vec![transfer(TOKEN, CURVE, W, 499, 0), buy(W, 100, 500, 1)]);
        let rows = rows_of_receipt("syn", &r2, Some(&tx(100)), &gate(Address::ZERO), false, &t);
        assert!(!rows[0].token_exact() && !rows[0].promotable());
    }

    #[test]
    fn erc20_quote_buy_sell_and_graduation_classes_are_exact() {
        let t = topics();
        // Sell: seller -> curve tokens, curve -> recipient quote 90 (quoteOut = gross - fee - tax).
        let sell = log(
            CURVE,
            vec![PONS_V2_CURVE_SELL_TOPIC0, W.into_word(), W.into_word()],
            &[word(7), word(90), word(8), word(2)],
            1,
        );
        let r = receipt(vec![
            transfer(TOKEN, W, CURVE, 7, 0),
            sell,
            transfer(QUOTE, CURVE, W, 90, 2),
        ]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(0)), &gate(QUOTE), false, &t);
        assert!(
            rows[0].token_exact() && rows[0].quote == QuoteForm::TokenExact,
            "{:?}",
            rows[0]
        );
        // Graduating buy: curve gets 100 quote, sends 500 tokens, then graduate()
        // pays protocol 4 + creator 6 (FeesSwept) and hands quoteOut 90 / tokenOut 300 on.
        let swept = log(CURVE, vec![t.fees_swept], &[word(4), word(0), word(6)], 2);
        let completed = log(
            CURVE,
            vec![PONS_V2_CURVE_COMPLETED_TOPIC0],
            &[Address::repeat_byte(9).into_word().0, word(90), word(300)],
            3,
        );
        let r = receipt(vec![
            transfer(QUOTE, W, CURVE, 100, 0),
            transfer(TOKEN, CURVE, W, 500, 1),
            buy(W, 100, 500, 1),
            transfer(QUOTE, CURVE, Address::repeat_byte(8), 10, 2),
            swept,
            transfer(QUOTE, CURVE, Address::repeat_byte(9), 90, 3),
            transfer(TOKEN, CURVE, Address::repeat_byte(9), 300, 3),
            completed,
        ]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(0)), &gate(QUOTE), false, &t);
        assert!(
            rows[0].token_exact() && rows[0].quote == QuoteForm::TokenExact,
            "{:?}",
            rows[0]
        );
        assert_eq!(rows[0].graduated, I256::try_from(300u64).unwrap());
        // Launch class: the constructor mint lands on the curve in the same tx.
        let r = receipt(vec![
            transfer(TOKEN, Address::ZERO, CURVE, 1_000, 0),
            buy(W, 100, 500, 1),
        ]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(100)), &gate(Address::ZERO), false, &t);
        // Curve keeps 1000 - 0 (the buy's transfer is missing here): not exact.
        assert!(!rows[0].token_exact());
        let r = receipt(vec![
            transfer(TOKEN, Address::ZERO, CURVE, 1_000, 0),
            transfer(TOKEN, CURVE, W, 500, 1),
            buy(W, 100, 500, 2),
        ]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(100)), &gate(Address::ZERO), false, &t);
        assert!(rows[0].token_exact() && rows[0].launch == I256::try_from(1_000u64).unwrap());
    }

    #[test]
    fn router_receiver_and_unadmitted_curves_are_labelled_and_derived_rows_find_the_token() {
        let t = topics();
        // Router buyer: account != signer, no native claim.
        let router = Address::repeat_byte(0x99);
        let r = receipt(vec![
            transfer(TOKEN, CURVE, router, 500, 0),
            buy(router, 100, 500, 1),
        ]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(100)), &gate(Address::ZERO), false, &t);
        assert!(!rows[0].account_is_signer && rows[0].quote == QuoteForm::NotApplicable);
        // Not admitted: nothing without `derive`, a token-side row with it.
        let empty = SwapVenueGate::new(RH);
        let r = receipt(vec![
            transfer(TOKEN, CURVE, W, 500, 0),
            transfer(QUOTE, W, CURVE, 100, 0),
            buy(W, 100, 500, 1),
        ]);
        assert!(rows_of_receipt("syn", &r, Some(&tx(0)), &empty, false, &t).is_empty());
        let rows = rows_of_receipt("syn", &r, Some(&tx(0)), &empty, true, &t);
        assert_eq!(
            (rows.len(), rows[0].admitted, rows[0].token),
            (1, false, TOKEN)
        );
        assert!(rows[0].token_exact());
        // Bags: native direct buy against grossQuoteIn.
        let bags_gate = {
            let mut g = SwapVenueGate::new(RH);
            g.admit_curve(
                SwapVenue::BagsCurve,
                CURVE,
                &CurveMetadata {
                    factory: Some(BAGS_FACTORY),
                    token: Some(TOKEN),
                    quote: None,
                    registered_curve: Some(CURVE),
                },
            )
            .unwrap();
            g
        };
        let bought = log(
            CURVE,
            vec![BAGS_TOKENS_BOUGHT_TOPIC0, W.into_word(), W.into_word()],
            &[
                word(100),
                word(97),
                word(500),
                word(3),
                word(1),
                word(1),
                word(0),
                word(1),
                word(1),
                word(1),
            ],
            1,
        );
        let r = receipt(vec![transfer(TOKEN, CURVE, W, 500, 0), bought]);
        let rows = rows_of_receipt("syn", &r, Some(&tx(100)), &bags_gate, false, &t);
        assert_eq!(rows[0].family, CurveFamily::Bags);
        assert!(
            rows[0].token_exact() && rows[0].quote == QuoteForm::NativeExact,
            "{:?}",
            rows[0]
        );
        let rows = rows_of_receipt("syn", &r, Some(&tx(99)), &bags_gate, false, &t);
        assert_eq!(rows[0].quote, QuoteForm::NativeBelow);
    }
}
