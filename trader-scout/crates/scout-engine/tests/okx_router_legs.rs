//! OKX DEX Router (`proVF4pM...`) events as route-swap evidence (ADR-017
//! draft). Real live fixtures through the real `HeliusProvider` decode path,
//! plus an evidence table over the RAW JSON of every committed fixture that
//! contains the router:
//!
//! * every order event (`source_token_change` / `destination_token_change`)
//!   against the owner-keyed net token deltas of the owners and mints it
//!   names (exact comparison, per side);
//! * every per-hop `SwapEvent` against the venue CPI it follows (an account
//!   of that CPI moved by exactly the hop's `amount_in` / `amount_out`).
//!
//! Run with `--nocapture` for the per-event rows.
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
    OkxEventDecoder, OkxEventOutcome, OkxHop, OkxOrderEvent, OkxOrderEventKind,
};
use scout_engine::{
    LedgerDecoders, LedgerOptions, OkxOrderPolicy, RouteRejections, SolanaWalletLedgerReport,
    build_solana_wallet_ledger_venues, default_okx_order_policy, pump_amm_decoder,
    pump_bonding_curve_decoder, solana_mainnet_chain,
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

const ROUTER_WALLET_9OC3: &str = "9oC3XYAs2oeU39NeNFke8m3JGixMq7g8PfANsmsbKR8W";
const ROUTER_WALLET_TAWV: &str = "tAwv75TULEMoR5bU8sNgPrDM2d8gdagDUy4b3qo8VXY";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WSOL: &str = "So11111111111111111111111111111111111111112";
const PROVF: &str = "proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u";

/// Every committed fixture whose JSON contains the router program id.
const FIXTURES: [&str; 4] = [
    "router_wallet_9oC3_page_2026-10-02.json",
    "router_wallet_tAwv_page_2026-10-02.json",
    "pump_variants_live_2026-10-02.json",
    "pumpswap_variants_live_2026-10-02.json",
];

/// Programs that are never the venue of a hop.
const NOT_VENUES: [&str; 6] = [
    PROVF,
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    "11111111111111111111111111111111",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
    "ComputeBudget111111111111111111111111111111",
];

#[test]
fn every_fixture_that_contains_the_router_is_listed() {
    let dir = fixture_path("");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.ends_with(".json")
            && !name.contains("idl")
            && std::fs::read_to_string(&path).unwrap().contains(PROVF)
        {
            found.push(name);
        }
    }
    found.sort();
    let mut expect: Vec<String> = FIXTURES.iter().map(|s| (*s).to_owned()).collect();
    expect.sort();
    assert_eq!(found, expect);
}

// ---------------------------------------------------------------------
// Evidence over the RAW JSON.
// ---------------------------------------------------------------------

/// Owner-keyed net delta of `(owner, mint)`, `None` if the owner has no
/// token account of that mint in the transaction (native SOL, for wSOL).
fn owner_mint_delta(
    deltas: &BTreeMap<String, (String, Option<String>, i128)>,
    owner: &str,
    mint: &str,
) -> Option<i128> {
    let mut seen = false;
    let mut sum = 0i128;
    for (m, o, d) in deltas.values() {
        if m == mint && o.as_deref() == Some(owner) {
            seen = true;
            sum += d;
        }
    }
    seen.then_some(sum)
}

fn raw_instruction(
    ix: &serde_json::Value,
    keys: &[String],
    index: u32,
) -> (String, RawSolanaInstruction) {
    let prog = keys[usize::try_from(ix["programIdIndex"].as_u64().unwrap()).unwrap()].clone();
    let accounts = ix["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| pubkey(&keys[usize::try_from(a.as_u64().unwrap()).unwrap()]))
        .collect();
    let data = bs58::decode(ix["data"].as_str().unwrap())
        .into_vec()
        .unwrap();
    (
        prog.clone(),
        RawSolanaInstruction {
            program_id: pubkey(&prog),
            accounts,
            data,
            instruction_index: index,
        },
    )
}

/// How one side of an order event compares to the owner-keyed delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// `delta == +-change` exactly.
    Exact,
    /// Right sign, magnitude differs: the wallet paid more / received less
    /// than the event says by `gap` raw units (never the other way).
    Gap(i128),
    /// The owner has no token account of the mint in the transaction
    /// (native SOL leg): not checkable against token balances.
    NoTokenAccount,
    /// Wrong sign, or the event under-reports (`gap < 0`).
    Contradiction(i128),
}

fn side(delta: Option<i128>, change: u64, source: bool) -> Side {
    let Some(d) = delta else {
        return Side::NoTokenAccount;
    };
    let change = i128::from(change);
    // Source: the owner paid `change` (delta = -change). Destination: the
    // owner received `change` (delta = +change). `gap >= 0` means the owner
    // is worse off than the event reports.
    let gap = if source { -d - change } else { change - d };
    match gap {
        0 => Side::Exact,
        g if g > 0 => Side::Gap(g),
        g => Side::Contradiction(g),
    }
}

fn mint_class(mint: &str) -> &'static str {
    match mint {
        USDC | USDT => "stable",
        WSOL => "wsol",
        _ => "token",
    }
}

/// One order event with its reconciliation.
struct OrderRow {
    sig: String,
    ok: bool,
    event: OkxOrderEvent,
    src: Side,
    dst: Side,
    src_class: &'static str,
    dst_class: &'static str,
    /// `source_token_account_owner` signs the transaction.
    src_owner_signs: bool,
}

/// One per-hop event with its venue-CPI check.
struct HopRow {
    sig: String,
    ok: bool,
    hop: OkxHop,
    venue: Option<String>,
    in_exact: bool,
    out_exact: bool,
}

struct Evidence {
    txs: BTreeSet<String>,
    orders: Vec<OrderRow>,
    hops: Vec<HopRow>,
    unknown_or_malformed: usize,
}

fn collect_evidence() -> Evidence {
    let dec = OkxEventDecoder::new();
    let mut ev = Evidence {
        txs: BTreeSet::new(),
        orders: Vec::new(),
        hops: Vec::new(),
        unknown_or_malformed: 0,
    };
    for name in FIXTURES {
        for tx in raw_fixture_txs(name) {
            let keys = account_keys(&tx);
            let sig = tx["transaction"]["signatures"][0]
                .as_str()
                .unwrap()
                .to_owned();
            if !keys.iter().any(|k| k == PROVF) || !ev.txs.insert(sig.clone()) {
                continue;
            }
            let ok = tx["meta"]["err"].is_null();
            let deltas = token_account_deltas(&tx, &keys);
            let n_sig = usize::try_from(
                tx["transaction"]["message"]["header"]["numRequiredSignatures"]
                    .as_u64()
                    .unwrap(),
            )
            .unwrap();
            let signers = &keys[..n_sig];
            for group in tx["meta"]["innerInstructions"].as_array().unwrap() {
                let ixs = group["instructions"].as_array().unwrap();
                for (j, ix) in ixs.iter().enumerate() {
                    let (prog, raw) = raw_instruction(ix, &keys, 0);
                    if prog != PROVF {
                        continue;
                    }
                    match dec.classify(&raw) {
                        OkxEventOutcome::Order(e) => {
                            let (sm, dm) = (
                                bs58::encode(e.source_mint).into_string(),
                                bs58::encode(e.destination_mint).into_string(),
                            );
                            let (so, d_o) = (
                                bs58::encode(e.source_token_account_owner).into_string(),
                                bs58::encode(e.destination_token_account_owner).into_string(),
                            );
                            ev.orders.push(OrderRow {
                                sig: sig.clone(),
                                ok,
                                src: side(
                                    owner_mint_delta(&deltas, &so, &sm),
                                    e.source_token_change,
                                    true,
                                ),
                                dst: side(
                                    owner_mint_delta(&deltas, &d_o, &dm),
                                    e.destination_token_change,
                                    false,
                                ),
                                src_class: mint_class(&sm),
                                dst_class: mint_class(&dm),
                                src_owner_signs: signers.contains(&so),
                                event: e,
                            });
                        }
                        OkxEventOutcome::Hop(h) => {
                            // Venue CPI: nearest preceding instruction at the
                            // same stack height that is not the router, a
                            // token/system/ATA/compute program.
                            let height = ix["stackHeight"].as_u64();
                            let venue = ixs[..j].iter().rev().find(|x| {
                                x["stackHeight"].as_u64() == height
                                    && !NOT_VENUES.contains(
                                        &keys[usize::try_from(
                                            x["programIdIndex"].as_u64().unwrap(),
                                        )
                                        .unwrap()]
                                        .as_str(),
                                    )
                            });
                            let (mut in_exact, mut out_exact, mut venue_prog) =
                                (false, false, None);
                            if let Some(v) = venue {
                                venue_prog = Some(
                                    keys[usize::try_from(v["programIdIndex"].as_u64().unwrap())
                                        .unwrap()]
                                    .clone(),
                                );
                                let (ia, oa) = (i128::from(h.amount_in), i128::from(h.amount_out));
                                for a in v["accounts"].as_array().unwrap() {
                                    let key = &keys[usize::try_from(a.as_u64().unwrap()).unwrap()];
                                    let Some((_, _, d)) = deltas.get(key) else {
                                        continue;
                                    };
                                    in_exact |= *d == ia || *d == -ia;
                                    out_exact |= *d == oa || *d == -oa;
                                }
                            }
                            ev.hops.push(HopRow {
                                sig: sig.clone(),
                                ok,
                                hop: h,
                                venue: venue_prog,
                                in_exact,
                                out_exact,
                            });
                        }
                        OkxEventOutcome::NotMine | OkxEventOutcome::NotEventCpi => {}
                        OkxEventOutcome::UnknownEvent { .. }
                        | OkxEventOutcome::Malformed { .. } => {
                            ev.unknown_or_malformed += 1;
                        }
                    }
                }
            }
        }
    }
    ev
}

fn kind_stats<'a>(
    rows: impl Iterator<Item = &'a OrderRow>,
) -> BTreeMap<OkxOrderEventKind, Vec<&'a OrderRow>> {
    let mut m: BTreeMap<OkxOrderEventKind, Vec<&OrderRow>> = BTreeMap::new();
    for r in rows {
        m.entry(r.event.kind).or_default().push(r);
    }
    m
}

/// Evidence table (run with `--nocapture`): one row per order event, one
/// per hop, and the verdict per event variant.
#[test]
fn verification_table() {
    let ev = collect_evidence();
    println!(
        "== order events (src/dst: E exact, G<gap> wallet worse off by gap, N no token account, X contradiction)"
    );
    println!("sig       ok  kind                    owners     src(class)         dst(class)");
    let fmt = |s: Side| match s {
        Side::Exact => "E".to_owned(),
        Side::Gap(g) => format!("G{g}"),
        Side::NoTokenAccount => "N".to_owned(),
        Side::Contradiction(g) => format!("X{g}"),
    };
    for r in &ev.orders {
        let src = format!("{}({})", fmt(r.src), r.src_class);
        let dst = format!("{}({})", fmt(r.dst), r.dst_class);
        println!(
            "{}  {}  {:<24}{:<10} {:<18} {}",
            &r.sig[..8],
            if r.ok { "ok " } else { "ERR" },
            r.event.kind.name(),
            if r.event.single_owner() {
                "single"
            } else {
                "RECEIVER"
            },
            src,
            dst
        );
    }
    println!("== hops (venue = program of the CPI the event follows)");
    for h in &ev.hops {
        println!(
            "{}  {}  {:<20} in {} out {}  venue {}",
            &h.sig[..8],
            if h.ok { "ok " } else { "ERR" },
            h.hop.dex,
            if h.in_exact { "exact" } else { "-" },
            if h.out_exact { "exact" } else { "-" },
            h.venue.as_deref().unwrap_or("NONE")
        );
    }

    // Failed transactions: events are present but nothing executed; they are
    // excluded from every verification count below.
    let ok_orders: Vec<&OrderRow> = ev.orders.iter().filter(|r| r.ok).collect();
    let ok_hops: Vec<&HopRow> = ev.hops.iter().filter(|h| h.ok).collect();
    let failed_txs = ev
        .hops
        .iter()
        .filter(|h| !h.ok)
        .map(|h| h.sig.clone())
        .collect::<BTreeSet<_>>()
        .len();
    println!(
        "TOTAL txs {} (failed on chain {failed_txs}) | order events {} ok / {} failed | hops {} ok / {} failed | unknown or malformed {}",
        ev.txs.len(),
        ok_orders.len(),
        ev.orders.len() - ok_orders.len(),
        ok_hops.len(),
        ev.hops.len() - ok_hops.len(),
        ev.unknown_or_malformed
    );
    assert_eq!(ev.unknown_or_malformed, 0);

    // Per variant verdict.
    let stats = kind_stats(ok_orders.iter().copied());
    for kind in OkxOrderEventKind::ALL {
        let rows = stats.get(&kind).cloned().unwrap_or_default();
        let sides: Vec<(Side, &str)> = rows
            .iter()
            .flat_map(|r| [(r.src, r.src_class), (r.dst, r.dst_class)])
            .collect();
        let exact = sides.iter().filter(|(s, _)| *s == Side::Exact).count();
        let checkable = sides
            .iter()
            .filter(|(s, _)| *s != Side::NoTokenAccount)
            .count();
        let token_side: Vec<&(Side, &str)> = sides.iter().filter(|(_, c)| *c == "token").collect();
        let token_exact = token_side.iter().filter(|(s, _)| *s == Side::Exact).count();
        let quote: Vec<&(Side, &str)> = sides
            .iter()
            .filter(|(s, c)| *c != "token" && *s != Side::NoTokenAccount)
            .collect();
        let quote_exact = quote.iter().filter(|(s, _)| *s == Side::Exact).count();
        let all_exact = !rows.is_empty() && exact == checkable;
        let verdict = if rows.is_empty() {
            "IdlOnly (no sample)"
        } else if all_exact {
            "every sample exact"
        } else {
            "NOT every sample exact"
        };
        println!(
            "{:<32} samples {:>2} | sides exact {exact}/{checkable} | token side exact {token_exact}/{} | quote side exact {quote_exact}/{} | {verdict} -> {:?}",
            kind.name(),
            rows.len(),
            token_side.len(),
            quote.len(),
            kind.verification()
        );
        // The code may only say FixtureVerified when the table says so.
        assert_eq!(
            kind.verification() == scout_dex_solana::VariantVerification::FixtureVerified,
            all_exact,
            "{}",
            kind.name()
        );
    }

    // Pinned facts about the committed samples.
    assert_eq!(ev.txs.len(), EXPECT_TXS);
    assert_eq!(
        (ok_orders.len(), ev.orders.len() - ok_orders.len()),
        EXPECT_ORDERS
    );
    let only_fees2 = ok_orders
        .iter()
        .all(|r| r.event.kind == OkxOrderEventKind::SwapWithFeesCpiEvent2);
    assert!(
        only_fees2,
        "a new variant sample appeared: re-evaluate its verdict"
    );
    // No contradiction anywhere: the event never overstates what the owner
    // lost / underreports what the owner got.
    assert!(ok_orders.iter().all(|r| {
        !matches!(r.src, Side::Contradiction(_)) && !matches!(r.dst, Side::Contradiction(_))
    }));
    // Non-quote token sides are exact whenever checkable.
    let token_sides: Vec<Side> = ok_orders
        .iter()
        .flat_map(|r| [(r.src, r.src_class), (r.dst, r.dst_class)])
        .filter(|(_, c)| *c == "token")
        .map(|(s, _)| s)
        .collect();
    assert_eq!(token_sides.len(), EXPECT_TOKEN_SIDES);
    assert!(token_sides.iter().all(|s| *s == Side::Exact));
    // Quote sides: some exact, some off by a fee paid to other owners.
    let quote_sides: Vec<Side> = ok_orders
        .iter()
        .flat_map(|r| [(r.src, r.src_class), (r.dst, r.dst_class)])
        .filter(|(s, c)| *c != "token" && *s != Side::NoTokenAccount)
        .map(|(s, _)| s)
        .collect();
    let exact_q = quote_sides.iter().filter(|s| **s == Side::Exact).count();
    assert_eq!((exact_q, quote_sides.len()), EXPECT_QUOTE_SIDES);
    let native = ok_orders
        .iter()
        .flat_map(|r| [r.src, r.dst])
        .filter(|s| *s == Side::NoTokenAccount)
        .count();
    assert_eq!(native, EXPECT_NATIVE_SIDES);

    // Hops: every successful hop matches its venue CPI on at least one side
    // and each Dex variant always maps to one venue program.
    let mut venue_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for h in &ok_hops {
        assert!(h.venue.is_some(), "{} {}: no venue CPI", h.sig, h.hop.dex);
        venue_of
            .entry(h.hop.dex)
            .or_default()
            .insert(h.venue.as_deref().unwrap());
    }
    for (dex, v) in &venue_of {
        println!("dex {dex:<22} -> venue program(s) {v:?}");
        if *dex == "DynamicRouteV1" {
            // The router picks one candidate venue at run time (Auto mode):
            // here Tessera or BisonFi.
            assert!(
                v.iter()
                    .all(|p| p.starts_with("TessVdML") || p.starts_with("BiSoNHVp"))
            );
        } else {
            assert_eq!(v.len(), 1, "{dex} maps to several venue programs");
        }
    }
    let (inx, outx, any, both) = (
        ok_hops.iter().filter(|h| h.in_exact).count(),
        ok_hops.iter().filter(|h| h.out_exact).count(),
        ok_hops.iter().filter(|h| h.in_exact || h.out_exact).count(),
        ok_hops.iter().filter(|h| h.in_exact && h.out_exact).count(),
    );
    println!(
        "hops ok {} | in-exact {inx} out-exact {outx} any {any} both {both}",
        ok_hops.len()
    );
    assert_eq!(any, ok_hops.len());
    assert_eq!((ok_hops.len(), inx, outx, both), EXPECT_HOPS);
    // The same-name, same-discriminator hop events of a failed transaction
    // are not executed swaps.
    assert!(ev.hops.iter().filter(|h| !h.ok).all(|h| !h.sig.is_empty()));
}

/// Unique transactions holding the router.
const EXPECT_TXS: usize = 26;
/// (order events in successful txs, in failed txs).
const EXPECT_ORDERS: (usize, usize) = (24, 0);
const EXPECT_TOKEN_SIDES: usize = 24;
/// (exact, checkable) quote-asset sides.
const EXPECT_QUOTE_SIDES: (usize, usize) = (2, 22);
const EXPECT_NATIVE_SIDES: usize = 2;
/// (hops, in-exact, out-exact, both).
const EXPECT_HOPS: (usize, usize, usize, usize) = (77, 64, 67, 54);

#[test]
fn owner_roles_of_the_samples() {
    // Which owners the order events name relative to the signers.
    let ev = collect_evidence();
    let mut single = 0;
    let mut receiver = 0;
    let mut single_signer = 0;
    for r in ev.orders.iter().filter(|r| r.ok) {
        if r.event.single_owner() {
            single += 1;
            single_signer += usize::from(r.src_owner_signs);
        } else {
            receiver += 1;
        }
    }
    println!(
        "single-owner orders {single} (owner signs {single_signer}), with receiver {receiver}"
    );
    assert_eq!((single, single_signer, receiver), EXPECT_OWNER_ROLES);
}

/// (single-owner orders, of which the owner signs, orders with a distinct receiver).
const EXPECT_OWNER_ROLES: (usize, usize, usize) = (23, 23, 1);

// ---------------------------------------------------------------------
// Golden decodes and the ledger on the router pages.
// ---------------------------------------------------------------------

fn okx_events(tx: &RawSolanaTransaction) -> Vec<OkxEventOutcome> {
    let dec = OkxEventDecoder::new();
    tx.instructions
        .iter()
        .map(|ix| dec.classify(ix))
        .filter(|o| !matches!(o, OkxEventOutcome::NotMine | OkxEventOutcome::NotEventCpi))
        .collect()
}

#[tokio::test]
async fn golden_decode_of_live_events() {
    // 5 hops (Kipseli, TesseraV2, WhirlpoolV2, MeteoraDlmmSwap2,
    // RaydiumCpmmSwap) and one order: USDC -> token for the wallet itself.
    let tx = tx_by_prefix("router_wallet_9oC3_page_2026-10-02.json", "3K8mjYT6").await;
    let evs = okx_events(&tx);
    assert_eq!(evs.len(), 6);
    let dexes: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            OkxEventOutcome::Hop(h) => Some(h.dex),
            _ => None,
        })
        .collect();
    assert_eq!(
        dexes,
        [
            "Kipseli",
            "TesseraV2",
            "WhirlpoolV2",
            "MeteoraDlmmSwap2",
            "RaydiumCpmmSwap"
        ]
    );
    let OkxEventOutcome::Order(o) = evs.last().unwrap() else {
        panic!("last event is not the order");
    };
    assert_eq!(o.kind, OkxOrderEventKind::SwapWithFeesCpiEvent2);
    assert_eq!(bs58::encode(o.source_mint).into_string(), USDC);
    assert_eq!(
        bs58::encode(o.destination_mint).into_string(),
        "fJ5tNJQzyaGbx9oiz2MTdpP6ZRRgcaTM9AesKReCZWC"
    );
    assert_eq!(
        bs58::encode(o.source_token_account_owner).into_string(),
        ROUTER_WALLET_9OC3
    );
    assert!(o.single_owner());
    assert_eq!(
        (
            o.amount_in,
            o.source_token_change,
            o.destination_token_change
        ),
        (999_050_000, 999_050_000, 11_479_939_695_375)
    );
    // ADR-009 router-forward case: the order names the signer as source
    // owner and a DIFFERENT account as destination owner (swap with receiver).
    let tx = tx_by_prefix("pump_variants_live_2026-10-02.json", "bUh87USD").await;
    let OkxEventOutcome::Order(o) = okx_events(&tx)
        .into_iter()
        .find(|e| matches!(e, OkxEventOutcome::Order(_)))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(!o.single_owner());
    assert_eq!(bs58::encode(o.source_mint).into_string(), WSOL);
    assert_eq!(
        (o.amount_in, o.destination_token_change),
        (500_000, 2_928_987_168)
    );
    assert_eq!(
        bs58::encode(o.destination_token_account_owner).into_string(),
        "CNudZYFgpbT26fidsiNrWfHeGTBMMeVWqruZXsEkcUPc"
    );
}

fn ledger(
    wallet: &str,
    txs: &[RawSolanaTransaction],
    policy: OkxOrderPolicy,
) -> SolanaWalletLedgerReport {
    let curve = pump_bonding_curve_decoder().unwrap();
    let amm = pump_amm_decoder();
    let decoders = LedgerDecoders {
        curve: &curve,
        amm: Some(&amm),
        okx_order_policy: policy,
    };
    let opts = LedgerOptions {
        left_censoring: true,
    };
    build_solana_wallet_ledger_venues(&pubkey(wallet), txs, &decoders, opts).unwrap()
}

fn all_verified(_: OkxOrderEventKind) -> scout_dex_solana::VariantVerification {
    scout_dex_solana::VariantVerification::FixtureVerified
}

/// Successful, signed, route-shaped (one non-quote token + USDC/USDT moved,
/// owner-keyed) transactions of the wallet that are not booked, as signature
/// prefixes, plus how many of them hold router events.
fn unbooked_route_shaped(
    wallet: &str,
    txs: &[RawSolanaTransaction],
    booked: &BTreeSet<[u8; 64]>,
) -> Vec<(String, bool)> {
    let w = pubkey(wallet);
    let quote: BTreeSet<SolanaPubkey> = [USDC, USDT].into_iter().map(pubkey).collect();
    let mut out = Vec::new();
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
            let via_router = tx
                .instructions
                .iter()
                .any(|i| i.program_id == scout_dex_solana::OKX_DEX_ROUTER_PROGRAM_ID_BYTES);
            out.push((tx_sig(tx)[..8].to_owned(), via_router));
        }
    }
    out
}

/// Router pages: the production status keeps every order event `IdlOnly`
/// (no change to booking); the injected `FixtureVerified` status is the
/// what-if that shows what promotion would book.
#[tokio::test]
async fn router_pages_default_and_promoted_status() {
    // (page, wallet, route swaps now, order events of the page, route swaps if
    // promoted, unbooked route-shaped now / if promoted)
    for (name, wallet, swaps, orders, promoted_swaps, unbooked) in [
        (
            "router_wallet_9oC3_page_2026-10-02.json",
            ROUTER_WALLET_9OC3,
            49,
            11,
            51,
            2,
        ),
        (
            "router_wallet_tAwv_page_2026-10-02.json",
            ROUTER_WALLET_TAWV,
            77,
            10,
            79,
            2,
        ),
    ] {
        let txs = fixture_txs(name).await;
        let now = ledger(wallet, &txs, default_okx_order_policy);
        let promoted = ledger(wallet, &txs, all_verified);
        let booked_now: BTreeSet<[u8; 64]> =
            now.route_swap_log.iter().map(|r| r.signature).collect();
        let booked_promoted: BTreeSet<[u8; 64]> = promoted
            .route_swap_log
            .iter()
            .map(|r| r.signature)
            .collect();
        let left_now = unbooked_route_shaped(wallet, &txs, &booked_now);
        let left_promoted = unbooked_route_shaped(wallet, &txs, &booked_promoted);
        let (e, p) = (
            promoted.trades.route_swaps_by_evidence,
            now.trades.route_swaps_by_evidence,
        );
        println!(
            "{name}: route swaps {} -> {} if promoted (okx {} / okx_only {} / owner==wallet {}); \
             route-shaped unbooked {} -> {} (now: {left_now:?}); okx order events not used {} ; receiver {} ; rejected {:?}",
            now.trades.route_swaps,
            promoted.trades.route_swaps,
            e.okx,
            e.okx_only,
            e.okx_owner_is_wallet,
            left_now.len(),
            left_promoted.len(),
            now.diagnostics.okx_idl_only_order_events,
            now.diagnostics.okx_swap_with_receiver_not_attributed,
            promoted.diagnostics.route_rejected,
        );
        // Production: unchanged by the OKX decoder.
        assert_eq!(now.trades.route_swaps, swaps, "{name}");
        assert_eq!((p.okx, p.okx_only), (0, 0));
        assert_eq!(now.diagnostics.okx_idl_only_order_events, orders, "{name}");
        assert_eq!(now.diagnostics.okx_swap_with_receiver_not_attributed, 0);
        assert_eq!(
            (
                now.diagnostics.okx_malformed_events,
                now.diagnostics.okx_unknown_events
            ),
            (0, 0)
        );
        assert_eq!(now.diagnostics.route_rejected, RouteRejections::default());
        assert_eq!(left_now.len(), unbooked, "{name}");
        // The remaining unbooked route-shaped transactions, all via the router.
        let mut left_sigs: Vec<&str> = left_now.iter().map(|(s, _)| s.as_str()).collect();
        left_sigs.sort_unstable();
        let mut expect: Vec<&str> = if name.contains("9oC3") {
            vec!["3K8mjYT6", "55nGGQdf"]
        } else {
            vec!["4uuSP9kT", "53FC5swy"]
        };
        expect.sort_unstable();
        assert_eq!(left_sigs, expect, "{name}");
        assert!(
            left_now.iter().all(|(_, via)| *via),
            "{name}: unbooked but not via the router"
        );
        // What-if.
        assert_eq!(promoted.trades.route_swaps, promoted_swaps, "{name}");
        assert_eq!(
            e.okx_only,
            promoted.trades.route_swaps - now.trades.route_swaps
        );
        assert_eq!(e.okx_owner_is_wallet, e.okx);
        assert_eq!(promoted.diagnostics.okx_idl_only_order_events, 0);
        assert_eq!(
            promoted.diagnostics.route_rejected,
            RouteRejections::default()
        );
        assert!(booked_now.is_subset(&booked_promoted));
        assert_eq!(
            left_promoted.len(),
            unbooked - usize::try_from(promoted_swaps - swaps).unwrap(),
            "{name}"
        );
    }
}

/// ADR-009 router-forward case `bUh87USD`: the order has a distinct receiver,
/// so it is counted and attributed to neither the signer nor the receiver,
/// even under the injected `FixtureVerified` status.
#[tokio::test]
async fn swap_with_receiver_is_not_attributed_to_either_owner() {
    let name = "pump_variants_live_2026-10-02.json";
    let txs = fixture_txs(name).await;
    let tx = txs
        .iter()
        .find(|t| tx_sig(t).starts_with("bUh87USD"))
        .unwrap();
    let signer = bs58::encode(tx.signers[0]).into_string();
    let receiver = "CNudZYFgpbT26fidsiNrWfHeGTBMMeVWqruZXsEkcUPc";
    let default_policy: OkxOrderPolicy = default_okx_order_policy;
    for (who, policy) in [
        ("signer", default_policy),
        ("signer", all_verified),
        ("receiver", all_verified),
    ] {
        let wallet = if who == "signer" {
            signer.as_str()
        } else {
            receiver
        };
        let r = ledger(wallet, &txs, policy);
        assert_eq!(
            r.diagnostics.okx_swap_with_receiver_not_attributed, 1,
            "{who}"
        );
        assert!(
            r.route_swap_log
                .iter()
                .all(|x| !tx_sig_bytes(&x.signature).starts_with("bUh87USD")),
            "{who}: the receiver-order tx was booked as a route swap"
        );
        assert_eq!(r.trades.route_swaps_by_evidence.okx, 0, "{who}");
        assert!(
            r.evidence_samples
                .iter()
                .any(|e| e.kind.label() == "okx_swap_with_receiver_not_attributed")
        );
    }
}

fn tx_sig_bytes(sig: &[u8; 64]) -> String {
    bs58::encode(sig).into_string()
}
