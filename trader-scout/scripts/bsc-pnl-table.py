#!/usr/bin/env python3
"""Join BSC human-like candidates to wallet stats and sort by exact realized USD PnL."""
import argparse
import csv
import hashlib
import json
from decimal import Decimal, InvalidOperation, ROUND_HALF_EVEN
from pathlib import Path


FIELDS = {
    "pnl30d_status": ("status",),
    "pnl30d_error": ("error",),
    "pnl30d_history_complete": ("coverage", "complete"),
    "pnl30d_coverage": ("coverage",),
    "pnl30d_txs_scanned": ("coverage", "transactions_scanned"),
    "pnl30d_realized_net_usd": ("stats", "usd", "realized_net_pnl", "decimal"),
    "pnl30d_realized_lower_bound_usd": ("stats", "usd", "realized_pnl_lower_bound", "decimal"),
    "pnl30d_realized_lower_bound_status": ("stats", "usd", "realized_pnl_lower_bound", "status"),
    "pnl30d_roi_usd_pct": ("stats", "usd", "realized_cost_roi", "percent_2dp"),
    "pnl30d_win_rate_known": ("stats", "usd", "win_rate", "value"),
    "pnl30d_win_rate_lower_bound_pct": ("stats", "usd", "win_rate_lower_bound", "percent_2dp"),
    "pnl30d_profit_factor": ("stats", "usd", "profit_factor", "value"),
    "pnl30d_closed_known": ("stats", "usd", "closed_known"),
    "pnl30d_closed_unknown": ("stats", "usd", "closed_unknown"),
    "pnl30d_unknown_share_pct": ("stats", "usd", "unknown_episode_share", "percent_2dp"),
    "pnl30d_wins": ("stats", "usd", "wins"),
    "pnl30d_losses": ("stats", "usd", "losses"),
    "pnl30d_median_hold_s": ("stats", "median_holding_seconds"),
    "pnl30d_buys": ("stats", "trades", "buys"),
    "pnl30d_sells": ("stats", "trades", "sells"),
    "pnl30d_mints_traded": ("stats", "distinct_mints_traded"),
    "pnl30d_active_days": ("stats", "activity", "active_utc_days"),
    "pnl30d_trades_per_day": ("stats", "activity", "trades_per_active_day", "display_2dp"),
    "pnl30d_open_positions": ("stats", "open_episodes"),
    "pnl30d_open_value_usd": ("stats", "evm_open_valuation", "value_usd"),
}
GOOD_STATUSES = {"ok", "incomplete", "no_trade_activity"}
VALID_STATUSES = GOOD_STATUSES | {"no_activity", "no_pump_activity", "error", "not_scanned"}


def get(data, path):
    for key in path:
        if not isinstance(data, dict):
            return None
        data = data.get(key)
    return data


def load_source(path):
    records = []
    meta = summary = None
    lines = Path(path).read_text().splitlines()
    if not lines or json.loads(lines[0]).get("kind") != "run_meta":
        raise ValueError(f"{path}: run_meta must be the first record")
    for line_no, line in enumerate(lines, 1):
        item = json.loads(line)
        if item.get("kind") == "run_meta":
            if meta is not None:
                raise ValueError(f"{path}:{line_no}: duplicate run_meta")
            meta = item
        elif item.get("kind") == "run_summary":
            if summary is not None:
                raise ValueError(f"{path}:{line_no}: duplicate run_summary")
            summary = item
        elif item.get("kind") == "wallet_stats":
            wallet = item.get("wallet") or {}
            if wallet.get("chain") != "bsc" or not wallet.get("address"):
                raise ValueError(f"{path}:{line_no}: expected a BSC wallet address")
            if item.get("status") not in VALID_STATUSES:
                raise ValueError(f"{path}:{line_no}: unknown wallet status {item.get('status')!r}")
            records.append(item)
        else:
            raise ValueError(f"{path}:{line_no}: unknown record kind {item.get('kind')!r}")
    if not meta or not summary:
        raise ValueError(f"{path}: run_meta and final run_summary are required")
    if json.loads(lines[-1]).get("kind") != "run_summary":
        raise ValueError(f"{path}: run_summary must be the final record")
    if (meta.get("scope") or {}).get("chain") != "bsc":
        raise ValueError(f"{path}: run_meta scope is not BSC")
    if summary.get("run_id") != meta.get("run_id") or summary.get("records") != len(records):
        raise ValueError(f"{path}: footer does not match run metadata and wallet records")
    if meta.get("input_wallet_count") != len(records):
        raise ValueError(f"{path}: input_wallet_count does not match wallet records")
    footer = {(x["wallet"]["address"].lower(), x.get("status")) for x in summary.get("wallets", [])}
    actual = {(x["wallet"]["address"].lower(), x.get("status")) for x in records}
    if footer != actual:
        raise ValueError(f"{path}: footer wallet list does not match wallet records")
    if len({x["wallet"]["address"].lower() for x in records}) != len(records):
        raise ValueError(f"{path}: duplicate wallet_stats records")
    return meta, summary, records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidates", help="human_like candidate CSV")
    parser.add_argument("stats", nargs="+", help="one or more wallet-stats JSONL sources")
    parser.add_argument("output_base", help="output path without extension")
    args = parser.parse_args()

    candidates = list(csv.DictReader(open(args.candidates, newline="", encoding="utf-8")))
    if not candidates or not {"wallet", "class"}.issubset(candidates[0]):
        parser.error("candidate CSV must have rows plus wallet and class columns")
    candidates = [r for r in candidates if r["class"] == "human_like"]
    if not candidates:
        parser.error("candidate CSV contains no human_like rows")
    if len({r["wallet"].lower() for r in candidates}) != len(candidates):
        parser.error("human_like candidate wallets must be unique")
    original_fields = list(candidates[0])
    meta_records = []
    chosen = {}
    for index, path in enumerate(args.stats):
        meta, summary, records = load_source(path)
        meta_records.append({"source_id": index, "path": str(Path(path)),
                             "sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                             "run_meta": meta, "run_summary": summary})
        for record in records:
            wallet = record["wallet"]["address"].lower()
            previous = chosen.get(wallet)
            is_good = record["status"] in GOOD_STATUSES
            previous_good = previous and previous["record"]["status"] in GOOD_STATUSES
            is_later = meta.get("captured_at", "") >= previous["meta"].get("captured_at", "") if previous else True
            if not previous or (is_good and not previous_good) or (is_good == previous_good and is_later):
                chosen[wallet] = {"record": record, "source_id": index, "meta": meta}

    rows = []
    for candidate in candidates:
        wallet = candidate["wallet"].lower()
        selected = chosen.get(wallet)
        record = selected["record"] if selected else {}
        meta = selected["meta"] if selected else {}
        out = dict(candidate)
        for field, path in FIELDS.items():
            out[field] = get(record, path)
        win_rate = out.pop("pnl30d_win_rate_known", None)
        out["pnl30d_win_rate_known_pct"] = (str((Decimal(win_rate) * 100).quantize(Decimal("0.01"), rounding=ROUND_HALF_EVEN))
                                                  if win_rate is not None else None)
        window = meta.get("window", {})
        for field, key in (("since", "since"), ("until", "until"), ("as_of", "as_of")):
            out[f"pnl30d_{field}"] = window.get(key)
        out.update({"pnl30d_captured_at": meta.get("captured_at"),
                    "pnl30d_run_id": meta.get("run_id"),
                    "pnl30d_source_id": selected["source_id"] if selected else None,
                    "pnl30d_record": record or None})
        rows.append(out)

    def sort_key(row):
        value = row["pnl30d_realized_net_usd"]
        if value in (None, ""):
            return (1, Decimal(0), row["wallet"].lower())
        try:
            amount = Decimal(value)
        except (InvalidOperation, TypeError):
            raise ValueError(f"invalid exact PnL value for {row['wallet']}: {value!r}")
        if not amount.is_finite():
            raise ValueError(f"non-finite exact PnL value for {row['wallet']}: {value!r}")
        return (0, amount.copy_negate(), row["wallet"].lower())

    rows.sort(key=sort_key)
    output_base = Path(args.output_base)
    fields = original_fields + [name for name in rows[0] if name not in original_fields]
    with output_base.with_suffix(".csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows({key: json.dumps(value, sort_keys=True, separators=(",", ":"))
                           if isinstance(value, (dict, list)) else value for key, value in row.items()}
                          for row in rows)
    output_base.with_suffix(".json").write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    manifest = {"schema_version": 1, "generator_version": "bsc-pnl-table/1",
                "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "scope": "bsc / human_like candidates", "sort": "realized_net_pnl decimal DESC; unknown last; wallet ASC",
                "caveat": "Exploratory known-subset realized PnL; mixed per-wallet windows are recorded on each row. Not wallet-rank quality gates.",
                "source_software_versions": [s["run_meta"].get("software_version") for s in meta_records],
                "source_finality": [s["run_meta"].get("finality") for s in meta_records],
                "source_registry_versions": [s["run_meta"].get("registry_version") for s in meta_records],
                "metadata_note": "Null source versions/finality were not recorded by the source CLI; full decoder scope, ledger and price policies are preserved in sources.",
                "candidate_source": str(Path(args.candidates)), "candidate_sha256": hashlib.sha256(Path(args.candidates).read_bytes()).hexdigest(),
                "sources": meta_records, "rows": len(rows)}
    output_base.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
