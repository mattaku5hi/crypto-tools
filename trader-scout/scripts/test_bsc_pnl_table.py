import csv
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("bsc-pnl-table.py")


def source(path, run_id, captured_at, records):
    meta = {"kind": "run_meta", "run_id": run_id, "captured_at": captured_at,
            "input_wallet_count": len(records), "scope": {"chain": "bsc"},
            "window": {"since": f"{run_id}-since", "until": f"{run_id}-until", "as_of": f"{run_id}-asof"}}
    summary = {"kind": "run_summary", "run_id": run_id, "records": len(records),
               "wallets": [{"wallet": r["wallet"], "status": r["status"]} for r in records]}
    path.write_text("\n".join(json.dumps(row) for row in [meta, *records, summary]) + "\n")


def wallet(address, status, pnl=None):
    stats = {"usd": {"realized_net_pnl": {"decimal": pnl},
                      "realized_pnl_lower_bound": {"decimal": None, "status": "unbounded"},
                      "win_rate": {"value": "0.51234567"},
                      "win_rate_lower_bound": {"percent_2dp": "45.67"}},
             "trades": {"buys": 4, "sells": 3},
             "activity": {"active_utc_days": 2, "trades_per_active_day": {"display_2dp": "3.50"}},
             "evm_open_valuation": {"value_usd": None}}
    return {"kind": "wallet_stats", "wallet": {"address": address, "chain": "bsc"},
            "status": status, "error": "fixture error" if status == "error" else None,
            "coverage": {"complete": status == "ok", "transactions_scanned": 12},
            "stats": stats if status not in ("error", "not_scanned") else None}


class BscPnlTableTest(unittest.TestCase):
    def run_script(self, candidates, sources, output):
        return subprocess.run([sys.executable, str(SCRIPT), str(candidates),
                               *map(str, sources), str(output)], capture_output=True, text=True)

    def test_merges_success_over_error_and_sorts_exact_known_values_before_missing(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            candidates = root / "candidates.csv"
            with candidates.open("w", newline="") as f:
                writer = csv.DictWriter(f, fieldnames=["wallet", "class", "curve"])
                writer.writeheader()
                writer.writerows([
                    {"wallet": "0xA", "class": "human_like", "curve": "kept"},
                    {"wallet": "0xB", "class": "human_like", "curve": "kept"},
                    {"wallet": "0xC", "class": "human_like", "curve": "missing"},
                    {"wallet": "0xZ", "class": "bot", "curve": "filtered"},
                ])
            old, new = root / "old.jsonl", root / "new.jsonl"
            source(old, "old", "2026-10-09T00:00:00Z", [
                wallet("0xa", "error"), wallet("0xb", "ok", "-10000000000000000000000000000.0")])
            source(new, "new", "2026-10-10T00:00:00Z", [
                wallet("0xa", "incomplete", "-10000000000000000000000000000.1"), wallet("0xb", "error")])
            output = root / "table"
            result = self.run_script(candidates, [old, new], output)
            self.assertEqual(result.returncode, 0, result.stderr)
            with output.with_suffix(".csv").open(newline="") as f:
                rows = list(csv.DictReader(f))
            self.assertEqual([r["wallet"] for r in rows], ["0xB", "0xA", "0xC"])
            self.assertEqual([r["curve"] for r in rows], ["kept", "kept", "missing"])
            self.assertEqual(rows[0]["pnl30d_realized_net_usd"], "-10000000000000000000000000000.0")
            self.assertEqual(rows[0]["pnl30d_run_id"], "old")
            self.assertEqual(rows[0]["pnl30d_error"], "")
            self.assertEqual(rows[1]["pnl30d_realized_net_usd"], "-10000000000000000000000000000.1")
            self.assertEqual(rows[1]["pnl30d_run_id"], "new")
            self.assertEqual(rows[1]["pnl30d_since"], "new-since")
            self.assertEqual(json.loads(rows[1]["pnl30d_coverage"]), {"complete": False, "transactions_scanned": 12})
            self.assertEqual(rows[1]["pnl30d_win_rate_known_pct"], "51.23")
            self.assertEqual(rows[1]["pnl30d_win_rate_lower_bound_pct"], "45.67")
            self.assertEqual(rows[1]["pnl30d_realized_lower_bound_status"], "unbounded")
            self.assertEqual(rows[1]["pnl30d_trades_per_day"], "3.50")
            self.assertEqual(rows[2]["pnl30d_realized_net_usd"], "")
            json_rows = json.loads(output.with_suffix(".json").read_text())
            self.assertEqual([r["wallet"] for r in json_rows], ["0xB", "0xA", "0xC"])
            self.assertEqual(json_rows[1]["pnl30d_coverage"], {"complete": False, "transactions_scanned": 12})
            self.assertEqual(json_rows[1]["pnl30d_record"]["error"], None)
            self.assertIsNone(json_rows[2]["pnl30d_realized_net_usd"])
            manifest = json.loads(output.with_suffix(".manifest.json").read_text())
            self.assertEqual([s["run_meta"]["run_id"] for s in manifest["sources"]], ["old", "new"])
            self.assertIn("mixed per-wallet windows", manifest["caveat"])

    def test_rejects_footer_that_does_not_match_wallet_records(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            candidates = root / "candidates.csv"
            candidates.write_text("wallet,class\n0xa,human_like\n")
            bad = root / "bad.jsonl"
            source(bad, "run", "2026-10-10T00:00:00Z", [wallet("0xa", "ok", "1")])
            lines = bad.read_text().splitlines()
            footer = json.loads(lines[-1])
            footer["records"] = 2
            lines[-1] = json.dumps(footer)
            bad.write_text("\n".join(lines) + "\n")
            result = self.run_script(candidates, [bad], root / "table")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("footer does not match", result.stderr)

    def test_requires_first_run_meta_known_record_kinds_and_human_like_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            candidates = root / "candidates.csv"
            candidates.write_text("wallet,class\n0xa,human_like\n")
            valid = root / "valid.jsonl"
            source(valid, "run", "2026-10-10T00:00:00Z", [wallet("0xa", "ok", "1")])
            lines = valid.read_text().splitlines()
            first_meta = root / "meta-not-first.jsonl"
            first_meta.write_text("\n".join(lines[1:2] + lines[:1] + lines[2:]) + "\n")
            result = self.run_script(candidates, [first_meta], root / "first")
            self.assertIn("run_meta must be the first record", result.stderr)
            unknown = root / "unknown-kind.jsonl"
            unknown.write_text("\n".join(lines[:1] + ['{"kind":"unexpected"}'] + lines[1:]) + "\n")
            result = self.run_script(candidates, [unknown], root / "unknown")
            self.assertIn("unknown record kind", result.stderr)
            candidates.write_text("wallet,class\n0xa,bot\n")
            result = self.run_script(candidates, [valid], root / "empty")
            self.assertIn("contains no human_like rows", result.stderr)

    def test_not_scanned_does_not_replace_previous_success(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            candidates = root / "candidates.csv"
            candidates.write_text("wallet,class\n0xa,human_like\n")
            old, new = root / "old.jsonl", root / "new.jsonl"
            source(old, "old", "2026-10-09T00:00:00Z", [wallet("0xa", "ok", "3")])
            source(new, "new", "2026-10-10T00:00:00Z", [wallet("0xa", "not_scanned")])
            output = root / "table"
            result = self.run_script(candidates, [old, new], output)
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = json.loads(output.with_suffix(".json").read_text())
            self.assertEqual(rows[0]["pnl30d_run_id"], "old")
            self.assertEqual(rows[0]["pnl30d_realized_net_usd"], "3")


if __name__ == "__main__":
    unittest.main()
