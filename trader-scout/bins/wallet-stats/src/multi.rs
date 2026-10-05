//! Multi-chain merge of `wallet-stats` (docs/CLI.md §5): one card per input
//! wallet in the ORIGINAL input order across chains (each card keeps its
//! `chain`), `run_meta` listing every chain's sub-run, `run_summary` per chain
//! and overall. A chain that produced no output gets `error` cards carrying
//! the reason; the other chains are unaffected.

use scout_app::{ChainGroup, ChainRun, SCHEMA_VERSION};
use scout_core::WalletKey;
use scout_engine::{
    AnalysisWindow, SolanaProtocolScope, SolanaWalletStatsReport, WalletScanStatus,
};
use serde_json::{Value, json};

use crate::output::{self, Detail, SortMode};

/// Everything the merge needs besides the per-chain runs.
pub(crate) struct MergeInput<'a> {
    pub run_id: &'a str,
    pub captured_at: &'a str,
    pub detail: Detail,
    pub chain_concurrency: usize,
    pub duplicates: usize,
    pub upstream_complete: bool,
    pub overall_status: u8,
    pub cancelled: bool,
}

fn synthetic_cards(
    keys: &[WalletKey],
    group: &ChainGroup,
    message: &str,
) -> Vec<scout_engine::SolanaWalletStats> {
    let reason = format!("chain {} was not scanned: {message}", group.name);
    scout_app::unscanned_cards(keys, WalletScanStatus::Error, &reason)
}

fn keys_of(all: &[WalletKey], g: &ChainGroup) -> Vec<WalletKey> {
    g.indices
        .iter()
        .filter_map(|i| all.get(*i).cloned())
        .collect()
}

pub(crate) fn jsonl(
    all: &[WalletKey],
    groups: &[ChainGroup],
    runs: &[ChainRun],
    m: &MergeInput<'_>,
) -> Result<Vec<String>, String> {
    let mut cards: Vec<Option<Value>> = vec![None; all.len()];
    let mut chain_metas: Vec<Value> = Vec::new();
    let mut chain_summaries: Vec<Value> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    let mut requests_by_chain = serde_json::Map::new();
    let mut requests_total = 0u64;
    for (g, r) in groups.iter().zip(runs) {
        let records = scout_app::parse_records(&r.lines)?;
        let meta = records
            .iter()
            .find(|v| scout_app::record_kind(v) == "run_meta")
            .cloned();
        let summary = records
            .iter()
            .find(|v| scout_app::record_kind(v) == "run_summary")
            .cloned();
        let mut wallets: Vec<Value> = records
            .into_iter()
            .filter(|v| scout_app::record_kind(v) == "wallet_stats")
            .collect();
        let failed = r.failure.is_some() || wallets.len() != g.indices.len();
        let message = r
            .failure
            .clone()
            .unwrap_or_else(|| "internal: card count mismatch".to_string());
        if failed {
            wallets = synthetic_cards(&keys_of(all, g), g, &message)
                .iter()
                .map(|w| {
                    let rec = output::wallet_record(w, m.detail, &|t| t.to_string());
                    let mut v = serde_json::to_value(rec).map_err(|e| e.to_string())?;
                    scout_app::evm_spelling(&mut v, w.chain.native_label);
                    Ok(v)
                })
                .collect::<Result<Vec<_>, String>>()?;
        }
        for (idx, v) in g.indices.iter().zip(wallets) {
            if let Some(slot) = cards.get_mut(*idx) {
                *slot = Some(v);
            }
        }
        for reason in &r.reasons {
            reasons.push(format!("chain {}: {reason}", g.name));
        }
        requests_by_chain.insert(g.name.to_string(), json!(r.requests_made));
        requests_total = requests_total.saturating_add(r.requests_made);
        chain_metas.push(json!({
            "chain": g.name,
            "input_wallet_count": g.indices.len(),
            "status": if failed { "failed" } else { "ran" },
            "exit_code": r.status,
            "failure": r.failure,
            "run_meta": meta,
        }));
        chain_summaries.push(json!({
            "chain": g.name,
            "exit_code": r.status,
            "status": if failed { "failed" } else { "ran" },
            "requests_made": r.requests_made,
            "incomplete_reasons": r.reasons,
            "summary": summary,
        }));
    }
    let cards: Vec<Value> = cards.into_iter().flatten().collect();
    let mut out: Vec<Value> = Vec::with_capacity(cards.len() + 2);
    out.push(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "run_meta",
        "run_id": m.run_id,
        "captured_at": m.captured_at,
        "multi_chain": true,
        "chain_concurrency": m.chain_concurrency,
        "input_wallet_count": all.len(),
        "input_duplicates": m.duplicates,
        "upstream_complete": m.upstream_complete,
        "budget_scope": "max_requests and max_price_requests apply per chain run \
                         (budgets belong to provider families); requests_made is per chain",
        "sort": "input",
        "sort_note": "multi-chain output keeps the input order; native units are not comparable across chains",
        "chains": chain_metas,
    }));
    let statuses: Vec<Value> = cards
        .iter()
        .map(|c| json!({"wallet": c.get("wallet"), "status": c.get("status")}))
        .collect();
    let records = cards.len();
    out.extend(cards);
    if !m.upstream_complete {
        reasons.push(
            "upstream JSONL run is not complete: the wallet universe may be partial".to_string(),
        );
    }
    out.push(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "run_summary",
        "run_id": m.run_id,
        "multi_chain": true,
        "status": if m.overall_status == 0 { "complete" } else { "partial" },
        "exit_code": m.overall_status,
        "cancelled": m.cancelled,
        "records": records,
        "requests_made": requests_total,
        "requests_made_by_chain": requests_by_chain,
        "incomplete_reasons": reasons,
        "chains": chain_summaries,
        "wallets": statuses,
    }));
    scout_app::render_records(&out)
}

pub(crate) fn table(
    all: &[WalletKey],
    groups: &[ChainGroup],
    runs: &[ChainRun],
    detail: Detail,
    window: &AnalysisWindow,
) -> Vec<String> {
    let mut out = Vec::new();
    for (g, r) in groups.iter().zip(runs) {
        match &r.failure {
            None => {
                out.push(format!(
                    "# chain: {} (exit {}, requests_made={})",
                    g.name, r.status, r.requests_made
                ));
                out.extend(r.lines.iter().cloned());
            }
            Some(message) => {
                out.push(format!(
                    "# chain: {} FAILED (exit {}, no cards scanned): {message}",
                    g.name, r.status
                ));
                let report = SolanaWalletStatsReport {
                    scope: SolanaProtocolScope::pump_wallet_ledger(),
                    wallets: synthetic_cards(&keys_of(all, g), g, message),
                    cancelled: false,
                    stop: None,
                    concurrency: 1,
                    evm: None,
                };
                out.extend(output::table_lines(
                    &report,
                    detail,
                    SortMode::Input,
                    window,
                ));
            }
        }
    }
    let named: Vec<(&str, &ChainRun)> = groups.iter().map(|g| g.name).zip(runs).collect();
    out.push(format!(
        "# multi-chain: {} (cards are grouped by chain here; --format jsonl keeps the input order)",
        scout_app::chain_status_text(&named)
    ));
    out
}
