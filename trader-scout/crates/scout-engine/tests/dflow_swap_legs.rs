//! ADR-015 amendment: DFlow Aggregator v4 (`DF1ow4ts...`) swap events as
//! route-swap leg evidence. Real live fixtures (2026-10-02 router wallets +
//! pump captures) through the real `HeliusProvider` decode path, plus a
//! per-hop reconciliation against the RAW token-account balance deltas of
//! the committed JSON (same method as `jupiter_swap_legs.rs`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ScanRequest, ScanTask};
use scout_core::RawSolanaInstruction;
use scout_core::{AddressBytes, AssetKey, RawPayload, RawSolanaTransaction, SolanaPubkey};
use scout_dex_solana::{
    DFLOW_V4_PROGRAM_ID_BYTES, DflowEventDecoder, DflowEventOutcome, DflowSwapLeg,
    JupiterEventDecoder, JupiterEventOutcome,
};
use scout_engine::{
    LedgerDecoders, LedgerOptions, RouteRejections, SolanaWalletLedgerReport,
    build_solana_wallet_ledger_venues, pump_amm_decoder, pump_bonding_curve_decoder,
    solana_mainnet_chain,
};
use scout_providers::HeliusProvider;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// `(instruction_index, leg)` of every DFlow swap event of a tx. Panics on a
/// malformed/unknown live DFlow event (none may exist in the fixtures).
fn dflow_events(tx: &RawSolanaTransaction) -> Vec<(u32, DflowSwapLeg)> {
    let dec = DflowEventDecoder::new();
    tx.instructions
        .iter()
        .filter_map(|ix| match dec.classify(ix) {
            DflowEventOutcome::Swap {
                leg,
                instruction_index,
            } => Some((instruction_index, leg)),
            DflowEventOutcome::Malformed { reason } => panic!("malformed live event: {reason}"),
            DflowEventOutcome::UnknownEvent { discriminator } => {
                panic!("unknown live DFlow event {discriminator:?}")
            }
            _ => None,
        })
        .collect()
}

const FIXTURES: [&str; 6] = [
    "router_wallet_tAwv_page_2026-10-02.json",
    "router_wallet_9oC3_page_2026-10-02.json",
    "pump_mint1_full.json",
    "pump_mint2_full.json",
    "pump_variants_live_2026-10-02.json",
    "pumpswap_variants_live_2026-10-02.json",
];

const ROUTER_WALLET_9OC3: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
const ROUTER_WALLET_TAWV: &str = "tAwv75TULEMoR5bU8sNgPrDM2d8gdagDUy4b3qo8VXY";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const DFLOW: &str = "DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH";
const PROVF: &str = "proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u";

async fn tx_by_prefix(fixture: &str, prefix: &str) -> RawSolanaTransaction {
    fixture_txs(fixture)
        .await
        .into_iter()
        .find(|t| tx_sig(t).starts_with(prefix))
        .unwrap_or_else(|| panic!("{prefix} not in {fixture}"))
}

// ---------------------------------------------------------------------
// Per-hop reconciliation against the RAW balances of the committed JSON.
// ---------------------------------------------------------------------

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

/// `account -> (mint, owner, delta)` over pre/post token balances; an
/// account absent before has pre 0, absent after has post 0.
fn token_account_deltas(
    tx: &serde_json::Value,
    keys: &[String],
) -> BTreeMap<String, (String, Option<String>, i128)> {
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
                keys[i].clone(),
                (
                    b["mint"].as_str().unwrap().to_owned(),
                    b["owner"].as_str().map(str::to_owned),
                    p1 - p0,
                ),
            )
        })
        .collect()
}

/// Per-hop result of the two exact checks.
#[derive(Debug, Clone, Copy, Default)]
struct HopCheck {
    /// The venue CPI instruction (program == the event's `amm`, invoked at
    /// the event's stack height before it) was found.
    cpi: bool,
    /// A token account of that CPI changed by exactly `+input_amount` (pool
    /// vault credit) or `-input_amount` (source debit) in the input mint.
    in_exact: bool,
    /// A token account of that CPI changed by exactly `-output_amount`
    /// (vault debit) or `+output_amount` (recipient credit) in the output mint.
    out_exact: bool,
}

/// One DFlow top-level instruction (one route): all its swap events.
struct RouteEvidence {
    sig: String,
    ok: bool,
    hops: Vec<(DflowSwapLeg, HopCheck)>,
    /// Mints that are both an input and an output of the route: their summed
    /// inputs and outputs must be equal (nothing created or lost between hops).
    intermediates_conserved: (usize, usize),
    /// Intermediates whose summed inputs and outputs differ by exactly one
    /// raw unit (rounding between hops; never more).
    intermediates_off_by_one: usize,
    /// `(token edge exact, token edge total, quote edge exact, quote edge total)`
    /// against the economic signer's owner-keyed deltas; a quote mint is USDC
    /// or wSOL.
    edges: (usize, usize, usize, usize),
    /// For each USDC edge: `(signer delta + route net) * 10_000 / route net`
    /// in bps (signed; the part of the route's USDC that is not the
    /// signer's, e.g. a fee routed to another owner).
    quote_gap_bps: Vec<i128>,
    /// `(exact, total)`: routes with a USDC `FeeEvent` whose signer USDC
    /// delta equals `-(route USDC net input + fee)` exactly.
    fee_explained: (usize, usize),
}

fn verify_fixture(name: &str) -> Vec<RouteEvidence> {
    let dec = DflowEventDecoder::new();
    let mut out = Vec::new();
    for tx in raw_fixture_txs(name) {
        let keys = account_keys(&tx);
        let deltas = token_account_deltas(&tx, &keys);
        let sig = tx["transaction"]["signatures"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let header = &tx["transaction"]["message"]["header"];
        let n_sig = usize::try_from(header["numRequiredSignatures"].as_u64().unwrap()).unwrap();
        let signers = &keys[..n_sig];
        let ok = tx["meta"]["err"].is_null();
        for group in tx["meta"]["innerInstructions"].as_array().unwrap() {
            let ixs = group["instructions"].as_array().unwrap();
            let mut used: BTreeMap<String, usize> = BTreeMap::new();
            let mut hops: Vec<(DflowSwapLeg, HopCheck)> = Vec::new();
            let mut fees: Vec<(String, i128)> = Vec::new();
            for (j, ix) in ixs.iter().enumerate() {
                let prog = &keys[usize::try_from(ix["programIdIndex"].as_u64().unwrap()).unwrap()];
                if prog != DFLOW {
                    continue;
                }
                let acc: Vec<SolanaPubkey> = ix["accounts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| pubkey(&keys[usize::try_from(a.as_u64().unwrap()).unwrap()]))
                    .collect();
                let raw = RawSolanaInstruction {
                    program_id: DFLOW_V4_PROGRAM_ID_BYTES,
                    accounts: acc,
                    data: bs58::decode(ix["data"].as_str().unwrap())
                        .into_vec()
                        .unwrap(),
                    instruction_index: 0,
                };
                let l = match dec.classify(&raw) {
                    DflowEventOutcome::Swap { leg, .. } => leg,
                    DflowEventOutcome::Fee(f) => {
                        fees.push((bs58::encode(f.mint).into_string(), i128::from(f.amount)));
                        continue;
                    }
                    _ => continue,
                };
                let height = ix["stackHeight"].as_u64();
                let amm = bs58::encode(l.amm).into_string();
                let cands: Vec<&serde_json::Value> = ixs[..j]
                    .iter()
                    .filter(|x| {
                        keys[usize::try_from(x["programIdIndex"].as_u64().unwrap()).unwrap()] == amm
                            && x["stackHeight"].as_u64() == height
                    })
                    .collect();
                let n = used.entry(amm.clone()).or_insert(0);
                let cpi = cands.get(*n).copied();
                *n += 1;
                let mut chk = HopCheck::default();
                if let Some(c) = cpi {
                    chk.cpi = true;
                    let (im, om) = (
                        bs58::encode(l.input_mint).into_string(),
                        bs58::encode(l.output_mint).into_string(),
                    );
                    for a in c["accounts"].as_array().unwrap() {
                        let key = &keys[usize::try_from(a.as_u64().unwrap()).unwrap()];
                        let Some((mint, _, d)) = deltas.get(key) else {
                            continue;
                        };
                        let (ia, oa) = (i128::from(l.input_amount), i128::from(l.output_amount));
                        if *mint == im && (*d == ia || *d == -ia) {
                            chk.in_exact = true;
                        }
                        if *mint == om && (*d == -oa || *d == oa) {
                            chk.out_exact = true;
                        }
                    }
                }
                hops.push((l, chk));
            }
            if hops.is_empty() {
                continue;
            }
            let legs: Vec<DflowSwapLeg> = hops.iter().map(|(l, _)| *l).collect();
            // Route-level checks.
            let ins: BTreeSet<SolanaPubkey> = legs.iter().map(|l| l.input_mint).collect();
            let outs: BTreeSet<SolanaPubkey> = legs.iter().map(|l| l.output_mint).collect();
            let mids: Vec<&SolanaPubkey> = ins.intersection(&outs).collect();
            let conserved = mids
                .iter()
                .filter(|m| {
                    let si: u128 = legs
                        .iter()
                        .filter(|l| l.input_mint == ***m)
                        .map(|l| u128::from(l.input_amount))
                        .sum();
                    let so: u128 = legs
                        .iter()
                        .filter(|l| l.output_mint == ***m)
                        .map(|l| u128::from(l.output_amount))
                        .sum();
                    si == so
                })
                .count();
            let sum_diff = |m: &SolanaPubkey| -> i128 {
                let si: i128 = legs
                    .iter()
                    .filter(|l| l.input_mint == *m)
                    .map(|l| i128::from(l.input_amount))
                    .sum();
                let so: i128 = legs
                    .iter()
                    .filter(|l| l.output_mint == *m)
                    .map(|l| i128::from(l.output_amount))
                    .sum();
                si - so
            };
            let off_by_one = mids.iter().filter(|m| sum_diff(m).abs() == 1).count();
            assert!(mids.iter().all(|m| sum_diff(m).abs() <= 1));
            let mut net: BTreeMap<String, i128> = BTreeMap::new();
            for l in &legs {
                *net.entry(bs58::encode(l.input_mint).into_string())
                    .or_default() += i128::from(l.input_amount);
                *net.entry(bs58::encode(l.output_mint).into_string())
                    .or_default() -= i128::from(l.output_amount);
            }
            let mut edges = (0, 0, 0, 0);
            let mut quote_gap_bps = Vec::new();
            let mut fee_explained = (0, 0);
            // The economic signer: the one whose deltas touch the route mints.
            let owner_delta = |s: &String, m: &String| -> i128 {
                deltas
                    .values()
                    .filter(|(mint, o, _)| mint == m && o.as_deref() == Some(s.as_str()))
                    .map(|(_, _, d)| *d)
                    .sum()
            };
            if let Some(w) = signers
                .iter()
                .find(|s| net.keys().any(|m| owner_delta(s, m) != 0))
            {
                for (m, v) in &net {
                    // Zero, or a one-unit rounding residue between hops.
                    if v.abs() <= 1 {
                        continue;
                    }
                    let d = owner_delta(w, m);
                    if m == USDC {
                        edges.3 += 1;
                        if d == -*v {
                            edges.2 += 1;
                        }
                        quote_gap_bps.push(((d + *v) * 10_000).checked_div(*v).unwrap());
                        let fee: i128 = fees
                            .iter()
                            .filter(|(mint, _)| mint == USDC)
                            .map(|(_, a)| *a)
                            .sum();
                        if fee > 0 {
                            fee_explained.1 += 1;
                            if d == -(*v + fee) {
                                fee_explained.0 += 1;
                            }
                        }
                    } else {
                        edges.1 += 1;
                        if d == -*v {
                            edges.0 += 1;
                        }
                    }
                }
            }
            out.push(RouteEvidence {
                sig: sig.clone(),
                ok,
                hops,
                intermediates_conserved: (conserved, mids.len()),
                intermediates_off_by_one: off_by_one,
                edges,
                quote_gap_bps,
                fee_explained,
            });
        }
    }
    out
}

/// Evidence table. Prints one row per DFlow route (run with `--nocapture`)
/// and pins the totals.
#[test]
fn verification_table_every_live_hop_reconciles() {
    let (mut routes, mut hops, mut cpi, mut inx, mut outx, mut any, mut both) =
        (0, 0, 0, 0, 0, 0, 0);
    let (mut mid_ok, mut mid_all, mut mid_off) = (0, 0, 0);
    let (mut te, mut tt, mut qe, mut qt) = (0, 0, 0, 0);
    let mut txs = BTreeSet::new();
    let mut fee_exact = (0, 0);
    let (mut failed, mut gap_min, mut gap_max) = (0, i128::MAX, i128::MIN);
    println!("sig       hops  per-hop (C=cpi I=in-exact O=out-exact)");
    for name in FIXTURES {
        for t in verify_fixture(name) {
            if !t.ok {
                // A reverted transaction's events are not executed swaps.
                failed += 1;
                continue;
            }
            routes += 1;
            txs.insert(t.sig.clone());
            let marks: Vec<String> = t
                .hops
                .iter()
                .map(|(_, c)| {
                    format!(
                        "{}{}{}",
                        if c.cpi { 'C' } else { '-' },
                        if c.in_exact { 'I' } else { '.' },
                        if c.out_exact { 'O' } else { '.' }
                    )
                })
                .collect();
            println!(
                "{}  {}  {}  mids {}/{} edges tok {}/{} usdc {}/{}",
                &t.sig[..8],
                t.hops.len(),
                marks.join(" "),
                t.intermediates_conserved.0,
                t.intermediates_conserved.1,
                t.edges.0,
                t.edges.1,
                t.edges.2,
                t.edges.3
            );
            fee_exact.0 += t.fee_explained.0;
            fee_exact.1 += t.fee_explained.1;
            if name.starts_with("router_wallet_") {
                for g in &t.quote_gap_bps {
                    gap_min = gap_min.min(*g);
                    gap_max = gap_max.max(*g);
                }
            }
            for (_, c) in &t.hops {
                hops += 1;
                cpi += usize::from(c.cpi);
                inx += usize::from(c.in_exact);
                outx += usize::from(c.out_exact);
                any += usize::from(c.in_exact || c.out_exact);
                both += usize::from(c.in_exact && c.out_exact);
            }
            mid_ok += t.intermediates_conserved.0;
            mid_all += t.intermediates_conserved.1;
            mid_off += t.intermediates_off_by_one;
            if name.starts_with("router_wallet_") {
                te += t.edges.0;
                tt += t.edges.1;
                qe += t.edges.2;
                qt += t.edges.3;
            }
        }
    }
    println!(
        "TOTAL txs {} routes {routes} hops {hops} cpi {cpi} in-exact {inx} out-exact {outx} any {any} both {both} \
         | USDC FeeEvent explains the signer's USDC edge exactly {}/{} \
         | failed-on-chain routes skipped {failed} | usdc gap bps [{gap_min}, {gap_max}] \
         | mids exact {mid_ok}/{mid_all} (off by one unit {mid_off}) | router-page edges: token {te}/{tt} usdc {qe}/{qt}",
        txs.len(),
        fee_exact.0,
        fee_exact.1
    );
    // Every hop of every live sample reconciles under at least one exact check.
    assert_eq!(any, hops);
    assert_eq!(cpi, hops);
    assert_eq!((txs.len(), routes, hops, inx, outx, both), EXPECT_TABLE);
    assert_eq!(fee_exact, EXPECT_FEE);
    assert_eq!((mid_ok, mid_all, mid_off), EXPECT_MIDS);
    assert_eq!(mid_ok + mid_off, mid_all);
    assert_eq!((te, tt, qe, qt), EXPECT_EDGES);
}

// ---------------------------------------------------------------------
// Golden decodes and ledger deltas on the two router pages.
// ---------------------------------------------------------------------

fn leg(amm: &str, im: &str, ia: u64, om: &str, oa: u64) -> DflowSwapLeg {
    DflowSwapLeg {
        amm: pubkey(amm),
        input_mint: pubkey(im),
        input_amount: ia,
        output_mint: pubkey(om),
        output_amount: oa,
    }
}

/// Golden values read from the committed live captures (the hop fields were
/// cross-checked against the transactions' own token balances in
/// `verification_table_every_live_hop_reconciles`).
#[tokio::test]
async fn golden_decode_of_live_events() {
    let sol = "So11111111111111111111111111111111111111112";
    let usdt = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
    let bison = "BiSoNHVpsVZW2F7rx2eQ59yQwKxzU5NvBcmKshCSUypi";
    let pumpswap = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
    // 3 hops: USDC -> SOL on two venues, then SOL -> token on the pump.fun
    // bonding-curve program (`amm` is a venue program id here). The SOL
    // intermediate is off by ONE lamport between the sides (2_869_986_579 in,
    // 2_066_414_862 + 803_571_718 = 2_869_986_580 out), reported by the table.
    let tx = tx_by_prefix("router_wallet_9oC3_page_2026-10-02.json", "3M5ZLsUB").await;
    let token = "7uQifGdUTZpx26rnEcBbdPNBNGWdxwCLUuUC9aC3aJxc";
    let got: Vec<DflowSwapLeg> = dflow_events(&tx).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        got,
        vec![
            leg(bison, USDC, 251_316_000, sol, 2_066_414_862),
            leg(
                "TessVdML9pBGgG9yGks7o4HewRaXVAMuoVj4x83GLQH",
                USDC,
                97_734_000,
                sol,
                803_571_718
            ),
            leg(
                "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",
                sol,
                2_869_986_579,
                token,
                29_242_235_997_941
            ),
        ]
    );
    // 4 swap events + a FeeEvent (88-byte instruction, USDC fee 899_820):
    // USDC -> USDT -> SOL -> token on two venues.
    let tx = tx_by_prefix("router_wallet_tAwv_page_2026-10-02.json", "5qpqXzqr").await;
    let got: Vec<DflowSwapLeg> = dflow_events(&tx).into_iter().map(|(_, l)| l).collect();
    let tok = "GAwhcphCqCv5bKHmCiN4VDdNWfbXJL4npmkc8L3Q9S9H";
    assert_eq!(
        got,
        vec![
            leg(
                "DRVSpZ2YUYYKgZP8XtLhAGtT1zYSCKzeHfb4DgRnrgqD",
                USDC,
                2_998_500_180,
                usdt,
                2_999_474_875
            ),
            leg(bison, usdt, 2_999_474_875, sol, 24_571_180_808),
            leg(pumpswap, sol, 12_547_810_067, tok, 419_865_501_404),
            leg(
                "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
                sol,
                12_023_370_741,
                tok,
                399_098_834_684
            ),
        ]
    );
    // The SOL hop output is split exactly between the two token venues.
    assert_eq!(12_547_810_067u64 + 12_023_370_741, 24_571_180_808);
    let fees: Vec<_> = tx
        .instructions
        .iter()
        .filter_map(|i| match DflowEventDecoder::new().classify(i) {
            DflowEventOutcome::Fee(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(fees.len(), 1);
    assert_eq!(fees[0].amount, 899_820);
    assert_eq!(bs58::encode(fees[0].mint).into_string(), USDC);
}

fn ledger(wallet: &str, txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    build_solana_wallet_ledger_venues(&pubkey(wallet), txs, &decoders, opts).unwrap()
}

fn without_program(
    txs: &[RawSolanaTransaction],
    program: SolanaPubkey,
) -> Vec<RawSolanaTransaction> {
    txs.iter()
        .map(|t| {
            let mut t = t.clone();
            t.instructions.retain(|i| i.program_id != program);
            t
        })
        .collect()
}

/// Successful, signed, route-shaped (one non-quote token + USDC/USDT moved,
/// owner-keyed) transactions of the wallet that are NOT booked. Returns
/// `(total, with a Jupiter event, via DFlow, via DFlow with a swap event,
/// via the `proVF4pM...` router)`.
fn unbooked_route_shaped(
    wallet: &str,
    txs: &[RawSolanaTransaction],
    booked: &BTreeSet<[u8; 64]>,
) -> (usize, usize, usize, usize, usize) {
    let w = pubkey(wallet);
    let quote: BTreeSet<SolanaPubkey> = [USDC, "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"]
        .into_iter()
        .map(pubkey)
        .collect();
    let (mut total, mut with_jupiter, mut df1, mut df1_ev, mut via_provf) = (0, 0, 0, 0, 0);
    for tx in txs {
        if !tx.execution.is_success() || !tx.signers.contains(&w) || booked.contains(&tx.signature)
        {
            continue;
        }
        let mut d: BTreeMap<SolanaPubkey, i128> = BTreeMap::new();
        for b in tx
            .token_balance_changes
            .iter()
            .filter(|b| b.owner == Some(w))
        {
            *d.entry(b.mint).or_default() +=
                i128::from(b.post_amount) - i128::from(b.pre_amount.unwrap_or(0));
        }
        d.retain(|_, v| *v != 0);
        let q = d.keys().filter(|m| quote.contains(*m)).count();
        let other = d.keys().filter(|m| !quote.contains(*m)).count();
        if q == 1 && other == 1 {
            total += 1;
            let progs: BTreeSet<String> = tx
                .instructions
                .iter()
                .map(|i| bs58::encode(i.program_id).into_string())
                .collect();
            via_provf += usize::from(progs.contains(PROVF));
            let jup = JupiterEventDecoder::new();
            with_jupiter += usize::from(
                tx.instructions
                    .iter()
                    .any(|i| matches!(jup.classify(i), JupiterEventOutcome::Swaps { .. })),
            );
            df1 += usize::from(
                tx.instructions
                    .iter()
                    .any(|i| i.program_id == DFLOW_V4_PROGRAM_ID_BYTES),
            );
            df1_ev += usize::from(!dflow_events(tx).is_empty());
        }
    }
    (total, with_jupiter, df1, df1_ev, via_provf)
}

#[tokio::test]
async fn dflow_legs_book_additional_route_swaps_on_the_router_pages() {
    for (name, wallet, base_swaps, with_swaps, dflow, only, unbooked) in [
        (
            "router_wallet_9oC3_page_2026-10-02.json",
            ROUTER_WALLET_9OC3,
            38,
            49,
            36,
            11,
            2,
        ),
        (
            "router_wallet_tAwv_page_2026-10-02.json",
            ROUTER_WALLET_TAWV,
            68,
            77,
            42,
            9,
            2,
        ),
    ] {
        let txs = fixture_txs(name).await;
        let base = ledger(wallet, &without_program(&txs, DFLOW_V4_PROGRAM_ID_BYTES));
        let with = ledger(wallet, &txs);
        let e = with.trades.route_swaps_by_evidence;
        let booked: BTreeSet<[u8; 64]> = with.route_swap_log.iter().map(|r| r.signature).collect();
        let (n, with_j, df1, df1_ev, via_provf) = unbooked_route_shaped(wallet, &txs, &booked);
        println!(
            "{name}: route swaps {} -> {} (dflow {} / dflow_only {}); route-shaped unbooked {n} \
             (with Jupiter event {with_j}, via DF1ow4 {df1}, with DFlow swap event {df1_ev}, \
             via proVF4pM {via_provf}); rejected {:?}; out-of-scope {} -> {}, continuity breaks {} -> {}",
            base.trades.route_swaps,
            with.trades.route_swaps,
            e.dflow,
            e.dflow_only,
            with.diagnostics.route_rejected,
            base.diagnostics.out_of_scope_token_movements,
            with.diagnostics.out_of_scope_token_movements,
            base.diagnostics.continuity_breaks,
            with.diagnostics.continuity_breaks
        );
        assert_eq!(base.trades.route_swaps, base_swaps, "{name}");
        assert_eq!(with.trades.route_swaps, with_swaps, "{name}");
        assert_eq!(base.trades.route_swaps_by_evidence.dflow, 0);
        assert_eq!((e.dflow, e.dflow_only), (dflow, only), "{name}");
        assert_eq!(
            e.dflow_only,
            with.trades.route_swaps - base.trades.route_swaps
        );
        assert_eq!(with.diagnostics.dflow_malformed_events, 0);
        assert_eq!(with.diagnostics.dflow_unknown_events, 0);
        assert_eq!(with.diagnostics.route_rejected, RouteRejections::default());
        // Every successful DFlow route of the page (a swap event whose wallet
        // is the signer) is booked: none is left unknown.
        for tx in txs
            .iter()
            .filter(|t| t.execution.is_success() && !dflow_events(t).is_empty())
        {
            assert!(booked.contains(&tx.signature), "{}", tx_sig(tx));
        }
        // What remains unbooked is not DFlow and not Jupiter: all of it
        // routes through the third router `proVF4pM...` (no events).
        assert_eq!((with_j, df1, df1_ev), (0, 0, 0), "{name}");
        assert_eq!((n, via_provf), (unbooked, unbooked), "{name}");
    }
}

/// (txs, routes, hops, in-exact, out-exact, both).
const EXPECT_TABLE: (usize, usize, usize, usize, usize, usize) = (86, 86, 248, 195, 223, 170);
/// (exact, total, off by one raw unit).
const EXPECT_MIDS: (usize, usize, usize) = (92, 93, 1);
/// Router pages: (token edge exact, total, USDC edge exact, total).
const EXPECT_EDGES: (usize, usize, usize, usize) = (78, 78, 0, 78);
/// (exact, total): USDC `FeeEvent` explains the signer's USDC edge. Informational
/// only (the rest are other platform fees / quote routing); FeeEvent is never a leg.
const EXPECT_FEE: (usize, usize) = (17, 44);
