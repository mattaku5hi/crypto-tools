import json
import argparse
import csv
import stat
import tempfile
import unittest
from decimal import Decimal
from pathlib import Path

from axiom_pnl import (
    InputError,
    ResumeError,
    StopCollection,
    BrowserAdapter,
    bind_input_identity,
    classify_amount,
    command_collect,
    has_css_token,
    load_manifest,
    connect_cdp,
    evaluate_snapshot,
    extract_performance_ui,
    normalize_wallet,
    wallet_tab_needs_click,
    search_results_ready,
    map_wallet_metrics_payload,
    merge_metrics_snapshot,
    parse_amount,
    parse_ui_amount,
    read_wallets,
    validate_cdp_url,
    write_outputs,
)


class MoneyTests(unittest.TestCase):
    def test_exact_thresholds_and_rounded_boundary_are_distinguished(self):
        self.assertEqual(classify_amount(parse_amount("10000"), Decimal("10000")), "PASS")
        self.assertEqual(classify_amount(parse_ui_amount("$10000"), Decimal("10000")), "UNKNOWN")
        self.assertEqual(classify_amount(parse_ui_amount("$10001"), Decimal("10000")), "PASS")
        self.assertEqual(classify_amount(parse_amount("9.9K"), Decimal("10000")), "FAIL")
        self.assertEqual(classify_amount(parse_amount("9.99K"), Decimal("10000")), "FAIL")
        self.assertEqual(classify_amount(parse_amount("9.9999K"), Decimal("10000")), "FAIL")
        self.assertEqual(classify_amount(parse_amount("10K"), Decimal("10000")), "UNKNOWN")
        self.assertEqual(parse_amount("9.99K").as_json()["raw"], "9.99K")

    def test_ui_rounding_uses_display_precision_for_signed_and_decimal_values(self):
        negative = parse_ui_amount("-$483.5")
        self.assertEqual((negative.lower, negative.upper), (Decimal("-483.55"), Decimal("-483.45")))
        sub_thousand = parse_ui_amount("+$10.45")
        self.assertEqual((sub_thousand.lower, sub_thousand.upper), (Decimal("10.445"), Decimal("10.455")))
        self.assertFalse(sub_thousand.exact)
        self.assertTrue(parse_amount("10.45").exact)

    def test_negative_values_remain_negative(self):
        amount = parse_amount("-$12.50")
        self.assertEqual(amount.lower, Decimal("-12.50"))
        self.assertEqual(classify_amount(amount, Decimal("0")), "FAIL")

    def test_selected_class_is_an_exact_whitespace_token(self):
        self.assertTrue(has_css_token("text-base text-primaryBlue", "text-primaryBlue"))
        self.assertFalse(has_css_token("hover:text-primaryBlue text-textSecondary", "text-primaryBlue"))

    def test_wallet_search_toggle_is_clicked_only_when_inactive(self):
        self.assertFalse(wallet_tab_needs_click("border-primaryBlue bg-primaryBlue/15 text-primaryBlue"))
        self.assertTrue(wallet_tab_needs_click("hover:text-primaryBlue text-textSecondary"))

    def test_history_cards_are_not_accepted_before_fresh_results_heading(self):
        self.assertFalse(search_results_ready(False, 1))
        self.assertFalse(search_results_ready(False, 2))
        self.assertFalse(search_results_ready(True, 2))
        self.assertTrue(search_results_ready(True, 1))

    def test_invalid_and_unknown_values_are_not_zero(self):
        with self.assertRaises(InputError):
            parse_amount(None)
        with self.assertRaises(InputError):
            parse_amount("—")


class WalletTests(unittest.TestCase):
    def test_cdp_attach_creates_owned_tab_without_launching_browser(self):
        class MockContext:
            pages = [object()]

            def new_page(self):
                return object()

        class MockBrowser:
            contexts = [MockContext()]

        class MockChromium:
            def __init__(self):
                self.connected_to = None

            def connect_over_cdp(self, url, timeout):
                self.connected_to = (url, timeout)
                return MockBrowser()

        class MockPlaywright:
            chromium = MockChromium()

        playwright = MockPlaywright()
        browser, page, created = connect_cdp(playwright, "http://127.0.0.1:9225")
        self.assertEqual(len(browser.contexts), 1)
        self.assertIsNot(page, browser.contexts[0].pages[0])
        self.assertTrue(created)
        self.assertEqual(playwright.chromium.connected_to, ("http://127.0.0.1:9225", 10_000))

    def test_chain_scoped_validation_and_dedupe_preserve_order_metadata(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "wallets.json"
            path.write_text(json.dumps([
                {"address": "0x" + "A" * 40, "chain": "bsc", "label": "first"},
                {"wallet": "0x" + "a" * 40, "chain": "bsc", "label": "duplicate"},
                {"trackedWalletAddress": "11111111111111111111111111111111", "chain": "solana", "label": "other chain"},
            ]))
            rows = read_wallets(path, None)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["label"], "first")
        self.assertEqual(rows[1]["chain"], "solana")
        self.assertEqual(rows[0]["wallet"], "0x" + "a" * 40)

    def test_solana_address_must_decode_to_32_bytes(self):
        valid = "11111111111111111111111111111111"
        self.assertEqual(len(__import__("axiom_pnl").decode_base58(valid)), 32)
        self.assertEqual(normalize_wallet(valid, "solana"), valid)
        with self.assertRaises(InputError):
            normalize_wallet("short", "solana")

    def test_metrics_request_must_bind_exact_wallet_and_chain_host(self):
        class Request:
            method = "POST"
            url = "https://bnb-api1.axiom.trade/portfolio"
            post_data = json.dumps({
                "walletAddresses": ["0x" + "a" * 40], "isOtherWallet": True,
                "timeOffsetMinutes": -180, "activesSort": "recent", "denomination": "usd",
            })

        wallet = {"chain": "bsc", "wallet": "0x" + "a" * 40}
        self.assertTrue(BrowserAdapter._matches_wallet_request(Request(), wallet))
        Request.post_data = json.dumps({
            "walletAddresses": ["0x" + "b" * 40], "isOtherWallet": True,
            "timeOffsetMinutes": -180, "activesSort": "recent", "denomination": "usd",
        })
        self.assertFalse(BrowserAdapter._matches_wallet_request(Request(), wallet))
        Request.post_data = json.dumps({
            "walletAddresses": [wallet["wallet"]], "isOtherWallet": True,
            "timeOffsetMinutes": -180, "activesSort": "recent", "denomination": "usd",
        })
        Request.url = "https://api10.axiom.trade/portfolio"
        self.assertFalse(BrowserAdapter._matches_wallet_request(Request(), wallet))
        sol_wallet = "11111111111111111111111111111111"
        Request.url = "https://api10.axiom.trade/portfolio"
        Request.post_data = json.dumps({
            "walletAddresses": [sol_wallet], "isOtherWallet": True,
            "timeOffsetMinutes": -180, "activesSort": "recent",
            "denomination": "usd",
        })
        self.assertTrue(BrowserAdapter._matches_wallet_request(Request(), {"chain": "solana", "wallet": sol_wallet}))
        Request.post_data = json.dumps({
            "walletAddresses": [sol_wallet], "isOtherWallet": True,
            "timeOffsetMinutes": -180, "activesSort": "recent", "denomination": "usd",
        })
        Request.url = "https://api10.axiom.trade/portfolio"
        self.assertTrue(BrowserAdapter._matches_wallet_request(Request(), {"chain": "solana", "wallet": sol_wallet}))
        Request.url = "https://api10.axiom.trade/wallet-metrics"
        self.assertFalse(BrowserAdapter._matches_wallet_request(Request(), {"chain": "solana", "wallet": sol_wallet}))

    def test_cdp_url_must_be_loopback(self):
        self.assertEqual(validate_cdp_url("http://127.0.0.1:9225"), "http://127.0.0.1:9225")
        with self.assertRaises(InputError):
            validate_cdp_url("http://example.com:9225")

    def test_evm_chain_is_required(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "wallets.json"
            path.write_text(json.dumps(["0x" + "a" * 40]))
            with self.assertRaises(InputError):
                read_wallets(path, None)


class MetricsPayloadTests(unittest.TestCase):
    def test_extracts_only_observed_exact_labels_from_performance_section(self):
        section = """Performance
30d Total PNL
+$85.1K
30d Realized PNL
+$85.5K
30d TXNS
123
Total Value
$999.0K"""
        self.assertEqual(extract_performance_ui(section, "30d"), {
            "pnl_30d_total": "+$85.1K",
            "pnl_30d_realized": "+$85.5K",
        })
        maximum = "Performance\nTotal Pnl\n+$438K\nRealized PNL\n+$438K"
        self.assertEqual(extract_performance_ui(maximum, "Max"), {
            "pnl_max_total": "+$438K",
            "pnl_max_realized": "+$438K",
        })
        with self.assertRaises(InputError):
            extract_performance_ui("Performance\nTotal Value\n$999K", "Max")

    def test_exact_api_realized_is_checked_against_rounded_ui_and_total_is_usd_ui(self):
        api = {
            "pnl_30d_realized": Decimal("12345.67"),
            "pnl_max_realized": Decimal("45678.90"),
            "source": "observed API",
            "provenance": {"native_token": "sol", "thirty_day_total_native": Decimal("781.15")},
        }
        ui = {
            "pnl_30d_total": "+$12.4K",
            "pnl_30d_realized": "+$12.3K",
            "pnl_max_total": "+$45.7K",
            "pnl_max_realized": "+$45.7K",
        }
        snapshot = merge_metrics_snapshot(api, ui)
        self.assertEqual(snapshot["pnl_30d_total"], "+$12.4K")
        self.assertEqual(snapshot["pnl_30d_realized"], Decimal("12345.67"))
        self.assertTrue(snapshot["source_consistent"])
        self.assertEqual(snapshot["raw_ui_values"], ui)
        api["pnl_max_realized"] = Decimal("50000")
        self.assertFalse(merge_metrics_snapshot(api, ui)["source_consistent"])

    def test_ui_realized_interval_accepts_only_values_within_shown_precision(self):
        api = {"pnl_30d_realized": Decimal("-483.4901385943016"), "pnl_max_realized": Decimal("10.454785507825228")}
        ui = {"pnl_30d_realized": "-$483.5", "pnl_max_realized": "+$10.45"}
        self.assertTrue(merge_metrics_snapshot(api, ui)["source_consistent"])
        api["pnl_30d_realized"] = Decimal("-483.6")
        self.assertFalse(merge_metrics_snapshot(api, ui)["source_consistent"])
        api["pnl_30d_realized"] = Decimal("548.949")
        ui["pnl_30d_realized"] = "+$542.6"
        self.assertFalse(merge_metrics_snapshot(api, ui)["source_consistent"])

    def test_maps_only_verified_realized_usd_not_native_total(self):
        payload = {
            "portfolio": {
                "performanceMetrics": {
                    "oneDay": {},
                    "sevenDay": {},
                    "thirtyDay": {"totalPnl": Decimal("781.15"), "realizedUsdPnl": Decimal("12345.67"), "nativeToken": "sol"},
                    "allTime": {"totalPnl": Decimal("4415.66"), "realizedUsdPnl": Decimal("45678.90"), "nativeToken": "sol"},
                },
            },
        }
        snapshot = map_wallet_metrics_payload(payload)
        self.assertIsNone(snapshot["pnl_30d_total"])
        self.assertIsNone(snapshot["pnl_max_total"])
        self.assertEqual(snapshot["pnl_30d_realized"], Decimal("12345.67"))
        self.assertEqual(snapshot["pnl_max_realized"], Decimal("45678.90"))
        self.assertEqual(snapshot["provenance"]["thirty_day_total_native"], Decimal("781.15"))
        self.assertIsNone(snapshot["wallet_match"])
        self.assertIsNone(snapshot["period_match"])

class EvaluationTests(unittest.TestCase):
    def test_all_four_conditions_required_and_unknown_is_preserved(self):
        wallet = {"chain": "bsc", "wallet": "0x" + "a" * 40}
        base = {
            "wallet_match": True,
            "period_match": True,
            "pnl_30d_total": "10001",
            "pnl_30d_realized": "10000",
            "pnl_max_total": "30001",
            "pnl_max_realized": "30000",
        }
        passed = evaluate_snapshot(wallet, base, "2026-10-12T00:00:00Z")
        self.assertEqual(passed["status"], "PASS")
        base["pnl_max_realized"] = "29999"
        self.assertEqual(evaluate_snapshot(wallet, base, "now")["status"], "FAIL")
        base["pnl_max_realized"] = "30K"
        row = evaluate_snapshot(wallet, base, "now")
        self.assertEqual(row["status"], "UNKNOWN")
        self.assertEqual(row["pnl_max_realized"]["raw"], "30K")

    def test_wallet_and_period_must_be_confirmed_before_accepting_values(self):
        snapshot = {"wallet_match": False, "period_match": True, **{key: "999999" for key, _ in __import__("axiom_pnl").METRICS}}
        row = evaluate_snapshot({}, snapshot, "now")
        self.assertEqual(row["status"], "UNKNOWN")
        self.assertIn("wallet", row["reason"])
        self.assertEqual(row["pnl_30d_total"]["raw"], "999999")

    def test_outputs_include_failed_and_unknown_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            rows = [{"wallet": "a", "status": "PASS"}, {"wallet": "b", "status": "FAIL"}, {"wallet": "c", "status": "UNKNOWN"}]
            write_outputs(out, rows, {"as_of": "now"})
            all_rows = (out / "all.jsonl").read_text().splitlines()
            passed = json.loads((out / "passed.json").read_text())
        self.assertEqual(len(all_rows), 3)
        self.assertEqual([row["wallet"] for row in passed], ["a"])

    def test_csv_flattens_metric_interval_for_manual_review(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            row = {
                "wallet": "x", "status": "UNKNOWN",
                "pnl_30d_total": {"raw": "+$10K", "lower": "9500", "upper_exclusive": "10500", "exact": False},
            }
            write_outputs(out, [row], {"as_of": "now"}, planned_count=1)
            with (out / "all.csv").open(newline="", encoding="utf-8") as source:
                result = next(csv.DictReader(source))
        self.assertEqual(result["pnl_30d_total_raw"], "+$10K")
        self.assertEqual(result["pnl_30d_total_lower_usd"], "9500")
        self.assertEqual(result["pnl_30d_total_upper_exclusive_usd"], "10500")
        self.assertEqual(result["pnl_30d_total_exact"], "False")

    def test_resume_reuses_manifest_policy_and_rejects_changed_chain_or_as_of(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "private-run"
            manifest = load_manifest(out, None, "bsc")
            manifest["planned_wallet_count"] = 1
            write_outputs(out, [{"wallet": "x", "status": "UNKNOWN", "retryable": True}], manifest, planned_count=1)
            resumed = load_manifest(out, None, "bsc")
            self.assertEqual(resumed["as_of"], manifest["as_of"])
            with self.assertRaises(ResumeError):
                load_manifest(out, None, "solana")
            with self.assertRaises(ResumeError):
                load_manifest(out, "2026-10-13T00:00:00Z", "bsc")

    def test_resume_rejects_changed_wallet_identity_or_order(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "run"
            manifest = load_manifest(out, None, "bsc")
            wallets = [
                {"chain": "bsc", "wallet": "0x" + "a" * 40},
                {"chain": "bsc", "wallet": "0x" + "b" * 40},
            ]
            bind_input_identity(manifest, wallets)
            with self.assertRaises(ResumeError):
                bind_input_identity(manifest, list(reversed(wallets)))

    def test_completed_resume_reuses_results_without_browser(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            input_path = root / "wallets.json"
            out = root / "run"
            wallet = {"wallet": "0x" + "a" * 40, "chain": "bsc"}
            input_path.write_text(json.dumps([wallet]))
            manifest = load_manifest(out, None, "bsc")
            rows = read_wallets(input_path, "bsc")
            bind_input_identity(manifest, rows)
            row = {**rows[0], "status": "PASS", "retryable": False}
            write_outputs(out, [row], manifest, planned_count=1)
            args = argparse.Namespace(
                cdp_url="http://127.0.0.1:9225", input=str(input_path), out=str(out),
                chain="bsc", as_of=None, resume=True,
            )
            self.assertEqual(command_collect(args), 0)
            self.assertEqual(json.loads((out / "manifest.json").read_text())["status"], "complete")

    def test_outputs_are_private_atomic_and_incomplete_for_unprocessed_wallets(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "private-run"
            manifest = {"as_of": "now"}
            write_outputs(out, [{"wallet": "x", "status": "UNKNOWN", "retryable": True}], manifest, planned_count=2)
            self.assertEqual(stat.S_IMODE(out.stat().st_mode), 0o700)
            for name in ("manifest.json", "all.jsonl", "all.csv", "passed.json", "passed.csv"):
                self.assertEqual(stat.S_IMODE((out / name).stat().st_mode), 0o600)
            self.assertEqual(json.loads((out / "manifest.json").read_text())["status"], "in_progress")
            self.assertFalse(list(out.glob("*.tmp")))

    def test_http_rate_limit_stops_with_safe_message(self):
        class MockPage:
            def set_default_timeout(self, timeout):
                self.timeout = timeout

            def on(self, event, callback):
                self.listener = (event, callback)

        class MockResponse:
            status = 429

        adapter = BrowserAdapter(MockPage())
        with self.assertRaises(StopCollection) as caught:
            adapter._map_response(MockResponse(), {"chain": "bsc"})
        self.assertIn("rate limit", str(caught.exception).lower())
        self.assertNotIn("http", str(caught.exception).lower())

    def test_native_identity_is_required_for_both_threshold_windows(self):
        payload = {
            "performanceMetrics": {
                "thirtyDay": {"realizedUsdPnl": Decimal("1"), "nativeToken": "sol"},
                "allTime": {"realizedUsdPnl": Decimal("2")},
            },
        }
        with self.assertRaises(InputError):
            map_wallet_metrics_payload(payload)

if __name__ == "__main__":
    unittest.main()
