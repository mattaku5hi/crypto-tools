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
    rescore_existing_row,
    extract_performance_ui,
    normalize_wallet,
    native_token_matches_chain,
    wallet_tab_needs_click,
    search_results_ready,
    values_are_stable,
    map_wallet_metrics_payload,
    merge_metrics_snapshot,
    parse_amount,
    parse_ui_amount,
    threshold_metrics,
    read_wallets,
    validate_cdp_url,
    validate_delay_seconds,
    validate_paced_wallets,
    delay_after_wallet,
    update_successful_wallet_count,
    write_outputs,
)


class MoneyTests(unittest.TestCase):
    def test_delay_bounds_are_finite_and_inclusive(self):
        self.assertEqual(validate_delay_seconds("0"), 0.0)
        self.assertEqual(validate_delay_seconds("2"), 2.0)
        self.assertEqual(validate_delay_seconds("120"), 120.0)
        for bad in ("-0.01", "120.01", "nan", "inf", "nope"):
            with self.subTest(bad=bad), self.assertRaises(InputError):
                validate_delay_seconds(bad)

    def test_paced_wallet_bounds_and_success_only_warmup(self):
        self.assertEqual(validate_paced_wallets("0"), 0)
        self.assertEqual(validate_paced_wallets("100000"), 100000)
        for bad in ("-1", "100001", "1.5", "nope"):
            with self.subTest(bad=bad), self.assertRaises(InputError):
                validate_paced_wallets(bad)
        self.assertEqual(delay_after_wallet(3.0, 0, 100), 3.0)
        self.assertEqual(delay_after_wallet(0.0, 20, 0), 0.0)
        successful = 19
        self.assertEqual(delay_after_wallet(3.0, 20, successful), 3.0)
        successful = update_successful_wallet_count(successful, "UNKNOWN")
        self.assertEqual(successful, 19)
        self.assertEqual(delay_after_wallet(3.0, 20, successful), 3.0)
        successful = update_successful_wallet_count(successful, "PASS")
        self.assertEqual(successful, 20)
        self.assertEqual(delay_after_wallet(3.0, 20, successful), 0.0)

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

    def test_thresholds_are_exact_finite_positive_decimals(self):
        thresholds = dict(threshold_metrics("5000.00", "15000"))
        self.assertEqual(thresholds["pnl_30d_total"], Decimal("5000.00"))
        self.assertEqual(thresholds["pnl_max_realized"], Decimal("15000"))
        for bad in ("0", "-1", "NaN", "Infinity", "not-money"):
            with self.subTest(bad=bad), self.assertRaises(InputError):
                threshold_metrics(bad, "15000")

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

    def test_period_values_require_two_consecutive_fresh_observations(self):
        stale = {"pnl_30d_total": "$1K", "pnl_30d_realized": "$2K"}
        fresh = {"pnl_30d_total": "$12K", "pnl_30d_realized": "$13K"}
        self.assertFalse(values_are_stable(None, stale))
        self.assertFalse(values_are_stable(stale, fresh))
        self.assertTrue(values_are_stable(fresh, fresh.copy()))

    def test_period_reader_ignores_first_stale_values_until_fresh_values_repeat(self):
        stale_text = "Performance\n30d Total PNL\n$1K\n30d Realized PNL\n$2K"
        fresh_text = "Performance\n30d Total PNL\n$10.1K\n30d Realized PNL\n$10.2K"

        class Button:
            def count(self):
                return 1

            def click(self):
                pass

            def get_attribute(self, name):
                return "text-primaryBlue"

        class Section:
            def __init__(self):
                self.texts = [stale_text, fresh_text, fresh_text]

            def inner_text(self):
                return self.texts.pop(0)

        class PerformanceLocator:
            def __init__(self, section):
                self.section = section

            def locator(self, selector):
                return self.section

        class Page:
            def __init__(self):
                self.section = Section()

            def set_default_timeout(self, timeout):
                pass

            def on(self, event, callback):
                pass

            def get_by_role(self, role, name, exact):
                return Button()

            def get_by_text(self, text, exact):
                return PerformanceLocator(self.section)

            def wait_for_timeout(self, timeout):
                pass

        values = BrowserAdapter(Page())._read_period("30d")
        self.assertEqual(values, {"pnl_30d_total": "$10.1K", "pnl_30d_realized": "$10.2K"})

    def test_invalid_and_unknown_values_are_not_zero(self):
        with self.assertRaises(InputError):
            parse_amount(None)
        with self.assertRaises(InputError):
            parse_amount("—")


class WalletTests(unittest.TestCase):
    def test_owned_tab_reuses_navigation_and_closes_two_overlays_before_next_search(self):
        class MockKeyboard:
            def __init__(self):
                self.presses = []

            def press(self, key):
                self.presses.append(key)

        class MockPage:
            def __init__(self):
                self.keyboard = MockKeyboard()
                self.navigations = []

            def set_default_timeout(self, timeout):
                pass

            def on(self, event, callback):
                pass

            def goto(self, url, **kwargs):
                self.navigations.append((url, kwargs))

        page = MockPage()
        adapter = BrowserAdapter(page)
        adapter._prepare_page_for_search()
        adapter._prepare_page_for_search()
        self.assertEqual(len(page.navigations), 1)
        self.assertEqual(page.keyboard.presses, ["Escape", "Escape"])

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
        with self.assertRaises(InputError):
            extract_performance_ui("Performance\nTotal Pnl\n10000\nRealized PNL\n$31000", "Max")

    def test_api_realized_is_retained_as_diagnostic_while_ui_is_authoritative(self):
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
        self.assertEqual(snapshot["pnl_30d_realized"], "+$12.3K")
        self.assertTrue(snapshot["api_ui_consistent"])
        self.assertEqual(snapshot["provenance"]["api_realized_usd_pnl"]["30d"], Decimal("12345.67"))
        self.assertEqual(snapshot["raw_ui_values"], ui)
        api["pnl_max_realized"] = Decimal("50000")
        self.assertFalse(merge_metrics_snapshot(api, ui)["api_ui_consistent"])

    def test_ui_realized_interval_accepts_only_values_within_shown_precision(self):
        api = {"pnl_30d_realized": Decimal("-483.4901385943016"), "pnl_max_realized": Decimal("10.454785507825228")}
        ui = {"pnl_30d_realized": "-$483.5", "pnl_max_realized": "+$10.45"}
        self.assertTrue(merge_metrics_snapshot(api, ui)["api_ui_consistent"])
        api["pnl_30d_realized"] = Decimal("-483.6")
        self.assertFalse(merge_metrics_snapshot(api, ui)["api_ui_consistent"])

    def test_api_ui_disagreement_does_not_override_displayed_ui_gate_status(self):
        api = {"wallet_match": True, "period_match": True, "pnl_30d_realized": Decimal("548.949"), "pnl_max_realized": Decimal("1")}
        ui = {
            "pnl_30d_total": "+$10.1K", "pnl_30d_realized": "+$10.1K",
            "pnl_max_total": "+$30.1K", "pnl_max_realized": "+$30.1K",
        }
        snapshot = merge_metrics_snapshot(api, ui)
        self.assertFalse(snapshot["api_ui_consistent"])
        self.assertEqual(evaluate_snapshot({"wallet": "w"}, snapshot, "now")["status"], "PASS")
        api["pnl_30d_realized"] = Decimal("548.949")
        ui["pnl_30d_realized"] = "+$542.6"
        diagnostic = merge_metrics_snapshot(api, ui)
        self.assertFalse(diagnostic["api_ui_consistent"])
        snapshot = {
            "wallet_match": True, "period_match": True, "api_ui_consistent": False,
            "pnl_30d_total": "+$10.1K", "pnl_30d_realized": "+$10.1K",
            "pnl_max_total": "+$30.1K", "pnl_max_realized": "+$30.1K",
        }
        self.assertEqual(evaluate_snapshot({"wallet": "w"}, snapshot, "now")["status"], "PASS")

    def test_api_mapping_retains_only_native_identity_and_realized_usd_diagnostic(self):
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
        self.assertNotIn("pnl_30d_total", snapshot)
        self.assertNotIn("pnl_30d_realized", snapshot)
        self.assertEqual(snapshot["provenance"]["api_realized_usd_pnl"]["30d"], Decimal("12345.67"))
        self.assertEqual(snapshot["provenance"]["api_realized_usd_pnl"]["max"], Decimal("45678.90"))
        self.assertEqual(snapshot["provenance"]["native_token"], "sol")
        self.assertNotIn("thirty_day_total_native", snapshot["provenance"])
        self.assertIsNone(snapshot["wallet_match"])
        self.assertIsNone(snapshot["period_match"])

    def test_native_token_must_match_explicit_network(self):
        self.assertTrue(native_token_matches_chain("solana", "sol"))
        self.assertTrue(native_token_matches_chain("bsc", "bnb"))
        self.assertFalse(native_token_matches_chain("bsc", "sol"))

class EvaluationTests(unittest.TestCase):
    def test_custom_threshold_boundaries_apply_to_all_four_metrics(self):
        thresholds = threshold_metrics("5000", "15000")
        wallet = {"wallet": "w", "chain": "bsc"}
        snapshot = {
            "wallet_match": True, "period_match": True,
            "pnl_30d_total": "$5000", "pnl_30d_realized": "$5000",
            "pnl_max_total": "$15000", "pnl_max_realized": "$15000",
        }
        row = evaluate_snapshot(wallet, snapshot, "now", thresholds)
        self.assertEqual(row["pnl_30d_total_status"], "UNKNOWN")
        self.assertEqual(row["pnl_30d_realized_status"], "UNKNOWN")
        self.assertEqual(row["pnl_max_total_status"], "UNKNOWN")
        self.assertEqual(row["pnl_max_realized_status"], "UNKNOWN")
        snapshot["pnl_30d_total"] = "$5001"
        snapshot["pnl_30d_realized"] = "$5001"
        snapshot["pnl_max_total"] = "$15001"
        snapshot["pnl_max_realized"] = "$15001"
        self.assertEqual(evaluate_snapshot(wallet, snapshot, "now", thresholds)["status"], "PASS")

    def test_saved_rows_can_be_rescored_without_losing_evidence(self):
        wallet = {"wallet": "w", "chain": "bsc", "label": "keep"}
        ui = {
            "pnl_30d_total": "$5.1K", "pnl_30d_realized": "$5.1K",
            "pnl_max_total": "$15.5K", "pnl_max_realized": "$15.5K",
        }
        initial = evaluate_snapshot(wallet, {
            "wallet_match": True, "period_match": True,
            "pnl_30d_total": ui["pnl_30d_total"], "pnl_30d_realized": ui["pnl_30d_realized"],
            "pnl_max_total": ui["pnl_max_total"], "pnl_max_realized": ui["pnl_max_realized"],
            "raw_ui_values": ui, "provenance": {"native_token": "bnb"},
        }, "2026-10-12T10:00:00Z")
        initial["pnl_30d_realized"] = {"raw": "5100.125", "lower": "5100.125", "upper_exclusive": "5100.125", "exact": True}
        initial["pnl_max_realized"] = {"raw": "15500.25", "lower": "15500.25", "upper_exclusive": "15500.25", "exact": True}
        initial["source_consistent"] = False
        rescored = rescore_existing_row(initial, threshold_metrics("5000", "15000"))
        self.assertEqual(rescored["status"], "PASS")
        self.assertEqual(rescored["wallet"], "w")
        self.assertEqual(rescored["label"], "keep")
        self.assertEqual(rescored["collected_at"], "2026-10-12T10:00:00Z")
        self.assertEqual(rescored["raw_ui_values"], ui)
        self.assertEqual(rescored["provenance"]["native_token"], "bnb")
        self.assertEqual(rescored["provenance"]["api_realized_usd_pnl"], {"30d": "5100.125", "max": "15500.25"})
        self.assertNotIn("source_consistent", rescored)
        retryable_v1 = dict(initial, retryable=True, collected_at=None)
        migrated = rescore_existing_row(retryable_v1, threshold_metrics("5000", "15000"))
        self.assertFalse(migrated["retryable"])
        self.assertIsNone(migrated["collected_at"])
        incomplete_v1 = dict(retryable_v1, raw_ui_values={**ui, "pnl_max_total": None})
        self.assertTrue(rescore_existing_row(incomplete_v1, threshold_metrics("5000", "15000"))["retryable"])

    def test_all_four_conditions_required_and_unknown_is_preserved(self):
        wallet = {"chain": "bsc", "wallet": "0x" + "a" * 40}
        base = {
            "wallet_match": True,
            "period_match": True,
            "pnl_30d_total": "$10001",
            "pnl_30d_realized": "$10001",
            "pnl_max_total": "$30001",
            "pnl_max_realized": "$30001",
        }
        passed = evaluate_snapshot(wallet, base, "2026-10-12T00:00:00Z")
        self.assertEqual(passed["status"], "PASS")
        base["pnl_max_realized"] = "$29999"
        self.assertEqual(evaluate_snapshot(wallet, base, "now")["status"], "FAIL")
        base["pnl_max_realized"] = "$30K"
        row = evaluate_snapshot(wallet, base, "now")
        self.assertEqual(row["status"], "UNKNOWN")
        self.assertEqual(row["pnl_max_realized"]["raw"], "$30K")

    def test_wallet_and_period_must_be_confirmed_before_accepting_values(self):
        snapshot = {"wallet_match": False, "period_match": True, **{key: "$999999" for key, _ in __import__("axiom_pnl").METRICS}}
        row = evaluate_snapshot({}, snapshot, "now")
        self.assertEqual(row["status"], "UNKNOWN")
        self.assertIn("wallet", row["reason"])
        self.assertEqual(row["pnl_30d_total"]["raw"], "$999999")
        snapshot["wallet_match"] = True
        snapshot["period_match"] = False
        row = evaluate_snapshot({}, snapshot, "now")
        self.assertEqual(row["status"], "UNKNOWN")
        self.assertIn("period", row["reason"])

    def test_ui_metric_without_explicit_usd_marker_is_unknown(self):
        snapshot = {
            "wallet_match": True, "period_match": True,
            "pnl_30d_total": "100000", "pnl_30d_realized": "$100000",
            "pnl_max_total": "$100000", "pnl_max_realized": "$100000",
        }
        row = evaluate_snapshot({"wallet": "w"}, snapshot, "now")
        self.assertEqual(row["pnl_30d_total_status"], "UNKNOWN")
        self.assertEqual(row["status"], "UNKNOWN")

    def test_outputs_include_failed_and_unknown_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            rows = [{"wallet": "a", "status": "PASS"}, {"wallet": "b", "status": "FAIL"}, {"wallet": "c", "status": "UNKNOWN"}]
            write_outputs(out, rows, {"as_of": "now"})
            all_rows = (out / "all.jsonl").read_text().splitlines()
            passed = json.loads((out / "passed.json").read_text())
        self.assertEqual(len(all_rows), 3)
        self.assertEqual([row["wallet"] for row in passed], ["a"])

    def test_passed_exports_sort_max_total_descending_and_all_exports_keep_input_order(self):
        rows = [
            {"wallet": "pass-low", "status": "PASS", "pnl_max_total": {"raw": "$40K", "lower": "39500", "upper_exclusive": "40500", "exact": False}},
            {"wallet": "fail-high", "status": "FAIL", "pnl_max_total": {"raw": "$900K", "lower": "899500", "upper_exclusive": "900500", "exact": False}},
            {"wallet": "pass-high", "status": "PASS", "pnl_max_total": {"raw": "$90K", "lower": "89500", "upper_exclusive": "90500", "exact": False}},
            {"wallet": "unknown", "status": "UNKNOWN"},
            {"wallet": "pass-missing", "status": "PASS"},
        ]
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            write_outputs(out, rows, {"as_of": "now"}, planned_count=len(rows))
            passed_json = json.loads((out / "passed.json").read_text())
            all_jsonl = [json.loads(line) for line in (out / "all.jsonl").read_text().splitlines()]
            with (out / "passed.csv").open(newline="", encoding="utf-8") as source:
                passed_csv = list(csv.DictReader(source))
            with (out / "all.csv").open(newline="", encoding="utf-8") as source:
                all_csv = list(csv.DictReader(source))
        self.assertEqual([row["wallet"] for row in passed_json], ["pass-high", "pass-low", "pass-missing"])
        self.assertEqual([row["wallet"] for row in passed_csv], ["pass-high", "pass-low", "pass-missing"])
        self.assertEqual([row["wallet"] for row in all_jsonl], [row["wallet"] for row in rows])
        self.assertEqual([row["wallet"] for row in all_csv], [row["wallet"] for row in rows])

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
            with self.assertRaises(ResumeError):
                load_manifest(out, None, "bsc", threshold_metrics("5000", "15000"))

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
                chain="bsc", as_of=None, resume=True, min_30d_usd="10000", min_max_usd="30000",
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
