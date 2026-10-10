#!/usr/bin/env python3
"""Refresh PnL-sorted tables for the BSC top-100 discovery selection."""
import csv
import hashlib
import json
from decimal import Decimal
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def write(base, rows, fields):
    with base.with_suffix(".csv").open("w", newline="", encoding="utf-8") as file:
        writer = csv.DictWriter(file, fieldnames=fields)
        writer.writeheader()
        writer.writerows({k: json.dumps(v, sort_keys=True, separators=(",", ":"))
                          if isinstance(v, (dict, list)) else v for k, v in row.items()} for row in rows)
    base.with_suffix(".json").write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")


def refresh(root=ROOT):
    results = Path(root) / "inputs/results"
    discovery_path = results / "bsc_leaderboard_top100_early_k2plus_with_curve.csv"
    human_path = results / "bsc_k2_human_pnl30d_by_pnl.json"
    human_manifest_path = results / "bsc_k2_human_pnl30d_by_pnl.manifest.json"
    output = results / "bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_by_pnl"
    filtered_output = results / "bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_tpd100_by_pnl"
    with discovery_path.open(newline="", encoding="utf-8") as file:
        candidates = list(csv.DictReader(file))
    human = json.loads(human_path.read_text())
    human_manifest = json.loads(human_manifest_path.read_text())
    wallets = [row["wallet"].lower() for row in candidates]
    stats = {row["wallet"].lower(): row for row in human}
    if len(candidates) != 100 or len(set(wallets)) != 100 or len(stats) != len(human):
        raise ValueError("expected 100 unique discovery wallets and unique human stats")
    if not all(wallet in stats for wallet in wallets):
        raise ValueError("discovery wallet missing from human stats")

    rows = []
    for candidate in candidates:
        joined = dict(candidate)
        joined["discovery_rank"] = candidate["rank"]
        joined.update({k: v for k, v in stats[candidate["wallet"].lower()].items() if k.startswith("pnl30d_")})
        rows.append(joined)
    rows.sort(key=lambda row: (row["pnl30d_realized_net_usd"] in (None, ""),
                               Decimal(row["pnl30d_realized_net_usd"]).copy_negate()
                               if row["pnl30d_realized_net_usd"] not in (None, "") else Decimal(0),
                               row["wallet"].lower()))
    for rank, row in enumerate(rows, 1):
        row["pnl_rank"] = rank
    fields = list(candidates[0]) + ["discovery_rank"]
    fields += [key for key in rows[0] if key.startswith("pnl30d_")] + ["pnl_rank"]
    write(output, rows, fields)

    filtered, exclusions = [], []
    for row in rows:
        status = row.get("pnl30d_status")
        days, buys, sells = (row.get(f"pnl30d_{key}") for key in ("active_days", "buys", "sells"))
        days, buys, sells = (int(value) if value not in (None, "") else None for value in (days, buys, sells))
        reasons = []
        if status == "error": reasons.append("scan_error")
        if status == "not_scanned": reasons.append("not_scanned")
        if status in ("no_activity", "no_pump_activity", "no_trade_activity"): reasons.append("no_activity")
        if days is None: reasons.append("active_days_unknown")
        elif days <= 0: reasons.append("active_days_not_positive")
        if buys is None or sells is None: reasons.append("trade_count_unknown")
        elif days and days > 0 and buys + sells > 100 * days: reasons.append("trades_per_active_day_gt_100")
        if reasons:
            exclusions.append({"wallet": row["wallet"], "discovery_rank": row["discovery_rank"],
                               "pnl_rank": row["pnl_rank"], "reasons": reasons})
        else:
            filtered.append(row)
    write(filtered_output, filtered, fields)

    sources = (discovery_path, human_path, human_manifest_path)
    manifest = {
        "generator_version": "bsc-pnl-top100/1",
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "scope": "BSC discovery selection: original top 100 early_k2plus rows",
        "quality_gates_applied": False,
        "sort": "realized_net_pnl exact Decimal DESC; unavailable last; wallet ASC",
        "tpd100_filter": "active_days > 0 and (buys + sells) <= 100 * active_days; exact integer counts",
        "windows": "per-row pnl30d_since/until/as_of/captured_at/run_id preserved from human table",
        "sources": [{"file": str(path.relative_to(root)),
                     "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for path in sources],
        "raw_human_table_manifest": human_manifest,
        "full_table": {"csv": str(output.relative_to(root)) + ".csv",
                       "json": str(output.relative_to(root)) + ".json", "rows": len(rows)},
        "tpd100_table": {"csv": str(filtered_output.relative_to(root)) + ".csv",
                         "json": str(filtered_output.relative_to(root)) + ".json", "rows": len(filtered)},
        "excluded": exclusions,
    }
    output.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    refresh()
