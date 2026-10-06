//! EVM path of `buyer-intersect` (ADR-020 step 2, ADR-014 sides).
//!
//! Token-centric over RPC only (no explorer key needed): `eth_getLogs` of
//! each token's `Transfer` topic over the window, receipts, owner-keyed
//! signer trades via FixtureVerified venue events. Output shapes are the
//! Solana ones (`buyer_match` / `run_meta` / `run_summary`); only the scope
//! and the evidence (transaction hash + block) are EVM-specific.
//! The RPC URL (a secret on keyed providers) is never printed.

use scout_app::ChainRun;
use scout_core::{AssetKey, ChainKey};
use scout_engine::{
    AnalysisWindow, EvmBuyerIntersectReport, EvmTokenScanSummary, ScanStop, TokenScanStatus,
    run_evm_buyer_intersect,
};
use scout_providers::{EvmHistoryScanner, ScanLimits};
use serde_json::{Value, json};

use crate::output::{
    BuyerMatchRecord, EvidenceDto, MatchedTokenDto, asset_dto, wallet_dto, window_dto,
};
use crate::{Args, limit_text, side_filter};

pub(crate) fn run_evm(
    rt: &tokio::runtime::Runtime,
    tokens: &[AssetKey],
    chain: &ChainKey,
    args: &Args,
    window: &AnalysisWindow,
) -> ChainRun {
    let started = std::time::Instant::now();
    let notice: scout_app::LimiterNotice =
        std::sync::Arc::new(|m| eprintln!("buyer-intersect: warning: {m}"));
    let setup = match rt.block_on(scout_app::setup_evm(
        chain,
        args.allow_unverified_chain,
        args.max_requests,
        usize::try_from(args.concurrency).unwrap_or(1).max(1) * 2,
        &args.net,
        &notice,
        true,
        |k| std::env::var(k).ok(),
    )) {
        Ok(s) => s,
        Err(scout_app::EvmSetupError::Usage(m)) => {
            eprintln!("buyer-intersect: {m}");
            return ChainRun::failed(2, m);
        }
        Err(scout_app::EvmSetupError::Config(m)) => {
            eprintln!("buyer-intersect: {m}");
            return ChainRun::failed(4, m);
        }
    };
    for w in &setup.warnings {
        eprintln!("buyer-intersect: warning: {w}");
    }
    let secrets = setup.secrets();
    let scrub = move |t: &str| {
        let mut o = t.to_string();
        for s in &secrets {
            if s.len() >= 4 {
                o = o.replace(s.as_str(), "<redacted>");
            }
        }
        o
    };
    let scanner = EvmHistoryScanner::new(
        setup.rpc.clone(),
        setup.chain.clone(),
        ScanLimits::default(),
    );
    // Up-front cost estimate for a range-capped logs endpoint (never burn the
    // whole budget on a scan that cannot finish).
    if setup.logs_span_cap.is_some() {
        let (since, until) = match window.bounds() {
            Some((s, u)) => (u64::try_from(s).ok(), u64::try_from(u).ok()),
            None => (None, None),
        };
        let blocks = match rt.block_on(scanner.resolve_window(since, until)) {
            Ok(b) => b,
            Err(e) => {
                let text = format!("could not resolve the window: {}", scrub(&e.to_string()));
                eprintln!("buyer-intersect: {text}");
                return ChainRun::failed(4, text);
            }
        };
        let hint = scout_app::logs_rpc_env_name(setup.profile.name)
            .unwrap_or("SCOUT_<CHAIN>_LOGS_RPC_URL");
        if let Err(m) = scout_app::check_log_scan_feasible(
            setup.logs_span_cap,
            blocks,
            tokens.len(),
            args.max_requests,
            hint,
        ) {
            eprintln!("buyer-intersect: {m}");
            return ChainRun::failed(4, m);
        }
    }
    let mut info = setup.info.clone();
    info.history_source = "eth_getLogs token scan (RPC) + receipts".to_string();
    info.listing_kind = "token_logs".to_string();
    // Native legs are not needed for sides; say so in the scope.
    info.trace = "not used (sides need no native leg)".to_string();
    info.archive_state = "not used (sides need no native leg)".to_string();
    let _progress = {
        let _rt = rt.enter();
        setup.spawn_progress(&notice, None)
    };
    let mut report = match rt.block_on(run_evm_buyer_intersect(
        &setup.cfg,
        &scanner,
        tokens,
        args.min_token_hits,
        side_filter(args),
        window,
        info,
    )) {
        Ok(r) => r,
        Err(err) => {
            let requests = setup.rpc.total_requests_made();
            eprintln!(
                "buyer-intersect: requests_made={requests} max_requests={}",
                limit_text(args.max_requests)
            );
            if let scout_api::ProviderError::Other(inner) = &err
                && inner
                    .downcast_ref::<scout_rpc::RequestBudgetExhausted>()
                    .is_some()
            {
                eprintln!(
                    "buyer-intersect: request budget exhausted before any data: {} (IncompleteCoverage, exit 3)",
                    scrub(&err.to_string())
                );
                return ChainRun::failed(
                    3,
                    format!(
                        "request budget exhausted before any data: {}",
                        scrub(&err.to_string())
                    ),
                );
            }
            let text = format!("provider error: {}", scrub(&err.to_string()));
            eprintln!("buyer-intersect: {text}");
            return ChainRun::failed(4, text);
        }
    };
    let requests_made = setup.rpc.total_requests_made();
    report.info.rate_limits = setup.rate_limit_report();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    print_diagnostics(&report, requests_made, elapsed_ms, &scrub, args);
    let incomplete = report.is_coverage_incomplete();
    let captured_at = scout_app::now_utc_rfc3339();
    let lines = if args.format == "jsonl" {
        match jsonl_lines(
            &report,
            tokens,
            &captured_at,
            requests_made,
            args,
            incomplete,
            &scrub,
        ) {
            Ok(l) => l,
            Err(m) => {
                eprintln!("buyer-intersect: could not render output: {m}");
                return ChainRun::failed(4, format!("could not render output: {m}"));
            }
        }
    } else {
        table_lines(&report)
    };
    ChainRun {
        lines,
        status: if incomplete { 3 } else { 0 },
        cancelled: report.cancelled,
        requests_made,
        failure: None,
        reasons: report
            .incomplete_reasons()
            .iter()
            .map(|r| scrub(r))
            .collect(),
    }
}

fn side_letters(h: &scout_engine::EvmTokenSideHits) -> &'static str {
    match (h.buy.is_some(), h.sell.is_some()) {
        (true, true) => "B/S",
        (true, false) => "B",
        (false, true) => "S",
        (false, false) => "?",
    }
}

/// `<wallet> hit_count=2 <token>=B/S <token>=S` (same shape as Solana).
fn table_lines(report: &EvmBuyerIntersectReport) -> Vec<String> {
    report
        .base
        .matches
        .iter()
        .map(|m| {
            let hits = report.side_hits.get(&m.wallet);
            let tokens: Vec<String> = m
                .matched_assets
                .iter()
                .map(|a| {
                    let token = match a {
                        AssetKey::Token(_, addr) => addr.to_string(),
                        AssetKey::Native(_) => "native".to_string(),
                    };
                    let sides = hits.and_then(|h| h.get(a)).map_or("?", side_letters);
                    format!("{token}={sides}")
                })
                .collect();
            format!(
                "{} hit_count={} {}",
                m.wallet.address,
                m.hit_count,
                tokens.join(" ")
            )
        })
        .collect()
}

fn evidence(e: &scout_engine::EvmSideEvidence) -> EvidenceDto {
    EvidenceDto {
        signature: format!("{:#x}", e.tx_hash),
        slot: e.block_number,
        venue: e.venue,
        variant: "owner_keyed_net_flow",
    }
}

fn status_label(t: &EvmTokenScanSummary) -> &'static str {
    match t.status {
        TokenScanStatus::Ok => "ok",
        TokenScanStatus::Failed { .. } => "failed",
        TokenScanStatus::NotScanned { .. } => "not_scanned",
    }
}

fn jsonl_lines(
    report: &EvmBuyerIntersectReport,
    tokens: &[AssetKey],
    captured_at: &str,
    requests_made: u64,
    args: &Args,
    incomplete: bool,
    scrub: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let compact: String = captured_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let run_id = format!("buyer-intersect-{compact}");
    let i = &report.info;
    let ser = |v: &Value| serde_json::to_string(v).map_err(|e| e.to_string());
    let mut lines = Vec::new();
    lines.push(ser(&json!({
        "schema_version": scout_app::SCHEMA_VERSION,
        "kind": "run_meta",
        "run_id": run_id,
        "captured_at": captured_at,
        "scope": {
            "chain": i.chain.name,
            "chain_id": i.chain.chain_id,
            "extraction_version": i.extraction_version,
            "recognized": "a wallet qualifies for a token with a booked trade of the selected side: the transaction signer, a swap event of a FixtureVerified venue deployment moving the token, the signer's own net flows = one traded token and one quote asset (native/wrapped-native merged, the chain's pinned stable: USDG on Robinhood, USDC on Base) with opposite signs; side = sign of the wallet's own token delta",
            "not_decoded": "venues not listed as FixtureVerified (their swap-shaped logs are counted as coverage gaps), smart-wallet/AA ownership, trades whose token was delivered to a non-signer",
            "venues": i.venues.iter().map(|x| json!({
                "venue": x.venue, "anchor": x.anchor, "role": x.role,
                "verification": x.verification, "active_from_block": x.active_from_block})).collect::<Vec<_>>(),
            "quote_assets": i.quote_assets.iter().map(|q| json!({
                "symbol": q.symbol, "address": q.address, "decimals": q.decimals,
                "decimals_check": q.decimals_check})).collect::<Vec<_>>(),
            "native_leg": "not resolved: sides do not need the native leg",
            "logs_source": i.logs_source,
            "state_source": i.state_source,
            "rate_limits": i.rate_limits,
            "wallet_set_is_lower_bound": true,
        },
        "budget": {"max_requests": args.max_requests, "concurrency": 1},
        "requests_made": requests_made,
        "input_tokens": tokens.iter().map(asset_dto).collect::<Result<Vec<_>, _>>()?,
        "input_token_count": report.base.input_token_count,
        "min_token_hits": report.base.min_token_hits,
        "side": report.side.label(),
        "window": window_dto(&report.window),
        "scan_order": "block_range",
        "block_range": i.block_range.map(|(a, b)| json!([a, b])),
    }))?);
    for m in &report.base.matches {
        let hits = report.side_hits.get(&m.wallet);
        let matched_tokens = m
            .matched_assets
            .iter()
            .filter_map(|a| hits.and_then(|h| h.get(a)).map(|h| (a, h)))
            .map(|(a, h)| {
                let crate::output::AssetDto::Token { chain, token } = asset_dto(a)? else {
                    return Err("matched asset is not a token".to_string());
                };
                let mut sides = Vec::new();
                if h.buy.is_some() {
                    sides.push("buy");
                }
                if h.sell.is_some() {
                    sides.push("sell");
                }
                Ok(MatchedTokenDto {
                    chain,
                    token,
                    sides,
                    buy_count: h.buy.as_ref().map(|e| e.count),
                    sell_count: h.sell.as_ref().map(|e| e.count),
                    first_buy: h.buy.as_ref().map(evidence),
                    first_sell: h.sell.as_ref().map(evidence),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let rec = BuyerMatchRecord {
            schema_version: scout_app::SCHEMA_VERSION,
            kind: "buyer_match",
            wallet: wallet_dto(&m.wallet)?,
            hit_count: m.hit_count,
            matched_assets: m
                .matched_assets
                .iter()
                .map(asset_dto)
                .collect::<Result<_, _>>()?,
            matched_tokens,
        };
        lines.push(serde_json::to_string(&rec).map_err(|e| e.to_string())?);
    }
    lines.push(ser(&json!({
        "schema_version": scout_app::SCHEMA_VERSION,
        "kind": "run_summary",
        "run_id": run_id,
        "status": if incomplete { "partial" } else { "complete" },
        "cancelled": report.cancelled,
        "records": report.base.matches.len(),
        "incomplete_reasons": report.incomplete_reasons().iter().map(|r| scrub(r)).collect::<Vec<_>>(),
        "tokens": report.per_token.iter().map(|t| json!({
            "token": format!("{}:{:#x}", i.chain.name, t.token),
            "status": status_label(t),
            "error": match &t.status { TokenScanStatus::Failed { message, .. } => Some(scrub(message)), _ => None },
            "stop_reason": match &t.status { TokenScanStatus::NotScanned { reason } => Some(reason.describe()), _ => None },
            "transfer_logs": t.transfer_logs,
            "transactions_scanned": t.transactions_scanned,
            "qualified_buyers": t.transactions_scanned.map(|_| t.qualified_buyers),
            "qualified_sellers": t.transactions_scanned.map(|_| t.qualified_sellers),
            "qualified_wallets": t.transactions_scanned.map(|_| t.qualified_wallets),
            "idl_only_trades": t.transactions_scanned.map(|_| t.idl_only_trades),
            "ungated_swap_logs": t.transactions_scanned.map(|_| t.ungated_swap_logs),
            "ungated_hop_swap_logs": t.transactions_scanned.map(|_| t.ungated_hop_swap_logs),
            "log_splits": t.log_splits,
            "extraction": t.extraction.as_ref().map(|e| json!({
                "transactions": e.transactions, "trades": e.trades,
                "unknown_consideration": e.unknown_consideration, "failed": e.failed,
                "nft_transfer_logs": e.nft_transfer_logs, "no_trade": e.no_trade,
            })),
        })).collect::<Vec<_>>(),
    }))?);
    Ok(lines)
}

fn print_diagnostics(
    report: &EvmBuyerIntersectReport,
    requests_made: u64,
    elapsed_ms: u64,
    scrub: &dyn Fn(&str) -> String,
    args: &Args,
) {
    let i = &report.info;
    eprintln!(
        "buyer-intersect: protocol scope ({} chain id {}):",
        i.chain.name,
        i.chain.chain_id.unwrap_or(0)
    );
    eprintln!("  venues: {}", i.venues_text());
    let quotes: Vec<String> = i
        .quote_assets
        .iter()
        .map(|q| {
            format!(
                "{}={} decimals={} ({})",
                q.symbol, q.address, q.decimals, q.decimals_check
            )
        })
        .collect();
    eprintln!(
        "  quote assets: native (WETH merged), {}",
        quotes.join(", ")
    );
    eprintln!(
        "  source: {}; native leg: not resolved (sides need none)",
        i.history_source
    );
    eprintln!(
        "  routing: eth_getLogs -> {}; receipts/state -> {}",
        i.logs_source, i.state_source
    );
    for l in &i.rate_limits {
        eprintln!("  rate limit: {l}");
    }
    eprintln!(
        "  extraction: {}",
        scout_engine::EVM_BUYER_INTERSECT_VERSION
    );
    eprintln!("  side={}", report.side.label());
    match (report.window.bounds(), i.block_range) {
        (Some((s, u)), r) => eprintln!(
            "  window [{}, {}) source={} blocks={:?}",
            scout_app::format_unix_utc(u64::try_from(s).unwrap_or(0)),
            scout_app::format_unix_utc(u64::try_from(u).unwrap_or(0)),
            report.window.source.label(),
            r
        ),
        (None, _) => eprintln!("  window: none (full available history)"),
    }
    eprintln!(
        "  requests_made={requests_made} max_requests={} elapsed_ms={elapsed_ms} (stderr only)",
        limit_text(args.max_requests)
    );
    eprintln!(
        "  input_tokens(N)={} min_token_hits(K)={} matches={}",
        report.base.input_token_count,
        report.base.min_token_hits,
        report.base.matches.len()
    );
    for t in &report.per_token {
        let status = match &t.status {
            TokenScanStatus::Failed { message, .. } => format!("scan failed: {}", scrub(message)),
            TokenScanStatus::NotScanned { reason } => format!("not_scanned: {}", reason.describe()),
            TokenScanStatus::Ok => "ok".to_string(),
        };
        eprintln!(
            "  token {:#x}: status={status} transfer_logs={:?} txs_scanned={:?} buyers={} sellers={} wallets={} idl_only_trades={} ungated_swap_logs={} ungated_hop_swap_logs={}",
            t.token,
            t.transfer_logs,
            t.transactions_scanned,
            t.qualified_buyers,
            t.qualified_sellers,
            t.qualified_wallets,
            t.idl_only_trades,
            t.ungated_swap_logs,
            t.ungated_hop_swap_logs
        );
    }
    if let Some(ScanStop::BudgetExhausted { limit }) = report.stop {
        eprintln!(
            "buyer-intersect: request budget exhausted after {requests_made} requests (limit {limit}); unscanned tokens are marked failed/not_scanned"
        );
    }
    let reasons = report.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!(
            "buyer-intersect: status=complete within declared protocol scope (wallet set is a lower bound: unverified venues are not decoded)"
        );
    } else {
        eprintln!("buyer-intersect: status=partial (IncompleteCoverage, exit 3):");
        for r in reasons {
            eprintln!("  - {}", scrub(&r));
        }
    }
}
