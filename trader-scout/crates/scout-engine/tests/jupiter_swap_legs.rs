//! ADR-015: Jupiter v6 swap legs as route-swap evidence. Real live
//! fixtures (2026-10-02 router wallets + pump captures) through the real
//! `HeliusProvider` decode path, plus a per-hop reconciliation against the
//! RAW token-account balance deltas of the committed JSON.
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
    JUPITER_V6_PROGRAM_ID_BYTES, JupiterEventDecoder, JupiterEventKind, JupiterEventOutcome,
    JupiterSwapLeg,
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

/// `(instruction_index, kind, legs)` of every Jupiter swap event of a tx.
fn jupiter_events(tx: &RawSolanaTransaction) -> Vec<(u32, JupiterEventKind, Vec<JupiterSwapLeg>)> {
    let dec = JupiterEventDecoder::new();
    tx.instructions
        .iter()
        .filter_map(|ix| match dec.classify(ix) {
            JupiterEventOutcome::Swaps {
                kind,
                legs,
                instruction_index,
            } => Some((instruction_index, kind, legs)),
            JupiterEventOutcome::Malformed { reason } => panic!("malformed live event: {reason}"),
            JupiterEventOutcome::UnknownEvent { discriminator } => {
                panic!("unknown live Jupiter event {discriminator:?}")
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
const DLMM: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const PUMPSWAP: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";

fn leg(amm: &str, im: &str, ia: u64, om: &str, oa: u64) -> JupiterSwapLeg {
    JupiterSwapLeg {
        amm: pubkey(amm),
        input_mint: pubkey(im),
        input_amount: ia,
        output_mint: pubkey(om),
        output_amount: oa,
    }
}

async fn tx_by_prefix(fixture: &str, prefix: &str) -> RawSolanaTransaction {
    fixture_txs(fixture)
        .await
        .into_iter()
        .find(|t| tx_sig(t).starts_with(prefix))
        .unwrap_or_else(|| panic!("{prefix} not in {fixture}"))
}

/// Golden values read from the committed live captures (the base58 hop
/// fields were cross-checked against the transactions' own token balances in
/// `verification_table_every_live_hop_reconciles`).
#[tokio::test]
async fn golden_decode_of_live_events() {
    // 2-hop route PumpSwap (token -> CARD) then Meteora DLMM (CARD -> USDC),
    // `SwapsEvent` (disc 982f4eebc0606e6a), 244-byte instruction.
    let tx = tx_by_prefix("router_wallet_9oC3_page_2026-10-02.json", "2kQpjwzR").await;
    let ev = jupiter_events(&tx);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].0, ev[0].1), (18, JupiterEventKind::SwapsEvent));
    let token = "8dBnKHwNYH3hz2fFTJczVwzBTpJFAMtc53k3uA4zpump";
    let card = "CARDSccUMFKoPRZxt5vt3ksUbxEFEcnZ3H2pd3dKxYjp";
    assert_eq!(
        ev[0].2,
        vec![
            leg(PUMPSWAP, token, 3_942_827_129_571, card, 9_102_061_949),
            leg(DLMM, card, 9_102_061_949, USDC, 2_488_536_041),
        ]
    );
    // 3-hop split route (CPMM -> DLMM + CLMM), 356-byte instruction.
    let tx = tx_by_prefix("router_wallet_9oC3_page_2026-10-02.json", "42fgKzug").await;
    let ev = jupiter_events(&tx);
    let a = "2eBJvNnn7guSjX1yuE7seuDY9pveDiHkfoyeBh2WZroS";
    let x = "Xsa62P5mvPszXL1krVUnU5ar38bBSVcWAB6fmPCo5Zu";
    assert_eq!(
        ev[0].2,
        vec![
            leg(
                "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
                a,
                23_532_060_992_177,
                x,
                52_260_483
            ),
            leg(DLMM, x, 15_322_773, USDC, 112_094_713),
            leg(
                "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
                x,
                36_937_710,
                USDC,
                270_126_619
            ),
        ]
    );
    // The split conserves the intermediate token exactly.
    assert_eq!(15_322_773 + 36_937_710, 52_260_483);
    // The one live IDL-layout `SwapEvent` (disc 40c6cde8260871e2, 128 bytes).
    let tx = tx_by_prefix("pump_mint2_full.json", "5twkEEg4").await;
    let ev = jupiter_events(&tx);
    assert_eq!((ev[0].0, ev[0].1), (16, JupiterEventKind::SwapEvent));
    assert_eq!(
        ev[0].2,
        vec![leg(
            PUMPSWAP,
            "GGf4EX9qbzxuboefDTEvqHdysHqtZSQC7Sahprjpump",
            22_219_948_498_623,
            "So11111111111111111111111111111111111111112",
            767_294_900
        )]
    );
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

struct TxEvidence {
    sig: String,
    kind: JupiterEventKind,
    ok: bool,
    hops: Vec<(JupiterSwapLeg, HopCheck)>,
    /// Mints that are both an input and an output of the route: their summed
    /// inputs and outputs must be equal (nothing created or lost between hops).
    intermediates_conserved: (usize, usize),
    /// `(token edge exact, token edge total, quote edge short, quote edge total)`
    /// against the signer's owner-keyed deltas.
    edges: (usize, usize, usize, usize),
}

fn verify_fixture(name: &str) -> Vec<TxEvidence> {
    let dec = JupiterEventDecoder::new();
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
            for (j, ix) in ixs.iter().enumerate() {
                let prog = &keys[usize::try_from(ix["programIdIndex"].as_u64().unwrap()).unwrap()];
                if *prog != scout_prog() {
                    continue;
                }
                let acc: Vec<SolanaPubkey> = ix["accounts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| pubkey(&keys[usize::try_from(a.as_u64().unwrap()).unwrap()]))
                    .collect();
                let raw = RawSolanaInstruction {
                    program_id: JUPITER_V6_PROGRAM_ID_BYTES,
                    accounts: acc,
                    data: bs58::decode(ix["data"].as_str().unwrap())
                        .into_vec()
                        .unwrap(),
                    instruction_index: 0,
                };
                let JupiterEventOutcome::Swaps { kind, legs, .. } = dec.classify(&raw) else {
                    continue;
                };
                let height = ix["stackHeight"].as_u64();
                let mut used: BTreeMap<String, usize> = BTreeMap::new();
                let mut hops = Vec::new();
                for l in &legs {
                    let amm = bs58::encode(l.amm).into_string();
                    let cands: Vec<&serde_json::Value> = ixs[..j]
                        .iter()
                        .filter(|x| {
                            keys[usize::try_from(x["programIdIndex"].as_u64().unwrap()).unwrap()]
                                == amm
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
                            let (ia, oa) =
                                (i128::from(l.input_amount), i128::from(l.output_amount));
                            if *mint == im && (*d == ia || *d == -ia) {
                                chk.in_exact = true;
                            }
                            if *mint == om && (*d == -oa || *d == oa) {
                                chk.out_exact = true;
                            }
                        }
                    }
                    hops.push((*l, chk));
                }
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
                let mut net: BTreeMap<String, i128> = BTreeMap::new();
                for l in &legs {
                    *net.entry(bs58::encode(l.input_mint).into_string())
                        .or_default() += i128::from(l.input_amount);
                    *net.entry(bs58::encode(l.output_mint).into_string())
                        .or_default() -= i128::from(l.output_amount);
                }
                let mut edges = (0, 0, 0, 0);
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
                        if *v == 0 {
                            continue;
                        }
                        let d = owner_delta(w, m);
                        if m == USDC {
                            edges.3 += 1;
                            // A platform fee is skimmed outside the route.
                            if d < -*v {
                                edges.2 += 1;
                            }
                        } else {
                            edges.1 += 1;
                            if d == -*v {
                                edges.0 += 1;
                            }
                        }
                    }
                }
                out.push(TxEvidence {
                    sig: sig.clone(),
                    kind,
                    ok,
                    hops,
                    intermediates_conserved: (conserved, mids.len()),
                    edges,
                });
            }
        }
    }
    out
}

fn scout_prog() -> String {
    bs58::encode(JUPITER_V6_PROGRAM_ID_BYTES).into_string()
}

/// ADR-015 evidence table. Prints one row per Jupiter transaction (run with
/// `--nocapture`) and pins the totals.
#[test]
fn verification_table_every_live_hop_reconciles() {
    let (mut txs, mut hops, mut cpi, mut inx, mut outx, mut any, mut both) = (0, 0, 0, 0, 0, 0, 0);
    let (mut mid_ok, mut mid_all) = (0, 0);
    let (mut te, mut tt, mut qs, mut qt) = (0, 0, 0, 0);
    let (mut swaps_event_hops, mut swaps_events_hops) = (0, 0);
    println!("sig      kind        hops  per-hop (C=cpi I=in-exact O=out-exact)");
    for name in FIXTURES {
        for t in verify_fixture(name) {
            assert!(t.ok, "{} failed on chain", t.sig);
            txs += 1;
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
                "{}  {:<10}  {}  {}  mids {}/{} edges tok {}/{} usdc-short {}/{}",
                &t.sig[..8],
                t.kind.name(),
                t.hops.len(),
                marks.join(" "),
                t.intermediates_conserved.0,
                t.intermediates_conserved.1,
                t.edges.0,
                t.edges.1,
                t.edges.2,
                t.edges.3
            );
            for (_, c) in &t.hops {
                hops += 1;
                cpi += usize::from(c.cpi);
                inx += usize::from(c.in_exact);
                outx += usize::from(c.out_exact);
                any += usize::from(c.in_exact || c.out_exact);
                both += usize::from(c.in_exact && c.out_exact);
                match t.kind {
                    JupiterEventKind::SwapEvent => swaps_event_hops += 1,
                    JupiterEventKind::SwapsEvent => swaps_events_hops += 1,
                }
            }
            mid_ok += t.intermediates_conserved.0;
            mid_all += t.intermediates_conserved.1;
            // The signer-edge check is defined for the router pages (a single
            // economic signer trading token/USDC); the pump captures are
            // other wallets' routes with SOL edges and are reported only.
            if name.starts_with("router_wallet_") {
                te += t.edges.0;
                tt += t.edges.1;
                qs += t.edges.2;
                qt += t.edges.3;
            }
        }
    }
    println!(
        "TOTAL txs {txs} hops {hops} cpi {cpi} in-exact {inx} out-exact {outx} any {any} both {both} \
         | SwapEvent hops {swaps_event_hops} SwapsEvent hops {swaps_events_hops} \
         | mids {mid_ok}/{mid_all} | token edges {te}/{tt} usdc edges short {qs}/{qt}"
    );
    // Every hop of every live sample reconciles under at least one exact check.
    assert_eq!((txs, hops, cpi, any), (37, 111, 111, 111));
    assert_eq!((inx, outx, both), (83, 99, 71));
    assert_eq!((swaps_event_hops, swaps_events_hops), (1, 110));
    assert_eq!((mid_ok, mid_all), (38, 38));
    // Signer-edge check (ADR-015 §1, second alternative): exact for every
    // token edge; the USDC edge is always short by a platform fee skimmed
    // outside the route, so it cannot be an exact check.
    assert_eq!((te, tt), (31, 31));
    assert_eq!((qs, qt), (31, 31));
}

// ---------------------------------------------------------------------
// Ledger deltas on the two router pages.
// ---------------------------------------------------------------------

fn ledger(wallet: &str, txs: &[RawSolanaTransaction]) -> SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
        okx_order_policy: okx_idl_only,
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    build_solana_wallet_ledger_venues(&pubkey(wallet), txs, &decoders, opts).unwrap()
}

fn without_jupiter(txs: &[RawSolanaTransaction]) -> Vec<RawSolanaTransaction> {
    txs.iter()
        .map(|t| {
            let mut t = t.clone();
            t.instructions
                .retain(|i| i.program_id != JUPITER_V6_PROGRAM_ID_BYTES);
            t
        })
        .collect()
}

/// Successful, signed, route-shaped (one non-quote token + USDC/USDT moved,
/// owner-keyed) transactions of the wallet that are NOT booked, split by
/// the venue/router programs they involve.
fn unbooked_route_shaped(
    wallet: &str,
    txs: &[RawSolanaTransaction],
    booked: &BTreeSet<[u8; 64]>,
) -> (usize, usize, usize) {
    let w = pubkey(wallet);
    let quote: BTreeSet<SolanaPubkey> = [USDC, "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"]
        .into_iter()
        .map(pubkey)
        .collect();
    let (mut total, mut with_jupiter, mut df1) = (0, 0, 0);
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
            with_jupiter += usize::from(!jupiter_events(tx).is_empty());
            df1 +=
                usize::from(tx.instructions.iter().any(|i| {
                    i.program_id == pubkey("DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH")
                }));
        }
    }
    (total, with_jupiter, df1)
}

/// Since the DFlow amendment (ledger/7) the base ledger (Jupiter instructions
/// removed) still books the DFlow-evidenced route swaps, so the Jupiter delta
/// is measured on top of them (it was 36 -> 38 and 62 -> 68 under ledger/6;
/// the remaining 2 unbooked route-shaped txs per page route through
/// `proVF4pM...`, see tests/dflow_swap_legs.rs).
#[tokio::test]
async fn jupiter_legs_book_additional_route_swaps_on_the_router_pages() {
    for (name, wallet, base_swaps, with_swaps, jup_txs, only, unbooked) in [
        (
            "router_wallet_9oC3_page_2026-10-02.json",
            ROUTER_WALLET_9OC3,
            47,
            49,
            4,
            2,
            2,
        ),
        (
            "router_wallet_tAwv_page_2026-10-02.json",
            ROUTER_WALLET_TAWV,
            71,
            77,
            27,
            6,
            2,
        ),
    ] {
        let txs = fixture_txs(name).await;
        let base = ledger(wallet, &without_jupiter(&txs));
        let with = ledger(wallet, &txs);
        assert_eq!(base.trades.route_swaps, base_swaps, "{name}");
        assert_eq!(with.trades.route_swaps, with_swaps, "{name}");
        let e = with.trades.route_swaps_by_evidence;
        assert_eq!(e.jupiter, jup_txs, "{name}");
        assert_eq!(e.jupiter_only, only, "{name}");
        assert_eq!(e.jupiter_only, with_swaps - base_swaps, "{name}");
        assert_eq!(base.trades.route_swaps_by_evidence.jupiter, 0);
        assert_eq!(with.diagnostics.jupiter_malformed_events, 0);
        assert_eq!(with.diagnostics.jupiter_unknown_events, 0);
        assert_eq!(with.diagnostics.route_rejected, RouteRejections::default());
        // Every Jupiter transaction of the page is booked: none is left unknown.
        let booked: BTreeSet<[u8; 64]> = with.route_swap_log.iter().map(|r| r.signature).collect();
        for tx in txs.iter().filter(|t| !jupiter_events(t).is_empty()) {
            assert!(booked.contains(&tx.signature), "{}", tx_sig(tx));
        }
        let (n, with_j, df1) = unbooked_route_shaped(wallet, &txs, &booked);
        println!(
            "{name}: route swaps {base_swaps} -> {with_swaps} ({only} Jupiter-only); \
             route-shaped unbooked {n} (with Jupiter event {with_j}, via DF1ow4 {df1}); \
             out-of-scope {} -> {}, continuity breaks {} -> {}",
            base.diagnostics.out_of_scope_token_movements,
            with.diagnostics.out_of_scope_token_movements,
            base.diagnostics.continuity_breaks,
            with.diagnostics.continuity_breaks
        );
        assert_eq!(with_j, 0);
        assert_eq!(n, unbooked, "{name}");
    }
}

/// These Jupiter/DFlow measurements are about their own evidence: OKX order
/// events are kept out (`IdlOnly`) so the historical numbers stay comparable;
/// the OKX effect is measured in `okx_router_legs.rs`.
fn okx_idl_only(_: scout_dex_solana::OkxOrderEventKind) -> scout_dex_solana::VariantVerification {
    scout_dex_solana::VariantVerification::IdlOnly
}
