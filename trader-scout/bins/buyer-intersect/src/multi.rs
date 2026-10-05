//! Multi-chain `buyer-intersect` (docs/CLI.md §3). Tokens are chain-scoped
//! (AGENTS.md invariant 3): K counts distinct tokens within ONE chain, a
//! wallet address on two chains is two WalletKeys and hits never merge across
//! chains. The run is partitioned by chain, each chain runs with its own
//! sources and budget, matches are concatenated with their chain.

use std::process::ExitCode;

use scout_app::{ChainGroup, ChainRun, SCHEMA_VERSION, WriteOutcome, write_lines_to_stdout};
use scout_core::AssetKey;
use scout_engine::AnalysisWindow;
use serde_json::{Map, Value, json};

use crate::{Args, limit_text, run_chain};

pub(crate) fn run_multi(
    rt: &tokio::runtime::Runtime,
    tokens: &[AssetKey],
    groups: &[ChainGroup],
    args: &Args,
    window: &AnalysisWindow,
) -> ExitCode {
    let k = args.min_token_hits.max(1);
    // A chain with fewer tokens than K cannot produce a hit: not scanned.
    let runnable: Vec<&ChainGroup> = groups.iter().filter(|g| g.indices.len() >= k).collect();
    if runnable.is_empty() {
        eprintln!(
            "buyer-intersect: no chain has at least --min-token-hits ({k}) input tokens; \
             tokens are chain-scoped, K never counts across chains (nothing scanned)"
        );
        return ExitCode::from(2);
    }
    eprintln!(
        "buyer-intersect: multi-chain run over {} chains ({}); tokens are chain-scoped (K counts \
         within a chain); --max-requests applies per chain run, chain concurrency {}",
        groups.len(),
        groups.iter().map(|g| g.name).collect::<Vec<_>>().join(", "),
        args.chain_concurrency
    );
    let results = scout_app::run_chains_bounded(
        runnable.clone(),
        usize::try_from(args.chain_concurrency).unwrap_or(1),
        |g| {
            let subset: Vec<AssetKey> = g
                .indices
                .iter()
                .filter_map(|i| tokens.get(*i).cloned())
                .collect();
            run_chain(rt, &subset, &g.chain, args, window)
        },
    );
    let mut results = results.into_iter();
    // (group, run) in group order; None = skipped (fewer than K tokens).
    let mut runs: Vec<(&ChainGroup, Option<ChainRun>)> = Vec::new();
    for g in groups {
        if g.indices.len() < k {
            eprintln!(
                "buyer-intersect: chain {}: skipped, {} token(s) < --min-token-hits {k}",
                g.name,
                g.indices.len()
            );
            runs.push((g, None));
            continue;
        }
        let r = match results.next() {
            Some(Ok(r)) => r,
            Some(Err(m)) => ChainRun::failed(4, m),
            None => ChainRun::failed(4, "internal: missing chain result".to_string()),
        };
        eprintln!(
            "buyer-intersect: chain {}: exit={} requests_made={} max_requests={}{}",
            g.name,
            r.status,
            r.requests_made,
            limit_text(args.max_requests),
            r.failure
                .as_ref()
                .map_or_else(String::new, |m| format!(" FAILED: {m}"))
        );
        runs.push((g, Some(r)));
    }
    let statuses: Vec<u8> = runs
        .iter()
        .filter_map(|(_, r)| r.as_ref().map(|r| r.status))
        .collect();
    let overall = scout_app::aggregate_exit(&statuses);
    let cancelled = runs
        .iter()
        .any(|(_, r)| r.as_ref().is_some_and(|r| r.cancelled));
    eprintln!(
        "buyer-intersect: multi-chain exit code {overall} (worst chain rules, exit 4 only if \
         every scanned chain failed)"
    );
    let lines = if args.format == "jsonl" {
        match jsonl(&runs, args, overall, cancelled) {
            Ok(l) => l,
            Err(m) => {
                eprintln!("buyer-intersect: could not render output: {m}");
                return ExitCode::from(4);
            }
        }
    } else {
        table(&runs, overall)
    };
    let outcome = write_lines_to_stdout(lines);
    if cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    ExitCode::from(overall)
}

fn jsonl(
    runs: &[(&ChainGroup, Option<ChainRun>)],
    args: &Args,
    overall: u8,
    cancelled: bool,
) -> Result<Vec<String>, String> {
    let captured_at = scout_app::now_utc_rfc3339();
    let compact: String = captured_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let run_id = format!("buyer-intersect-{compact}");
    let mut metas = Vec::new();
    let mut sums = Vec::new();
    let mut matches: Vec<Value> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    let mut by_chain = Map::new();
    let mut total = 0u64;
    for (g, r) in runs {
        let Some(r) = r else {
            reasons.push(format!(
                "chain {}: skipped, fewer tokens than --min-token-hits (cannot match)",
                g.name
            ));
            metas.push(json!({"chain": g.name, "status": "skipped",
                "reason": "fewer_tokens_than_min_token_hits", "input_token_count": g.indices.len()}));
            sums.push(json!({"chain": g.name, "status": "skipped"}));
            continue;
        };
        let records = scout_app::parse_records(&r.lines)?;
        let find = |k: &str| {
            records
                .iter()
                .find(|v| scout_app::record_kind(v) == k)
                .cloned()
        };
        metas.push(json!({
            "chain": g.name,
            "status": if r.failure.is_some() { "failed" } else { "ran" },
            "exit_code": r.status,
            "failure": r.failure,
            "input_token_count": g.indices.len(),
            "run_meta": find("run_meta"),
        }));
        sums.push(json!({
            "chain": g.name,
            "exit_code": r.status,
            "requests_made": r.requests_made,
            "incomplete_reasons": r.reasons,
            "summary": find("run_summary"),
        }));
        matches.extend(
            records
                .iter()
                .filter(|v| scout_app::record_kind(v) == "buyer_match")
                .cloned(),
        );
        for x in &r.reasons {
            reasons.push(format!("chain {}: {x}", g.name));
        }
        by_chain.insert(g.name.to_string(), json!(r.requests_made));
        total = total.saturating_add(r.requests_made);
    }
    let mut out = Vec::with_capacity(matches.len() + 2);
    out.push(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "run_meta",
        "run_id": run_id,
        "captured_at": captured_at,
        "multi_chain": true,
        "chain_concurrency": args.chain_concurrency,
        "min_token_hits": args.min_token_hits,
        "identity_note": "tokens and wallets are chain-scoped: K counts within a chain, hits never merge across chains",
        "budget_scope": "max_requests applies per chain run; requests_made is per chain",
        "chains": metas,
    }));
    let n = matches.len();
    out.extend(matches);
    out.push(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "run_summary",
        "run_id": run_id,
        "multi_chain": true,
        "status": if overall == 0 { "complete" } else { "partial" },
        "exit_code": overall,
        "cancelled": cancelled,
        "matches": n,
        "requests_made": total,
        "requests_made_by_chain": by_chain,
        "incomplete_reasons": reasons,
        "chains": sums,
    }));
    scout_app::render_records(&out)
}

fn table(runs: &[(&ChainGroup, Option<ChainRun>)], overall: u8) -> Vec<String> {
    let mut out = Vec::new();
    for (g, r) in runs {
        match r {
            None => out.push(format!(
                "# chain: {} skipped: fewer tokens than --min-token-hits",
                g.name
            )),
            Some(r) => {
                out.push(format!(
                    "# chain: {} exit={} requests_made={}{}",
                    g.name,
                    r.status,
                    r.requests_made,
                    r.failure
                        .as_ref()
                        .map_or_else(String::new, |m| format!(" FAILED: {m}"))
                ));
                out.extend(r.lines.iter().cloned());
            }
        }
    }
    out.push(format!(
        "# multi-chain exit code {overall} (matches are per chain; K never counts across chains)"
    ));
    out
}
