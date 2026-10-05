//! Multi-chain `wallet-rank` (docs/CLI.md §4). Chains are scanned with their
//! own sources and budgets; ranking is
//!
//! * `--quote usd` (default for mixed input): ONE list over all chains in USD
//!   lower bounds (ADR-016/018), each row shows its chain;
//! * a native/stable unit: one list PER CHAIN where the unit exists (`--top`
//!   per chain); the wallets of the other chains are excluded with
//!   `quote_unit_not_on_chain` and are not scanned.
//!
//! Identity stays chain-scoped (invariant 3); native units are never mixed or
//! compared across chains (ARCHITECTURE §10).

use scout_app::{ChainGroup, ChainRun, SCHEMA_VERSION, WriteOutcome, write_lines_to_stdout};
use scout_core::{ChainKey, WalletKey};
use scout_engine::{
    AnalysisWindow, QuoteUnit, RankPolicy, SolanaWalletStats, WalletRankReport, WalletScanStatus,
    exclude_quote_unit_not_on_chain, quote_unit_label, rank_solana_wallets,
};
use serde_json::{Map, Value, json};
use std::process::ExitCode;

use crate::output::{self, SummaryInput};
use crate::{Args, RankRun, limit_text, policy_from, run_chain};

/// The unit `flag` (`--quote`) means on `chain`, `None` when it does not
/// exist there. `usd` exists everywhere.
pub(crate) fn unit_on_chain(flag: &str, chain: &ChainKey) -> Option<QuoteUnit> {
    let d = scout_app::display_of_chain(chain);
    let units = d.quote_units();
    let has = |u: QuoteUnit| units.contains(&u);
    match flag {
        "usd" => Some(QuoteUnit::ReportCurrency),
        "sol" => (d.native_label == "sol").then_some(QuoteUnit::Lamports),
        "eth" => (d.native_label == "eth").then_some(QuoteUnit::Wei),
        "bnb" => (d.native_label == "bnb").then_some(QuoteUnit::Wei),
        "usdc" => [QuoteUnit::UsdcUnits, QuoteUnit::BinancePegUsdcUnits]
            .into_iter()
            .find(|u| has(*u)),
        "usdt" => [QuoteUnit::UsdtUnits, QuoteUnit::BinancePegUsdtUnits]
            .into_iter()
            .find(|u| has(*u)),
        "usdg" => has(QuoteUnit::UsdgUnits).then_some(QuoteUnit::UsdgUnits),
        _ => None,
    }
}

/// One chain of the run after scanning/skipping.
struct Part {
    name: &'static str,
    /// `None` = skipped by design (unit not on chain).
    run: Option<ChainRun>,
    /// Cards of the group, group order.
    cards: Vec<SolanaWalletStats>,
    policy: RankPolicy,
    /// Native mode / table: chain-local lines of the selected format.
    lines: Vec<String>,
    /// JSONL parts of the chain-local ranking.
    meta: Value,
    ranked: Vec<Value>,
    excluded: Vec<Value>,
    summary: Value,
    secrets: Vec<String>,
    pricing_line: Option<String>,
    prices_made: u64,
}

fn evm_spell(v: &mut Value, native_label: &str, is_evm: bool) {
    if is_evm {
        scout_app::evm_spelling(v, native_label);
    }
}

fn scrubber(secrets: &[String]) -> impl Fn(&str) -> String + '_ {
    move |t| {
        let mut o = t.to_string();
        for s in secrets {
            if s.len() >= 4 {
                o = o.replace(s.as_str(), "<redacted>");
            }
        }
        o
    }
}

fn excluded_values(report: &WalletRankReport, secrets: &[String]) -> Result<Vec<Value>, String> {
    let scrub = scrubber(secrets);
    report
        .excluded
        .iter()
        .map(|e| {
            let mut v = serde_json::to_value(output::excluded_record(e, &scrub))
                .map_err(|e| e.to_string())?;
            evm_spell(
                &mut v,
                e.observation.chain.native_label,
                e.observation.chain.is_evm(),
            );
            Ok(v)
        })
        .collect()
}

fn summary_value(
    report: &WalletRankReport,
    run_id: &str,
    partial: bool,
    reasons: Vec<String>,
) -> Result<Value, String> {
    serde_json::to_value(output::run_summary_record(
        report,
        SummaryInput {
            run_id,
            partial,
            cancelled: false,
            stop: None,
            requests_made: 0,
            requests_made_prices: 0,
            incomplete_reasons: reasons,
        },
    ))
    .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_multi(
    rt: &tokio::runtime::Runtime,
    wallets: &[WalletKey],
    groups: &[ChainGroup],
    args: &Args,
    window: &AnalysisWindow,
    duplicates: usize,
    upstream_complete: bool,
) -> ExitCode {
    let flag = args.quote.clone().unwrap_or_else(|| "usd".to_string());
    let usd = flag == "usd";
    let base = match policy_from(args, &flag) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("wallet-rank: {m}");
            return ExitCode::from(2);
        }
    };
    let keys_of = |g: &ChainGroup| -> Vec<WalletKey> {
        g.indices
            .iter()
            .filter_map(|i| wallets.get(*i).cloned())
            .collect()
    };
    let units: Vec<Option<QuoteUnit>> = groups
        .iter()
        .map(|g| unit_on_chain(&flag, &g.chain))
        .collect();
    if units.iter().all(Option::is_none) {
        eprintln!(
            "wallet-rank: --quote {flag} is not a unit of any input chain; nothing would be \
             ranked (use --quote usd to rank across chains)"
        );
        return ExitCode::from(2);
    }
    let mut sub = args.clone();
    sub.top = args.top;
    eprintln!(
        "wallet-rank: multi-chain run over {} chains ({}), --quote {flag}{}; --max-requests \
         applies per chain run, chain concurrency {}",
        groups.len(),
        groups.iter().map(|g| g.name).collect::<Vec<_>>().join(", "),
        if usd {
            " (one USD ranking across chains)"
        } else {
            " (a separate ranking per chain; other chains excluded: quote_unit_not_on_chain)"
        },
        args.chain_concurrency
    );
    let work: Vec<(&ChainGroup, RankPolicy)> = groups
        .iter()
        .zip(&units)
        .filter_map(|(g, u)| u.map(|u| (g, base.with_quote(u))))
        .collect();
    let results = scout_app::run_chains_bounded(
        work,
        usize::try_from(args.chain_concurrency).unwrap_or(1),
        |(g, policy)| {
            // The upstream-complete rule is applied once, to the merged run.
            run_chain(rt, &keys_of(g), &g.chain, &sub, &policy, window, 0, true)
        },
    );
    let mut results = results.into_iter();
    let captured_at = scout_app::now_utc_rfc3339();
    let compact: String = captured_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let run_id = format!("wallet-rank-{compact}");
    let jsonl = args.format == "jsonl";

    let mut parts: Vec<Part> = Vec::new();
    for (g, unit) in groups.iter().zip(&units) {
        let keys = keys_of(g);
        let Some(unit) = unit else {
            let reason = format!(
                "quote_unit_not_on_chain: --quote {flag} does not exist on {} (not scanned)",
                g.name
            );
            let cards = scout_app::unscanned_cards(&keys, WalletScanStatus::NotScanned, &reason);
            let policy = base.with_quote(QuoteUnit::Lamports);
            let report = exclude_quote_unit_not_on_chain(&cards, &policy);
            parts.push(synthetic(
                g, None, cards, policy, &report, &run_id, &reason, window,
            ));
            continue;
        };
        let policy = base.with_quote(*unit);
        let rr: RankRun = match results.next() {
            Some(Ok(r)) => r,
            Some(Err(m)) => ChainRun::failed(4, m).into(),
            None => ChainRun::failed(4, "internal: missing chain result".to_string()).into(),
        };
        if let Some(message) = rr.run.failure.clone() {
            let reason = format!("chain {} was not scanned: {message}", g.name);
            let cards = scout_app::unscanned_cards(&keys, WalletScanStatus::Error, &reason);
            let report = rank_solana_wallets(&cards, &policy);
            parts.push(synthetic(
                g,
                Some(rr.run),
                cards,
                policy,
                &report,
                &run_id,
                &reason,
                window,
            ));
            continue;
        }
        let records = match scout_app::parse_records(&rr.run.lines) {
            Ok(r) if jsonl => r,
            _ => Vec::new(),
        };
        let pick = |k: &str| -> Vec<Value> {
            records
                .iter()
                .filter(|v| scout_app::record_kind(v) == k)
                .cloned()
                .collect()
        };
        parts.push(Part {
            name: g.name,
            meta: pick("run_meta").into_iter().next().unwrap_or(Value::Null),
            ranked: pick("wallet_rank"),
            excluded: pick("wallet_excluded"),
            summary: pick("run_summary")
                .into_iter()
                .next()
                .unwrap_or(Value::Null),
            lines: rr.run.lines.clone(),
            cards: rr.stats.map(|s| s.wallets).unwrap_or_default(),
            policy,
            secrets: rr.secrets,
            pricing_line: rr.pricing_line,
            prices_made: rr.prices_made,
            run: Some(rr.run),
        });
    }

    let ran: Vec<&ChainRun> = parts.iter().filter_map(|p| p.run.as_ref()).collect();
    let statuses: Vec<u8> = ran.iter().map(|r| r.status).collect();
    let mut overall = scout_app::aggregate_exit(&statuses);
    if overall == 0 && !upstream_complete {
        overall = 3;
    }
    let cancelled = ran.iter().any(|r| r.cancelled);
    for p in &parts {
        match &p.run {
            Some(r) => eprintln!(
                "wallet-rank: chain {}: exit={} requests_made={} max_requests={}{}",
                p.name,
                r.status,
                r.requests_made,
                limit_text(args.max_requests),
                r.failure
                    .as_ref()
                    .map_or_else(String::new, |m| format!(" FAILED: {m}"))
            ),
            None => eprintln!(
                "wallet-rank: chain {}: skipped (quote_unit_not_on_chain), no requests",
                p.name
            ),
        }
    }
    eprintln!(
        "wallet-rank: multi-chain exit code {overall} (worst chain rules, exit 4 only if every \
         scanned chain failed)"
    );

    // Cross-chain USD ranking: all cards in the ORIGINAL input order.
    let merged_report = usd.then(|| {
        let mut slots: Vec<Option<SolanaWalletStats>> = vec![None; wallets.len()];
        for (g, p) in groups.iter().zip(&parts) {
            for (idx, card) in g.indices.iter().zip(p.cards.iter().cloned()) {
                if let Some(s) = slots.get_mut(*idx) {
                    *s = Some(card);
                }
            }
        }
        let cards: Vec<SolanaWalletStats> = slots.into_iter().flatten().collect();
        rank_solana_wallets(&cards, &base.with_quote(QuoteUnit::ReportCurrency))
    });

    let lines = if jsonl {
        match merged_jsonl(
            &parts,
            merged_report.as_ref(),
            &run_id,
            &captured_at,
            &flag,
            overall,
            cancelled,
            (args, duplicates, upstream_complete),
        ) {
            Ok(l) => l,
            Err(m) => {
                eprintln!("wallet-rank: could not render output: {m}");
                return ExitCode::from(4);
            }
        }
    } else {
        merged_table(&parts, merged_report.as_ref(), overall, window, &flag)
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

/// A chain that was skipped by design or produced no output: cards, the
/// chain-local exclusion report and its JSONL/table renderings.
#[allow(clippy::too_many_arguments)]
fn synthetic(
    g: &ChainGroup,
    run: Option<ChainRun>,
    cards: Vec<SolanaWalletStats>,
    policy: RankPolicy,
    report: &WalletRankReport,
    run_id: &str,
    reason: &str,
    window: &AnalysisWindow,
) -> Part {
    let skipped = run.is_none();
    let table = output::table_lines(report, true, window, None);
    let excluded = excluded_values(report, &[]).unwrap_or_default();
    let summary =
        summary_value(report, run_id, !skipped, vec![reason.to_string()]).unwrap_or(Value::Null);
    Part {
        name: g.name,
        run,
        cards,
        policy,
        lines: table,
        meta: json!({
            "chain": g.name,
            "status": if skipped { "skipped" } else { "failed" },
            "reason": reason,
        }),
        ranked: Vec::new(),
        excluded,
        summary,
        secrets: Vec::new(),
        pricing_line: None,
        prices_made: 0,
    }
}

fn sum_u64(parts: &[Part], key: &str) -> u64 {
    parts
        .iter()
        .filter_map(|p| p.summary.get(key).and_then(Value::as_u64))
        .sum()
}

fn sum_map(parts: &[Part], key: &str) -> Map<String, Value> {
    let mut m: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for p in parts {
        if let Some(o) = p.summary.get(key).and_then(Value::as_object) {
            for (k, v) in o {
                *m.entry(k.clone()).or_insert(0) += v.as_u64().unwrap_or(0);
            }
        }
    }
    m.into_iter().map(|(k, v)| (k, json!(v))).collect()
}

#[allow(clippy::too_many_arguments)]
fn merged_jsonl(
    parts: &[Part],
    merged: Option<&WalletRankReport>,
    run_id: &str,
    captured_at: &str,
    flag: &str,
    overall: u8,
    cancelled: bool,
    (args, duplicates, upstream_complete): (&Args, usize, bool),
) -> Result<Vec<String>, String> {
    let mut reasons: Vec<String> = Vec::new();
    let mut by_chain = Map::new();
    let mut requests = 0u64;
    let mut prices = 0u64;
    let mut chains_meta: Vec<Value> = Vec::new();
    let mut chains_summary: Vec<Value> = Vec::new();
    for p in parts {
        let (status, made) = p
            .run
            .as_ref()
            .map_or((Value::Null, 0), |r| (json!(r.status), r.requests_made));
        for r in p.run.iter().flat_map(|r| &r.reasons) {
            reasons.push(format!("chain {}: {r}", p.name));
        }
        if p.run.is_none() {
            reasons.push(format!(
                "chain {}: quote_unit_not_on_chain: its wallets are excluded, not scanned",
                p.name
            ));
        }
        by_chain.insert(p.name.to_string(), json!(made));
        requests = requests.saturating_add(made);
        prices = prices.saturating_add(p.prices_made);
        chains_meta.push(json!({
            "chain": p.name,
            "status": match &p.run {
                None => "skipped",
                Some(r) if r.failure.is_some() => "failed",
                Some(_) => "ran",
            },
            "exit_code": status,
            "rank_quote_unit": quote_unit_label(p.policy.quote),
            "run_meta": p.meta,
        }));
        chains_summary.push(json!({
            "chain": p.name,
            "exit_code": status,
            "requests_made": made,
            "summary": p.summary,
        }));
    }
    if !upstream_complete {
        reasons.push(
            "upstream JSONL run is not complete: the wallet universe may be partial".to_string(),
        );
    }
    let mut out: Vec<Value> = Vec::new();
    out.push(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "run_meta",
        "run_id": run_id,
        "captured_at": captured_at,
        "multi_chain": true,
        "chain_concurrency": args.chain_concurrency,
        "quote": flag,
        "ranking": if merged.is_some() {
            "one list across chains in USD lower bounds (ADR-016/018); rows keep their chain"
        } else {
            "one list per chain in its native/stable unit; other chains: quote_unit_not_on_chain"
        },
        "input_wallet_count": parts.iter().map(|p| p.cards.len()).sum::<usize>(),
        "input_duplicates": duplicates,
        "upstream_complete": upstream_complete,
        "budget_scope": "max_requests and max_price_requests apply per chain run; requests_made is per chain",
        "chains": chains_meta,
    }));
    let secrets: Vec<String> = parts.iter().flat_map(|p| p.secrets.clone()).collect();
    let summary_core: Map<String, Value>;
    if let Some(r) = merged {
        for rk in &r.ranked {
            let rec = output::rank_record(rk, r)
                .ok_or_else(|| "ranked wallet without ledger (internal error)".to_string())?;
            let mut v = serde_json::to_value(rec).map_err(|e| e.to_string())?;
            evm_spell(
                &mut v,
                rk.observation.chain.native_label,
                rk.observation.chain.is_evm(),
            );
            out.push(v);
        }
        out.extend(excluded_values(r, &secrets)?);
        let s = summary_value(r, run_id, overall != 0, Vec::new())?;
        summary_core = s.as_object().cloned().unwrap_or_default();
    } else {
        for p in parts {
            out.extend(p.ranked.iter().cloned());
        }
        for p in parts {
            out.extend(p.excluded.iter().cloned());
        }
        let mut m = Map::new();
        m.insert("rank_by".into(), json!(args.rank_by));
        m.insert("rank_quote_unit".into(), json!(flag));
        m.insert("profile".into(), json!(args.profile));
        m.insert(
            "input_wallets".into(),
            json!(sum_u64(parts, "input_wallets")),
        );
        m.insert("eligible".into(), json!(sum_u64(parts, "eligible")));
        m.insert("ranked".into(), json!(sum_u64(parts, "ranked")));
        m.insert("excluded".into(), json!(sum_u64(parts, "excluded")));
        m.insert(
            "excluded_by_primary_reason".into(),
            Value::Object(sum_map(parts, "excluded_by_primary_reason")),
        );
        m.insert(
            "excluded_by_any_reason".into(),
            Value::Object(sum_map(parts, "excluded_by_any_reason")),
        );
        summary_core = m;
    }
    let mut summary = summary_core;
    summary.insert("schema_version".into(), json!(SCHEMA_VERSION));
    summary.insert("kind".into(), json!("run_summary"));
    summary.insert("run_id".into(), json!(run_id));
    summary.insert("multi_chain".into(), json!(true));
    summary.insert(
        "status".into(),
        json!(if overall == 0 { "complete" } else { "partial" }),
    );
    summary.insert("exit_code".into(), json!(overall));
    summary.insert("cancelled".into(), json!(cancelled));
    summary.insert("requests_made".into(), json!(requests));
    summary.insert("requests_made_by_chain".into(), Value::Object(by_chain));
    summary.insert("requests_made_prices".into(), json!(prices));
    summary.insert("stop".into(), Value::Null);
    summary.insert("incomplete_reasons".into(), json!(reasons));
    summary.insert("chains".into(), json!(chains_summary));
    out.push(Value::Object(summary));
    scout_app::render_records(&out)
}

fn merged_table(
    parts: &[Part],
    merged: Option<&WalletRankReport>,
    overall: u8,
    window: &AnalysisWindow,
    flag: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let chain_lines: Vec<String> = parts
        .iter()
        .map(|p| match &p.run {
            None => format!(
                "# chain: {} skipped: quote_unit_not_on_chain ({} wallet(s) excluded, not scanned)",
                p.name,
                p.cards.len()
            ),
            Some(r) => format!(
                "# chain: {} exit={} requests_made={}{}",
                p.name,
                r.status,
                r.requests_made,
                r.failure
                    .as_ref()
                    .map_or_else(String::new, |m| format!(" FAILED: {m}"))
            ),
        })
        .collect();
    match merged {
        Some(r) => {
            let note: Vec<String> = parts
                .iter()
                .filter_map(|p| p.pricing_line.as_ref().map(|l| format!("{}: {l}", p.name)))
                .collect();
            let pricing = (!note.is_empty()).then(|| note.join(" | "));
            let partial = overall != 0;
            out.extend(output::table_lines(r, partial, window, pricing.as_deref()));
            out.extend(chain_lines);
        }
        None => {
            for (p, head) in parts.iter().zip(chain_lines) {
                out.push(format!("{head} quote={}", quote_unit_label(p.policy.quote)));
                out.extend(p.lines.iter().cloned());
            }
            out.push(format!(
                "# multi-chain --quote {flag}: one ranking per chain (native units are never compared across chains; use --quote usd)"
            ));
        }
    }
    out.push(format!("# multi-chain exit code {overall}"));
    out
}
