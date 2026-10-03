//! ADR-013 section 2b: direct-venue swap events (Orca Whirlpool `Traded`,
//! Meteora DLMM `Swap`/`Swap2Evt`, Raydium CLMM/CPMM `SwapEvent`) as route-swap
//! leg evidence. Real live fixtures through the real `HeliusProvider` decode
//! path (which now carries the attribution-relevant log lines), reconciled
//! per event against the RAW token-account balance deltas of the committed
//! JSON, plus the effect on the two router-wallet ledgers.
//!
//! Two independent exactness criteria per leg:
//! - A (owner-keyed): the token accounts OWNED by the pool (Orca/CLMM/DLMM:
//!   the pool account; CPMM: the pool `authority`) changed by exactly
//!   `+input_amount` in the input mint and/or `-output_amount` in the output
//!   mint;
//! - B (named vaults): the vault accounts NAMED by the swap instruction
//!   changed by exactly those amounts, and their raw mints equal the leg's
//!   mints (this checks the mint resolution itself, including the v1
//!   Whirlpool/CLMM `swap` whose mints come from pool deltas).
//!
//! A venue is `FixtureVerified` only if every sample passes at least one
//! side of both criteria.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::type_complexity
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::{AddressBytes, AssetKey, RawPayload, RawSolanaTransaction, SolanaPubkey};
use scout_dex_solana::{
    DFLOW_V4_PROGRAM_ID_BYTES, JUPITER_V6_PROGRAM_ID_BYTES, LogAttribution, MintSource,
    OKX_DEX_ROUTER_PROGRAM_ID_BYTES, VariantVerification, VenueIssueKind, VenueKind, VenueLeg,
    attribute_program_data, scan_venue_events,
};
use scout_engine::{
    LedgerDecoders, LedgerOptions, SolanaWalletLedgerReport, build_solana_wallet_ledger_venues,
    pump_amm_decoder, pump_bonding_curve_decoder, solana_mainnet_chain,
};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[path = "support/venue_ablation.rs"]
mod venue_ablation;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/p0/measurements/fixtures")
        .join(name)
}

fn raw_fixture_txs(name: &str) -> Vec<serde_json::Value> {
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture_path(name)).unwrap()).unwrap();
    if let Some(pages) = fixture["pages"].as_array() {
        pages
            .iter()
            .flat_map(|p| p["data"].as_array().unwrap().clone())
            .collect()
    } else {
        fixture["result"]["data"].as_array().unwrap().clone()
    }
}

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}

async fn fixture_txs(name: &str) -> Vec<RawSolanaTransaction> {
    let data = raw_fixture_txs(name);
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
            .unwrap();
    let mut stream = provider.scan(
        ScanTask {
            request: ScanRequest::TokenMarketActivity {
                asset: AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana([10; 32])),
            },
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
    assert_eq!(out.len(), data.len());
    out
}

fn tx_sig(tx: &RawSolanaTransaction) -> String {
    bs58::encode(tx.signature).into_string()
}

const FIXTURES: [&str; 5] = [
    "router_wallet_tAwv_page_2026-10-02.json",
    "router_wallet_9oC3_page_2026-10-02.json",
    "pump_mint1_full.json",
    "pump_mint2_full.json",
    "pumpswap_variants_live_2026-10-02.json",
];

const ROUTER_WALLET_9OC3: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
const ROUTER_WALLET_TAWV: &str = "tAwv75TULEMoR5bU8sNgPrDM2d8gdagDUy4b3qo8VXY";

fn account_keys(tx: &serde_json::Value) -> Vec<String> {
    let strs = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
            .unwrap_or_default()
    };
    let mut keys = strs(&tx["transaction"]["message"]["accountKeys"]);
    keys.extend(strs(&tx["meta"]["loadedAddresses"]["writable"]));
    keys.extend(strs(&tx["meta"]["loadedAddresses"]["readonly"]));
    keys
}

/// `account -> (mint, owner, delta)`; a missing side is 0.
fn token_account_deltas(
    tx: &serde_json::Value,
) -> BTreeMap<SolanaPubkey, (SolanaPubkey, Option<SolanaPubkey>, i128)> {
    let keys = account_keys(tx);
    let amount = |b: &serde_json::Value| -> i128 {
        b["uiTokenAmount"]["amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let by_index = |field: &str| -> BTreeMap<usize, serde_json::Value> {
        tx["meta"][field]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    usize::try_from(b["accountIndex"].as_u64().unwrap()).unwrap(),
                    b.clone(),
                )
            })
            .collect()
    };
    let (pre, post) = (by_index("preTokenBalances"), by_index("postTokenBalances"));
    let idx: BTreeSet<usize> = pre.keys().chain(post.keys()).copied().collect();
    idx.into_iter()
        .map(|i| {
            let b = post.get(&i).or_else(|| pre.get(&i)).unwrap();
            let (p0, p1) = (
                pre.get(&i).map_or(0, amount),
                post.get(&i).map_or(0, amount),
            );
            (
                pubkey(&keys[i]),
                (
                    pubkey(b["mint"].as_str().unwrap()),
                    b["owner"].as_str().map(pubkey),
                    p1 - p0,
                ),
            )
        })
        .collect()
}

/// Per-leg result of the two criteria.
#[derive(Debug, Clone, Copy, Default)]
struct Check {
    a_in: bool,
    a_out: bool,
    b_in: bool,
    b_out: bool,
    /// The named vaults' raw mints equal the leg's mints.
    b_mints: bool,
    /// The vault accounts were found (instruction has them).
    b_found: bool,
}

/// `(input_vault, output_vault, pool_owner)` of a leg from its swap instruction.
fn vaults(tx: &RawSolanaTransaction, leg: &VenueLeg) -> (SolanaPubkey, SolanaPubkey, SolanaPubkey) {
    let ix = tx
        .instructions
        .iter()
        .find(|i| i.instruction_index == leg.instruction_index)
        .unwrap();
    let a = |i: usize| ix.accounts[i];
    match leg.venue {
        VenueKind::Whirlpool => {
            let a_to_b = ix.data[41] == 1;
            let (va, vb) = if ix.accounts.len() >= 15
                && ix.data.len() >= 43
                && ix.data[..8] == [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]
            {
                (a(8), a(10))
            } else {
                (a(4), a(6))
            };
            let (i, o) = if a_to_b { (va, vb) } else { (vb, va) };
            (i, o, leg.pool)
        }
        VenueKind::Dlmm => {
            // swap_for_y <=> the leg's input mint is token_x_mint (idx 6).
            let x_in = leg.input_mint == a(6);
            let (rx, ry) = (a(2), a(3));
            let (i, o) = if x_in { (rx, ry) } else { (ry, rx) };
            (i, o, leg.pool)
        }
        VenueKind::RaydiumClmm => (a(5), a(6), leg.pool),
        VenueKind::RaydiumCpmm => (a(6), a(7), a(1)),
    }
}

fn check(
    tx: &RawSolanaTransaction,
    deltas: &BTreeMap<SolanaPubkey, (SolanaPubkey, Option<SolanaPubkey>, i128)>,
    leg: &VenueLeg,
) -> Check {
    let (vin, vout, owner) = vaults(tx, leg);
    let (ia, oa) = (i128::from(leg.input_amount), i128::from(leg.output_amount));
    let mut owned: BTreeMap<SolanaPubkey, i128> = BTreeMap::new();
    for (mint, o, d) in deltas.values() {
        if *o == Some(owner) {
            *owned.entry(*mint).or_default() += d;
        }
    }
    let mut c = Check {
        a_in: owned.get(&leg.input_mint) == Some(&ia),
        a_out: owned.get(&leg.output_mint) == Some(&-oa),
        ..Check::default()
    };
    if let (Some((mi, _, di)), Some((mo, _, dout))) = (deltas.get(&vin), deltas.get(&vout)) {
        c.b_found = true;
        c.b_in = *di == ia;
        c.b_out = *dout == -oa;
        c.b_mints = *mi == leg.input_mint && *mo == leg.output_mint;
    }
    c
}

#[derive(Debug, Default, Clone, Copy)]
struct Row {
    n: usize,
    a_in: usize,
    a_out: usize,
    a_either: usize,
    b_in: usize,
    b_out: usize,
    b_either: usize,
    b_mints: usize,
    both_criteria_pass: usize,
}

struct Measured {
    table: BTreeMap<(VenueKind, &'static str, &'static str), Row>,
    legs: Vec<(String, VenueLeg)>,
    swap_txs: BTreeMap<VenueKind, BTreeSet<String>>,
    log_txs: usize,
    log_attributed: usize,
    log_truncated: usize,
    log_misaligned: usize,
    issues: Vec<String>,
}

fn source_label(s: MintSource) -> &'static str {
    match s {
        MintSource::InstructionAccounts => "instruction accounts",
        MintSource::EventFields => "event fields",
        MintSource::PoolVaultDeltas => "pool vault deltas",
    }
}

async fn measure() -> Measured {
    let mut m = Measured {
        table: BTreeMap::new(),
        legs: Vec::new(),
        swap_txs: BTreeMap::new(),
        log_txs: 0,
        log_attributed: 0,
        log_truncated: 0,
        log_misaligned: 0,
        issues: Vec::new(),
    };
    let mut seen: BTreeSet<[u8; 64]> = BTreeSet::new();
    for name in FIXTURES {
        let txs = fixture_txs(name).await;
        let raw = raw_fixture_txs(name);
        let raw_by_sig: BTreeMap<String, &serde_json::Value> = raw
            .iter()
            .map(|t| {
                (
                    t["transaction"]["signatures"][0]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    t,
                )
            })
            .collect();
        for tx in &txs {
            // The same transaction can appear in several fixtures: count once.
            if !tx.execution.is_success() || !seen.insert(tx.signature) {
                continue;
            }
            let sig = tx_sig(tx);
            let scan = scan_venue_events(tx);
            for i in &scan.issues {
                m.issues.push(format!(
                    "{sig} {} {:?} {} {}",
                    i.venue.label(),
                    i.kind,
                    i.name,
                    i.reason
                ));
            }
            if tx.instructions.iter().any(|i| {
                [
                    VenueKind::Whirlpool,
                    VenueKind::RaydiumClmm,
                    VenueKind::RaydiumCpmm,
                ]
                .iter()
                .any(|v| v.program_id() == i.program_id)
            }) {
                m.log_txs += 1;
                match attribute_program_data(tx) {
                    LogAttribution::Attributed { truncated, .. } => {
                        m.log_attributed += 1;
                        m.log_truncated += usize::from(truncated);
                    }
                    LogAttribution::Misaligned { .. } => m.log_misaligned += 1,
                    LogAttribution::NotObserved => panic!("provider dropped logs: {sig}"),
                }
            }
            let deltas = token_account_deltas(raw_by_sig[&sig]);
            for leg in scan.legs {
                let c = check(tx, &deltas, &leg);
                assert!(c.b_found, "{sig}: named vaults not found for {leg:?}");
                let r = m
                    .table
                    .entry((leg.venue, leg.event, source_label(leg.mint_source)))
                    .or_default();
                r.n += 1;
                r.a_in += usize::from(c.a_in);
                r.a_out += usize::from(c.a_out);
                r.a_either += usize::from(c.a_in || c.a_out);
                r.b_in += usize::from(c.b_in);
                r.b_out += usize::from(c.b_out);
                r.b_either += usize::from(c.b_in || c.b_out);
                r.b_mints += usize::from(c.b_mints);
                r.both_criteria_pass +=
                    usize::from((c.a_in || c.a_out) && (c.b_in || c.b_out) && c.b_mints);
                m.swap_txs.entry(leg.venue).or_default().insert(sig.clone());
                m.legs.push((sig.clone(), leg));
            }
        }
    }
    m
}

#[tokio::test]
async fn venue_events_reconcile_with_pool_and_vault_deltas() {
    let m = measure().await;
    println!("== evidence table (live fixtures, successful txs, each tx counted once) ==");
    println!(
        "{:<13} {:<9} {:<20} {:>4} {:>6} {:>6} {:>7} | {:>6} {:>6} {:>7} | {:>6} {:>5}",
        "venue",
        "event",
        "mints from",
        "n",
        "A-in",
        "A-out",
        "A-any",
        "B-in",
        "B-out",
        "B-any",
        "mints",
        "pass"
    );
    for ((venue, event, src), r) in &m.table {
        println!(
            "{:<13} {:<9} {:<20} {:>4} {:>6} {:>6} {:>7} | {:>6} {:>6} {:>7} | {:>6} {:>5}",
            venue.label(),
            event,
            src,
            r.n,
            r.a_in,
            r.a_out,
            r.a_either,
            r.b_in,
            r.b_out,
            r.b_either,
            r.b_mints,
            r.both_criteria_pass
        );
    }
    for v in VenueKind::ALL {
        println!(
            "{}: {} distinct txs with a leg",
            v.label(),
            m.swap_txs.get(&v).map_or(0, BTreeSet::len)
        );
    }
    println!(
        "logs: {} txs with a Whirlpool/CLMM/CPMM instruction: {} attributed ({} of them truncated), {} misaligned",
        m.log_txs, m.log_attributed, m.log_truncated, m.log_misaligned
    );
    // No event or swap instruction of the fixtures is malformed, unknown,
    // IdlOnly or unresolved: every swap instruction has its event(s).
    assert!(m.issues.is_empty(), "{:#?}", m.issues);
    assert_eq!(m.log_misaligned, 0);
    assert_eq!(m.log_txs, m.log_attributed);
    // Every leg passes at least one side of BOTH criteria and its mints
    // equal the named vaults' raw mints => FixtureVerified is earned.
    for ((venue, event, src), r) in &m.table {
        assert_eq!(
            r.both_criteria_pass,
            r.n,
            "{} {event} ({src}): not every sample passes",
            venue.label()
        );
        assert_eq!(r.b_mints, r.n);
    }
    for (_, leg) in &m.legs {
        assert_eq!(
            leg.verification,
            VariantVerification::FixtureVerified,
            "{leg:?}"
        );
    }
    // Sample counts (the numbers the ADR cites).
    let count = |v: VenueKind, e: &str| -> usize {
        m.table
            .iter()
            .filter(|((vv, ee, _), _)| *vv == v && *ee == e)
            .map(|(_, r)| r.n)
            .sum()
    };
    assert_eq!(count(VenueKind::Whirlpool, "Traded"), 20);
    assert_eq!(count(VenueKind::Dlmm, "Swap"), 111);
    assert_eq!(count(VenueKind::Dlmm, "Swap2Evt"), 111);
    assert_eq!(count(VenueKind::RaydiumClmm, "SwapEvent"), 19);
    assert_eq!(count(VenueKind::RaydiumCpmm, "SwapEvent"), 14);
    // Totals the docs cite: 275 events, input side exact 264, output side
    // exact 273 (criterion A and B agree), at least one side 275.
    let sum = |f: fn(&Row) -> usize| m.table.values().map(f).sum::<usize>();
    assert_eq!(sum(|r| r.n), 275);
    assert_eq!(
        (sum(|r| r.a_in), sum(|r| r.a_out), sum(|r| r.a_either)),
        (264, 273, 275)
    );
    assert_eq!(
        (sum(|r| r.b_in), sum(|r| r.b_out), sum(|r| r.b_either)),
        (264, 273, 275)
    );
    // Mint sources: v1 Whirlpool/CLMM swaps (13) resolve through pool deltas,
    // everything else from the instruction/event.
    let pool_delta_legs: usize = m
        .table
        .iter()
        .filter(|((_, _, s), _)| *s == "pool vault deltas")
        .map(|(_, r)| r.n)
        .sum();
    assert_eq!(pool_delta_legs, 13);
}

/// Independent re-derivation of the log attribution from the RAW JSON: the
/// per-venue-program multiset of `Program data:` payload lengths found by a
/// plain stack walk of `meta.logMessages` equals what the legs/events of the
/// scan saw.
#[tokio::test]
async fn log_attribution_equals_an_independent_stack_walk_of_the_raw_logs() {
    let (mut walked, mut attributed) = (BTreeMap::new(), BTreeMap::new());
    let log_venues = [
        VenueKind::Whirlpool,
        VenueKind::RaydiumClmm,
        VenueKind::RaydiumCpmm,
    ];
    for name in FIXTURES {
        let txs = fixture_txs(name).await;
        let raw = raw_fixture_txs(name);
        for (tx, r) in txs.iter().zip(&raw) {
            if !tx.execution.is_success() {
                continue;
            }
            let mut stack: Vec<String> = Vec::new();
            for line in r["meta"]["logMessages"].as_array().unwrap() {
                let line = line.as_str().unwrap();
                if line == "Log truncated" {
                    break;
                }
                let parts: Vec<&str> = line.split(' ').collect();
                if parts.len() >= 4 && parts[0] == "Program" && parts[2] == "invoke" {
                    stack.push(parts[1].to_owned());
                } else if parts.len() >= 3
                    && parts[0] == "Program"
                    && (parts[2] == "success" || parts[2].starts_with("failed"))
                {
                    stack.pop();
                } else if let Some(b64) = line.strip_prefix("Program data: ") {
                    let prog = pubkey(stack.last().unwrap());
                    if let Some(v) = log_venues.iter().find(|v| v.program_id() == prog) {
                        use base64::Engine as _;
                        let n = base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .unwrap()
                            .len();
                        *walked.entry((v.label(), n)).or_insert(0usize) += 1;
                    }
                }
            }
            if let LogAttribution::Attributed { events, .. } = attribute_program_data(tx) {
                for e in events {
                    let p = tx.instructions[e.position].program_id;
                    if let Some(v) = log_venues.iter().find(|v| v.program_id() == p) {
                        *attributed
                            .entry((v.label(), e.payload.len()))
                            .or_insert(0usize) += 1;
                    }
                }
            }
        }
    }
    println!("independent walk: {walked:?}");
    assert_eq!(walked, attributed);
    // Whirlpool 121, CLMM 221, CPMM 170 bytes (incl. discriminator).
    assert_eq!(walked.get(&("whirlpool", 121)), Some(&20));
    assert_eq!(walked.get(&("raydium_clmm", 221)), Some(&19));
    assert_eq!(walked.get(&("raydium_cpmm", 170)), Some(&14));
}

/// Real transactions, tampered: a swap whose events are no longer observable
/// or decodable is never a leg and is counted.
#[tokio::test]
async fn negative_goldens_on_real_transactions() {
    let txs = fixture_txs("router_wallet_9oC3_page_2026-10-02.json").await;
    let mut checked = 0;
    for tx in txs.iter().filter(|t| t.execution.is_success()) {
        let base = scan_venue_events(tx);
        let has_log_leg = base.legs.iter().any(|l| l.venue != VenueKind::Dlmm);
        if !has_log_leg {
            continue;
        }
        checked += 1;
        // 1. Logs not observed: the log-venue legs vanish, DLMM legs stay.
        let mut no_logs = tx.clone();
        no_logs.log_messages = None;
        let s = scan_venue_events(&no_logs);
        assert!(s.legs.iter().all(|l| l.venue == VenueKind::Dlmm));
        assert!(
            s.issues
                .iter()
                .any(|i| i.kind == VenueIssueKind::Unresolved && i.venue != VenueKind::Dlmm)
        );
        // 2. Truncated right at the start: nothing attributable.
        let mut cut = tx.clone();
        cut.log_messages = Some(vec!["Log truncated".to_owned()]);
        let s = scan_venue_events(&cut);
        assert!(s.legs.iter().all(|l| l.venue == VenueKind::Dlmm));
        // 3. A log that does not match the instruction list (one invoke
        // removed) misaligns the whole transaction: no log leg is guessed.
        let mut broken = tx.clone();
        let logs = broken.log_messages.as_mut().unwrap();
        let first_invoke = logs.iter().position(|l| l.contains(" invoke [")).unwrap();
        logs.remove(first_invoke);
        let s = scan_venue_events(&broken);
        assert!(
            s.legs.iter().all(|l| l.venue == VenueKind::Dlmm),
            "{}",
            tx_sig(tx)
        );
        // 4. A payload line one byte longer is malformed: no leg for that event.
        let mut longer = tx.clone();
        for line in longer.log_messages.as_mut().unwrap() {
            if let Some(b64) = line.strip_prefix("Program data: ") {
                use base64::Engine as _;
                let mut bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .unwrap();
                bytes.push(0);
                *line = format!(
                    "Program data: {}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                );
            }
        }
        let s = scan_venue_events(&longer);
        assert!(s.legs.iter().all(|l| l.venue == VenueKind::Dlmm));
        assert!(
            s.issues
                .iter()
                .any(|i| i.kind == VenueIssueKind::Malformed && i.venue != VenueKind::Dlmm)
        );
    }
    assert!(checked >= 10, "{checked}");
}

fn ledger(
    wallet: &str,
    txs: &[RawSolanaTransaction],
    venue_events: bool,
) -> SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
        okx_order_policy: scout_engine::default_okx_order_policy,
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    let txs = if venue_events {
        txs.to_vec()
    } else {
        venue_ablation::without_venue_events(txs)
    };
    build_solana_wallet_ledger_venues(&pubkey(wallet), &txs, &decoders, opts).unwrap()
}

#[tokio::test]
async fn venue_legs_on_the_router_pages_change_nothing_that_was_booked_and_book_nothing_new() {
    let aggregators: [SolanaPubkey; 3] = [
        JUPITER_V6_PROGRAM_ID_BYTES,
        DFLOW_V4_PROGRAM_ID_BYTES,
        OKX_DEX_ROUTER_PROGRAM_ID_BYTES,
    ];
    for (name, wallet, before_swaps, after_swaps) in [
        (
            "router_wallet_9oC3_page_2026-10-02.json",
            ROUTER_WALLET_9OC3,
            51,
            51,
        ),
        (
            "router_wallet_tAwv_page_2026-10-02.json",
            ROUTER_WALLET_TAWV,
            79,
            79,
        ),
    ] {
        let txs = fixture_txs(name).await;
        let before = ledger(wallet, &txs, false);
        let after = ledger(wallet, &txs, true);
        let e = after.trades.route_swaps_by_evidence;
        // Direct (non-aggregator) venue transactions of the page: a venue leg
        // in a tx without Jupiter/DFlow/OKX/PumpSwap/pump-curve instruction.
        let direct: Vec<String> = txs
            .iter()
            .filter(|t| {
                t.execution.is_success()
                    && !scan_venue_events(t).legs.is_empty()
                    && !t
                        .instructions
                        .iter()
                        .any(|i| aggregators.contains(&i.program_id))
            })
            .map(tx_sig)
            .collect();
        println!(
            "{name}: route swaps {} -> {}; evidence whirlpool {} dlmm {} clmm {} cpmm {} venue_only {}; \
             direct (non-aggregator) venue txs {}; unbooked route rejections {:?}; \
             venue diagnostics {:?}",
            before.trades.route_swaps,
            after.trades.route_swaps,
            e.whirlpool,
            e.dlmm,
            e.raydium_clmm,
            e.raydium_cpmm,
            e.venue_only,
            direct.len(),
            after.diagnostics.route_rejected,
            after.diagnostics.venue_events,
        );
        assert_eq!(before.trades.route_swaps, before_swaps, "{name}");
        assert_eq!(after.trades.route_swaps, after_swaps, "{name}");
        assert_eq!(before.trades.route_swaps_by_evidence.whirlpool, 0);
        assert!(after.diagnostics.venue_events.is_clean(), "{name}");
        assert_eq!(e.venue_only, 0, "{name}");
        // Venue evidence accompanies the aggregator evidence of routes.
        assert!(
            e.dlmm > 0 && e.whirlpool + e.raydium_clmm + e.raydium_cpmm > 0,
            "{name}"
        );
        // The booked trades are the same.
        assert_eq!(before.route_swap_log, after.route_swap_log, "{name}");
        assert!(direct.is_empty(), "{name}: {direct:?}");
    }
}

/// The only non-aggregator venue transactions of the committed fixtures: two
/// Raydium CLMM swaps routed by an unidentified program (`4w3DZU...`) in
/// `pump_mint2_full.json`. The venue leg is decoded (CLMM `SwapEvent`, exact
/// 213 bytes), but the signer is a bundle relayer that moves nothing: the
/// token/quote deltas belong to a NON-signing owner (`BM9CcyEr...`), so no
/// wallet is attributed (ADR-013 section 2a; P0.18's router-forward case stays
/// open). Nothing becomes bookable through the venue leg here.
#[tokio::test]
async fn third_party_clmm_bundles_have_a_venue_leg_but_no_attributable_signer() {
    let txs = fixture_txs("pump_mint2_full.json").await;
    let mut seen = 0;
    for tx in &txs {
        let scan = scan_venue_events(tx);
        if !tx.execution.is_success()
            || scan.legs.is_empty()
            || tx.instructions.iter().any(|i| {
                [
                    JUPITER_V6_PROGRAM_ID_BYTES,
                    DFLOW_V4_PROGRAM_ID_BYTES,
                    OKX_DEX_ROUTER_PROGRAM_ID_BYTES,
                ]
                .contains(&i.program_id)
            })
        {
            continue;
        }
        seen += 1;
        assert!(scan.issues.is_empty());
        let [leg] = scan.legs.as_slice() else {
            panic!("{:?}", scan.legs)
        };
        assert_eq!(leg.venue, VenueKind::RaydiumClmm);
        assert_eq!(leg.verification, VariantVerification::FixtureVerified);
        // Every signer (relayer or not): nothing is booked, before or after.
        for signer in &tx.signers {
            let w = bs58::encode(signer).into_string();
            let after = ledger(&w, &txs, true);
            assert_eq!(after.trades.route_swaps, 0, "{w}");
            assert_eq!(after.trades.route_swaps_by_evidence.venue_only, 0, "{w}");
        }
    }
    assert_eq!(seen, 2);
}

// ---------------------------------------------------------------------
// Synthetic direct trades: wallet W signs, the venue is the only evidence.
// ---------------------------------------------------------------------

mod synthetic {
    use super::*;
    use base64::Engine as _;
    use scout_core::{
        RawSolanaInstruction, SolanaExecutionStatus, SolanaNativeBalanceChange,
        SolanaTokenBalanceChange,
    };
    use scout_dex_solana::{
        DLMM_EVENT_AUTHORITY_BYTES, DLMM_SWAP2_DISCRIMINATOR, DLMM_SWAP2_EVENT_DISCRIMINATOR,
        EVENT_CPI_DISCRIMINATOR, RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR,
        RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR, WHIRLPOOL_SWAP_V2_DISCRIMINATOR,
        WHIRLPOOL_TRADED_DISCRIMINATOR,
    };
    use scout_engine::{USDC_MINT, VenueDiag};

    const W: u8 = 1;
    const TOKEN: u8 = 10;

    fn pk(b: u8) -> SolanaPubkey {
        [b; 32]
    }

    #[derive(Clone, Copy)]
    pub enum Kind {
        Dlmm,
        Whirlpool,
        CpmmBaseOutput,
    }

    fn base(sig: u8, buy: bool) -> RawSolanaTransaction {
        let usdc = pubkey(USDC_MINT);
        let bal = |mint, pre: u64, post: u64| SolanaTokenBalanceChange {
            mint,
            owner: Some(pk(W)),
            decimals: 6,
            pre_amount: Some(pre),
            post_amount: post,
            closed: false,
        };
        RawSolanaTransaction {
            block_time: Some(1_790_000_000 + i64::from(sig)),
            signature: [sig; 64],
            execution: SolanaExecutionStatus::Succeeded,
            slot: 100 + u64::from(sig),
            transaction_index: 0,
            instructions: vec![],
            token_balance_changes: if buy {
                vec![bal(pk(TOKEN), 0, 1_000), bal(usdc, 5_000_000, 0)]
            } else {
                vec![bal(pk(TOKEN), 1_000, 0), bal(usdc, 0, 5_000_000)]
            },
            fee_lamports: 5_000,
            fee_payer: pk(88),
            signers: vec![pk(88), pk(W)],
            native_balance_changes: vec![SolanaNativeBalanceChange {
                account: pk(88),
                pre_lamports: 1_000_000,
                post_lamports: 995_000,
            }],
            log_messages: None,
        }
    }

    fn logs(program: SolanaPubkey, payload: &[u8]) -> Vec<String> {
        let id = bs58::encode(program).into_string();
        vec![
            format!("Program {id} invoke [1]"),
            format!(
                "Program data: {}",
                base64::engine::general_purpose::STANDARD.encode(payload)
            ),
            format!("Program {id} success"),
        ]
    }

    pub fn tx(kind: Kind, sig: u8, buy: bool) -> RawSolanaTransaction {
        let usdc = pubkey(USDC_MINT);
        let (amount_in, amount_out) = if buy {
            (5_000_000u64, 1_000u64)
        } else {
            (1_000u64, 5_000_000u64)
        };
        let mut tx = base(sig, buy);
        let mut accounts: Vec<SolanaPubkey> = (0..16u8).map(|i| pk(100 + i)).collect();
        match kind {
            Kind::Dlmm => {
                let mut data = DLMM_SWAP2_DISCRIMINATOR.to_vec();
                data.resize(28, 0);
                accounts[0] = pk(0x60);
                accounts[6] = usdc;
                accounts[7] = pk(TOKEN);
                tx.instructions.push(RawSolanaInstruction {
                    program_id: VenueKind::Dlmm.program_id(),
                    accounts,
                    data,
                    instruction_index: 0,
                });
                let mut ev = EVENT_CPI_DISCRIMINATOR.to_vec();
                ev.extend(DLMM_SWAP2_EVENT_DISCRIMINATOR);
                ev.extend(pk(0x60));
                ev.extend(pk(W));
                ev.extend([0u8; 8]);
                ev.push(u8::from(buy));
                ev.extend([0u8; 16]);
                ev.extend(amount_in.to_le_bytes());
                ev.extend(0u64.to_le_bytes());
                ev.extend(amount_out.to_le_bytes());
                ev.extend([0u8; 32]);
                ev.extend([0u8, 0u8]);
                tx.instructions.push(RawSolanaInstruction {
                    program_id: VenueKind::Dlmm.program_id(),
                    accounts: vec![DLMM_EVENT_AUTHORITY_BYTES],
                    data: ev,
                    instruction_index: 1,
                });
            }
            Kind::Whirlpool => {
                let mut data = WHIRLPOOL_SWAP_V2_DISCRIMINATOR.to_vec();
                data.resize(43, 0);
                data[41] = u8::from(buy);
                accounts.truncate(15);
                accounts[4] = pk(0x61);
                accounts[5] = usdc;
                accounts[6] = pk(TOKEN);
                tx.instructions.push(RawSolanaInstruction {
                    program_id: VenueKind::Whirlpool.program_id(),
                    accounts,
                    data,
                    instruction_index: 0,
                });
                let mut ev = WHIRLPOOL_TRADED_DISCRIMINATOR.to_vec();
                ev.extend(pk(0x61));
                ev.push(u8::from(buy));
                ev.extend([0u8; 32]);
                for v in [amount_in, amount_out, 0, 0, 0, 0] {
                    ev.extend(v.to_le_bytes());
                }
                tx.log_messages = Some(logs(VenueKind::Whirlpool.program_id(), &ev));
            }
            Kind::CpmmBaseOutput => {
                let mut data = RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR.to_vec();
                data.resize(24, 0);
                accounts.truncate(13);
                accounts[3] = pk(0x62);
                let (im, om) = if buy {
                    (usdc, pk(TOKEN))
                } else {
                    (pk(TOKEN), usdc)
                };
                accounts[10] = im;
                accounts[11] = om;
                tx.instructions.push(RawSolanaInstruction {
                    program_id: VenueKind::RaydiumCpmm.program_id(),
                    accounts,
                    data,
                    instruction_index: 0,
                });
                let mut ev = RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR.to_vec();
                ev.extend(pk(0x62));
                for v in [0u64, 0, amount_in, amount_out, 0, 0] {
                    ev.extend(v.to_le_bytes());
                }
                ev.push(0); // base_input = false for swap_base_output
                ev.extend(im);
                ev.extend(om);
                ev.extend([0u8; 16]);
                ev.push(0);
                tx.log_messages = Some(logs(VenueKind::RaydiumCpmm.program_id(), &ev));
            }
        }
        tx
    }

    pub fn book(txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
        let curve = pump_bonding_curve_decoder().unwrap();
        let amm = pump_amm_decoder();
        let decoders = LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        };
        build_solana_wallet_ledger_venues(
            &pk(W),
            txs,
            &decoders,
            LedgerOptions {
                left_censoring: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn a_venue_event_alone_books_a_signer_route_swap_from_the_wallets_own_deltas() {
        let txs = vec![
            tx(Kind::Dlmm, 1, true),
            tx(Kind::Dlmm, 2, false),
            tx(Kind::Whirlpool, 3, true),
            tx(Kind::Whirlpool, 4, false),
        ];
        let r = book(&txs);
        let e = r.trades.route_swaps_by_evidence;
        assert_eq!(r.trades.route_swaps, 4);
        assert_eq!((e.dlmm, e.whirlpool, e.venue_only), (2, 2, 4));
        assert_eq!(
            (e.curve, e.pump_amm, e.jupiter, e.dflow, e.okx),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(r.trades.route_swaps_by_quote.usdc, 4);
        assert!(r.diagnostics.venue_events.is_clean());
        // Consideration comes from the wallet's own USDC delta, never the event.
        assert!(
            r.route_swap_log
                .iter()
                .all(|x| x.token_amount == 1_000 && x.quote_amount == 5_000_000)
        );
        // Without the events (ablation) nothing is booked.
        let off = book(&venue_ablation::without_venue_events(&txs));
        assert_eq!(off.trades.route_swaps, 0);
        // No decoded leg at all: the transactions are not even candidates.
        assert_eq!(off.diagnostics.route_rejected.no_verified_leg, 0);
        // The Whirlpool swaps lose their log lines: counted unresolved.
        assert_eq!(off.diagnostics.venue_events.whirlpool.unresolved, 2);
    }

    #[test]
    fn an_idl_only_venue_variant_is_decoded_counted_and_never_evidence() {
        let r = book(&[tx(Kind::CpmmBaseOutput, 1, true)]);
        assert_eq!(r.trades.route_swaps, 0);
        assert_eq!(r.diagnostics.route_rejected.no_verified_leg, 1);
        assert_eq!(
            r.diagnostics.venue_events.raydium_cpmm,
            VenueDiag {
                idl_only: 1,
                ..VenueDiag::default()
            }
        );
        assert_eq!(r.evidence_samples.len(), 1);
        assert_eq!(r.evidence_samples[0].kind.label(), "venue_unverified_event");
        assert_eq!(r.evidence_samples[0].program_name(), "raydium_cpmm");
    }

    #[test]
    fn a_pass_through_owner_rule_still_applies_to_venue_legs() {
        // The tx is signed by W, but another signer also moves tokens: the
        // wallet's deltas are still one token + one quote, and the venue leg
        // carries no owner, so there is nothing to veto; a wallet that does
        // NOT sign is never booked even with a venue leg.
        let mut t = tx(Kind::Dlmm, 1, true);
        t.signers = vec![pk(88)];
        let r = book(&[t]);
        assert_eq!(r.trades.route_swaps, 0);
        assert_eq!(r.diagnostics.route_rejected.wallet_not_signer, 1);
    }
}
