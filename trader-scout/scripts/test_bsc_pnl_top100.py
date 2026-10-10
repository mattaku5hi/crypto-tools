import csv
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("bsc-pnl-top100.py")
SPEC = importlib.util.spec_from_file_location("bsc_pnl_top100", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BscPnlTop100Test(unittest.TestCase):
    def test_joins_100_sorts_exactly_and_records_tpd_exclusions(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            results = root / "inputs/results"
            results.mkdir(parents=True)
            candidates, human = [], []
            for i in range(100):
                wallet = f"0x{i:040x}"
                candidates.append({"rank": str(i + 1), "wallet": wallet, "source_field": f"kept-{i}"})
                status, days, buys, sells, pnl = "incomplete", 1, 50, 50, str(-i)
                if i == 0:
                    pnl = "-10000000000000000000000000000.1"
                elif i == 1:
                    pnl, buys = "-10000000000000000000000000000.0", 101
                elif i == 2:
                    status, days, pnl = "no_trade_activity", 0, "0"
                elif i == 3:
                    status, days, buys, sells, pnl = "error", None, None, None, None
                human.append({"wallet": wallet.upper(), "pnl30d_status": status,
                              "pnl30d_active_days": days, "pnl30d_buys": buys, "pnl30d_sells": sells,
                              "pnl30d_realized_net_usd": pnl, "pnl30d_run_id": f"run-{i % 2}",
                              "pnl30d_since": f"since-{i % 2}", "pnl30d_until": f"until-{i % 2}",
                              "pnl30d_as_of": f"asof-{i % 2}", "pnl30d_captured_at": f"captured-{i % 2}",
                              "pnl30d_coverage": {"complete": False, "reason": "fixture"},
                              "pnl30d_record": {"quality": {"unknown": 1}}})
            discovery_path = results / "bsc_leaderboard_top100_early_k2plus_with_curve.csv"
            with discovery_path.open("w", newline="") as file:
                writer = csv.DictWriter(file, fieldnames=list(candidates[0]))
                writer.writeheader()
                writer.writerows(candidates)
            (results / "bsc_k2_human_pnl30d_by_pnl.json").write_text(json.dumps(human))
            (results / "bsc_k2_human_pnl30d_by_pnl.manifest.json").write_text(json.dumps({"source": "fixture"}))

            MODULE.refresh(root)

            output = results / "bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_by_pnl"
            filtered = results / "bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_tpd100_by_pnl"
            rows = json.loads(output.with_suffix(".json").read_text())
            kept = json.loads(filtered.with_suffix(".json").read_text())
            manifest = json.loads(output.with_suffix(".manifest.json").read_text())
            self.assertEqual(len(rows), 100)
            self.assertEqual(len({row["wallet"].lower() for row in rows}), 100)
            self.assertEqual([row["pnl_rank"] for row in rows], list(range(1, 101)))
            self.assertEqual(rows[0]["wallet"], candidates[2]["wallet"])
            self.assertLess(next(r["pnl_rank"] for r in rows if r["wallet"] == candidates[1]["wallet"]),
                            next(r["pnl_rank"] for r in rows if r["wallet"] == candidates[0]["wallet"]))
            self.assertEqual(rows[0]["discovery_rank"], "3")
            self.assertEqual(rows[0]["source_field"], "kept-2")
            self.assertEqual(rows[0]["pnl30d_record"], {"quality": {"unknown": 1}})
            self.assertEqual(rows[0]["pnl30d_since"], "since-0")
            self.assertEqual(len(kept), 97)
            self.assertIn(candidates[0]["wallet"], {row["wallet"] for row in kept})  # exactly 100 trades/day
            self.assertNotIn(candidates[1]["wallet"], {row["wallet"] for row in kept})
            excluded = {item["wallet"]: item["reasons"] for item in manifest["excluded"]}
            self.assertIn("trades_per_active_day_gt_100", excluded[candidates[1]["wallet"]])
            self.assertIn("no_activity", excluded[candidates[2]["wallet"]])
            self.assertIn("scan_error", excluded[candidates[3]["wallet"]])
            self.assertFalse(manifest["quality_gates_applied"])


if __name__ == "__main__":
    unittest.main()
