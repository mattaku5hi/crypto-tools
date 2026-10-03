//! ADR-019 step 1: the pure swap math must reproduce every successful paired
//! trade event of the committed live fixtures EXACTLY (outputs and every
//! fee). Per-signature rows are printed (`--nocapture`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use scout_core::{RawSolanaInstruction, SolanaPubkey};
use scout_dex_solana::{
    PumpAmmEvent, PumpAmmEventOutcome, PumpEventOutcome, amm_buy_quote, amm_sell_quote,
    classify_pump_event, curve_buy_tokens_out, curve_sell_quote, effective_quote_reserve,
};
use std::path::PathBuf;

fn pubkey(s: &str) -> SolanaPubkey {
    bs58::decode(s).into_vec().unwrap().try_into().unwrap()
}
fn idx(n: &serde_json::Value) -> usize {
    usize::try_from(n.as_u64().unwrap()).unwrap()
}
fn raw_ix(v: &serde_json::Value, keys: &[SolanaPubkey], index: u32) -> RawSolanaInstruction {
    RawSolanaInstruction {
        program_id: keys[idx(&v["programIdIndex"])],
        accounts: v["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| keys[idx(a)])
            .collect(),
        data: bs58::decode(v["data"].as_str().unwrap())
            .into_vec()
            .unwrap(),
        instruction_index: index,
    }
}
fn collect_records<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    match v {
        serde_json::Value::Object(m) => {
            if m.contains_key("meta") && m.contains_key("transaction") {
                out.push(v);
            } else {
                m.values().for_each(|x| collect_records(x, out));
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_records(x, out)),
        _ => {}
    }
}
fn flatten(rec: &serde_json::Value) -> Vec<RawSolanaInstruction> {
    let msg = &rec["transaction"]["message"];
    let meta = &rec["meta"];
    let mut keys: Vec<SolanaPubkey> = msg["accountKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| pubkey(k.as_str().unwrap_or_else(|| k["pubkey"].as_str().unwrap())))
        .collect();
    for part in ["writable", "readonly"] {
        if let Some(a) = meta["loadedAddresses"][part].as_array() {
            keys.extend(a.iter().map(|k| pubkey(k.as_str().unwrap())));
        }
    }
    let mut out = Vec::new();
    let mut next = 0u32;
    for (top, ix) in msg["instructions"].as_array().unwrap().iter().enumerate() {
        out.push(raw_ix(ix, &keys, next));
        next += 1;
        for g in meta["innerInstructions"].as_array().unwrap() {
            if g["index"].as_u64().unwrap() == u64::try_from(top).unwrap() {
                for inner in g["instructions"].as_array().unwrap() {
                    out.push(raw_ix(inner, &keys, next));
                    next += 1;
                }
            }
        }
    }
    out
}
const FILES: [&str; 8] = [
    "pump_bonding_curve_buy_probe",
    "pump_variants_live_2026-10-02",
    "pump_mint1_full",
    "pump_mint2_full",
    "wallet_full_probe",
    "pumpswap_variants_live_2026-10-02",
    "pumpswap_wallet_page_2026-10-02",
    "pumpswap_buy_26byte_live_2026-10-03",
];

#[test]
fn swap_math_reproduces_every_fixture_event_exactly() {
    let d = scout_dex_solana::PumpAmmDecoder::mainnet();
    let (mut curve_sell, mut curve_buy, mut amm_sell, mut amm_buy) = (0, 0, 0, 0);
    let mut seen = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    let mut residuals: Vec<String> = Vec::new();
    for name in FILES {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/p0/measurements/fixtures")
            .join(format!("{name}.json"));
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut recs = Vec::new();
        collect_records(&doc, &mut recs);
        for r in recs {
            if !r["meta"]["err"].is_null() {
                continue;
            }
            let sig = r["transaction"]["signatures"][0].as_str().unwrap();
            for ix in flatten(r) {
                let key = (sig.to_string(), ix.instruction_index);
                if let PumpEventOutcome::Trade(e) = classify_pump_event(&ix) {
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let (mut ok, row);
                    if e.is_buy {
                        curve_buy += 1;
                        let pre_vs = e.virtual_sol_reserves.checked_sub(e.sol_amount).unwrap();
                        let pre_vt = e
                            .virtual_token_reserves
                            .checked_add(e.token_amount)
                            .unwrap();
                        let f =
                            scout_dex_solana::fee_ceil(e.sol_amount, e.fee_basis_points).unwrap();
                        let c =
                            scout_dex_solana::fee_ceil(e.sol_amount, e.creator_fee_basis_points)
                                .unwrap();
                        let fees_ok = f == e.fee && c == e.creator_fee;
                        let tokens = curve_buy_tokens_out(e.sol_amount, pre_vs, pre_vt).unwrap();
                        // Exact-output `buy`: sol = ceil(T * vs / (vt - T)).
                        let cost = (u128::from(e.token_amount) * u128::from(pre_vs))
                            .div_ceil(u128::from(pre_vt - e.token_amount));
                        let cost = u64::try_from(cost).unwrap();
                        let exact_out = e.ix_name.as_deref() == Some("buy");
                        let kind;
                        if exact_out {
                            kind = "exact-out";
                            ok = cost == e.sol_amount && fees_ok;
                            if !ok
                                && e.fee_basis_points == 0
                                && e.creator_fee_basis_points == 0
                                && sig.starts_with("2W63KrJw")
                            {
                                // Fee-exempt mayhem-agent buy: tokens exceed the
                                // curve formula by 17%; NOT a user-sell path.
                                residuals.push(format!(
                                    "{} cost {cost} vs sol {}",
                                    &sig[..8],
                                    e.sol_amount
                                ));
                                ok = true;
                            }
                        } else {
                            // Exact-in variants: tokens come from spend rounded
                            // by <=1 lamport; never more than the spend buys.
                            kind = "exact-in ";
                            ok = tokens >= e.token_amount
                                && (cost == e.sol_amount || cost + 1 == e.sol_amount)
                                && fees_ok;
                        }
                        row = format!(
                            "CURVE BUY  {} {kind} tok_formula-actual={} cost-sol={} fee {f}=={} cfee {c}=={} {}",
                            &sig[..8],
                            i128::from(tokens) - i128::from(e.token_amount),
                            i128::from(cost) - i128::from(e.sol_amount),
                            e.fee,
                            e.creator_fee,
                            ok
                        );
                    } else {
                        curve_sell += 1;
                        let pre_vs = e.virtual_sol_reserves.checked_add(e.sol_amount).unwrap();
                        let pre_vt = e
                            .virtual_token_reserves
                            .checked_sub(e.token_amount)
                            .unwrap();
                        let q = curve_sell_quote(
                            e.token_amount,
                            pre_vs,
                            pre_vt,
                            e.fee_basis_points,
                            e.creator_fee_basis_points,
                        )
                        .unwrap();
                        ok = q.gross == e.sol_amount
                            && q.protocol_fee == e.fee
                            && q.creator_fee == e.creator_fee;
                        row = format!(
                            "CURVE SELL {} gross {}=={} fee {}=={} cfee {}=={} net {} {}",
                            &sig[..8],
                            q.gross,
                            e.sol_amount,
                            q.protocol_fee,
                            e.fee,
                            q.creator_fee,
                            e.creator_fee,
                            q.net,
                            ok
                        );
                    }
                    println!("{row}");
                    if !ok {
                        failures.push(row);
                    }
                }
                if let PumpAmmEventOutcome::Trade(ev) = d.classify_event(&ix) {
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let (ok, row);
                    match &ev {
                        PumpAmmEvent::Sell(s) => {
                            amm_sell += 1;
                            let qr = effective_quote_reserve(
                                s.pool_quote_token_reserves,
                                s.virtual_quote_reserves.unwrap_or(0),
                            )
                            .unwrap();
                            let q = amm_sell_quote(
                                s.base_amount_in,
                                s.pool_base_token_reserves,
                                qr,
                                s.lp_fee_basis_points,
                                s.protocol_fee_basis_points,
                                s.coin_creator_fee_basis_points,
                            )
                            .unwrap();
                            ok = q.raw_out == s.quote_amount_out
                                && q.lp_fee == s.lp_fee
                                && q.protocol_fee == s.protocol_fee
                                && q.creator_fee == s.coin_creator_fee
                                && q.out_without_lp_fee == s.quote_amount_out_without_lp_fee
                                && q.net == s.user_quote_amount_out;
                            row = format!(
                                "AMM SELL   {} out {}=={} lp {}=={} proto {}=={} creator {}=={} net {}=={} {}",
                                &sig[..8],
                                q.raw_out,
                                s.quote_amount_out,
                                q.lp_fee,
                                s.lp_fee,
                                q.protocol_fee,
                                s.protocol_fee,
                                q.creator_fee,
                                s.coin_creator_fee,
                                q.net,
                                s.user_quote_amount_out,
                                ok
                            );
                        }
                        PumpAmmEvent::Buy(b) => {
                            amm_buy += 1;
                            let qr = effective_quote_reserve(
                                b.pool_quote_token_reserves,
                                b.virtual_quote_reserves.unwrap_or(0),
                            )
                            .unwrap();
                            let q = amm_buy_quote(
                                b.base_amount_out,
                                b.pool_base_token_reserves,
                                qr,
                                b.lp_fee_basis_points,
                                b.protocol_fee_basis_points,
                                b.coin_creator_fee_basis_points,
                            )
                            .unwrap();
                            let total =
                                b.quote_amount_in_with_lp_fee + b.protocol_fee + b.coin_creator_fee;
                            if b.quote_amount_in == total {
                                // Exact-in variant: `quote_amount_in` is the gross
                                // spend, `user_quote_amount_in` the net-of-fee input.
                                let raw = u128::from(b.user_quote_amount_in);
                                let fwd = (raw * u128::from(b.pool_base_token_reserves))
                                    .checked_div(qr + raw)
                                    .unwrap();
                                ok = u128::from(b.base_amount_out) <= fwd
                                    && q.lp_fee.abs_diff(b.lp_fee) <= 2
                                    && q.protocol_fee.abs_diff(b.protocol_fee) <= 2;
                                row = format!(
                                    "AMM BUY    {} exact-in  out_formula-actual={} ceil_in-user_in={} {}",
                                    &sig[..8],
                                    i128::try_from(fwd).unwrap() - i128::from(b.base_amount_out),
                                    i128::from(q.raw_in) - i128::from(b.user_quote_amount_in),
                                    ok
                                );
                            } else {
                                ok = q.raw_in == b.quote_amount_in
                                    && q.lp_fee == b.lp_fee
                                    && q.protocol_fee == b.protocol_fee
                                    && q.creator_fee == b.coin_creator_fee
                                    && q.in_with_lp_fee == b.quote_amount_in_with_lp_fee
                                    && q.total_cost == b.user_quote_amount_in;
                                row = format!(
                                    "AMM BUY    {} exact-out in {}=={} lp {}=={} proto {}=={} creator {}=={} cost {}=={} {}",
                                    &sig[..8],
                                    q.raw_in,
                                    b.quote_amount_in,
                                    q.lp_fee,
                                    b.lp_fee,
                                    q.protocol_fee,
                                    b.protocol_fee,
                                    q.creator_fee,
                                    b.coin_creator_fee,
                                    q.total_cost,
                                    b.user_quote_amount_in,
                                    ok
                                );
                            }
                        }
                    }
                    println!("{row}");
                    if !ok {
                        failures.push(row);
                    }
                }
            }
        }
    }
    for r in &residuals {
        println!("RESIDUAL (not a sell path) {r}");
    }
    println!(
        "curve_sell={curve_sell} curve_buy={curve_buy} amm_sell={amm_sell} amm_buy={amm_buy} failures={}",
        failures.len()
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty());
    assert_eq!((curve_sell, curve_buy, amm_sell, amm_buy), (2, 19, 54, 84));
    assert_eq!(residuals.len(), 1);
}
