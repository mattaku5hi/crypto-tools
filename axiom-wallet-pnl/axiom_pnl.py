#!/usr/bin/env python3
"""Read-only Axiom wallet PnL collector core and CLI."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import re
import tempfile
import time
import subprocess
import sys
from urllib.parse import urlparse
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Iterable

VERSION = "0.1.0"
SOURCE_POLICY = "axiom-portfolio-performance-dom-usd-v1"
WINDOW_30D_USD = Decimal("10000")
MAX_USD = Decimal("30000")
MAX_INPUT_BYTES = 16 * 1024 * 1024
MAX_WALLETS = 100_000
METRICS = (
    ("pnl_30d_total", WINDOW_30D_USD),
    ("pnl_30d_realized", WINDOW_30D_USD),
    ("pnl_max_total", MAX_USD),
    ("pnl_max_realized", MAX_USD),
)
BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
BASE58_INDEX = {char: index for index, char in enumerate(BASE58_ALPHABET)}
MONEY_RE = re.compile(r"^\s*\$?\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+))\s*([KMB])?\s*$", re.I)


class InputError(ValueError):
    pass


class ResumeError(ValueError):
    pass


@dataclass(frozen=True)
class Amount:
    raw: str
    lower: Decimal
    upper: Decimal | None
    exact: bool

    def as_json(self) -> dict[str, Any]:
        return {
            "raw": self.raw,
            "lower": decimal_string(self.lower),
            "upper_exclusive": decimal_string(self.upper) if self.upper is not None else None,
            "exact": self.exact,
        }


def decimal_string(value: Decimal) -> str:
    return format(value, "f")


def parse_amount(value: Any) -> Amount:
    """Parse exact values; suffixed numeric strings retain rounded interval semantics."""
    if value is None or isinstance(value, bool):
        raise InputError("missing or invalid money value")
    raw = str(value)
    if isinstance(value, (int, Decimal)):
        try:
            parsed = Decimal(str(value))
        except InvalidOperation as exc:
            raise InputError("invalid money value") from exc
        if not parsed.is_finite():
            raise InputError("money value must be finite")
        return Amount(raw, parsed, parsed, True)
    normalized = re.sub(r"^\s*([+-])\s*\$", r"\1", raw)
    match = MONEY_RE.fullmatch(normalized)
    if not match:
        raise InputError(f"unrecognized money value: {raw!r}")
    number = Decimal(match.group(1))
    suffix = (match.group(2) or "").upper()
    multiplier = {"": Decimal(1), "K": Decimal(1000), "M": Decimal(1000000), "B": Decimal(1000000000)}[suffix]
    lower = number * multiplier
    if not suffix:
        return Amount(raw, lower, lower, True)
    places = max(0, -number.as_tuple().exponent)
    quantum = Decimal(1).scaleb(-places) * multiplier
    # UI abbreviations are treated as rounded display values. The hidden value
    # can lie half a displayed unit on either side; upper is exclusive.
    half = quantum / Decimal(2)
    return Amount(raw, lower - half, lower + half, False)


def parse_ui_amount(value: Any) -> Amount:
    """Parse a displayed Axiom money string as rounded at its shown precision."""
    if not isinstance(value, str):
        raise InputError("UI money value must be a string")
    parsed = parse_amount(value)
    normalized = re.sub(r"^\s*([+-])\s*\$", r"\1", value)
    match = MONEY_RE.fullmatch(normalized)
    if not match:
        raise InputError("unrecognized UI money value")
    number = Decimal(match.group(1))
    suffix = (match.group(2) or "").upper()
    multiplier = {"": Decimal(1), "K": Decimal(1000), "M": Decimal(1000000), "B": Decimal(1000000000)}[suffix]
    center = number * multiplier
    places = max(0, -number.as_tuple().exponent)
    quantum = Decimal(1).scaleb(-places) * multiplier
    half = quantum / Decimal(2)
    return Amount(parsed.raw, center - half, center + half, False)


def has_css_token(class_name: str, token: str) -> bool:
    return token in class_name.split()


def wallet_tab_needs_click(class_name: str) -> bool:
    return not has_css_token(class_name, "text-primaryBlue")


def search_results_ready(results_visible: bool, matching_card_count: int) -> bool:
    return results_visible and matching_card_count == 1


def classify_amount(amount: Amount, threshold: Decimal) -> str:
    if amount.exact:
        return "PASS" if amount.lower >= threshold else "FAIL"
    if amount.lower >= threshold:
        return "PASS"
    if amount.upper is not None and amount.upper <= threshold:
        return "FAIL"
    return "UNKNOWN"


def decode_base58(text: str) -> bytes:
    number = 0
    try:
        for char in text:
            number = number * 58 + BASE58_INDEX[char]
    except KeyError as exc:
        raise InputError("invalid base58 character") from exc
    body = number.to_bytes((number.bit_length() + 7) // 8, "big") if number else b""
    return b"\0" * (len(text) - len(text.lstrip("1"))) + body


def normalize_wallet(wallet: str, chain: str) -> str:
    chain = chain.strip().lower()
    if chain == "solana":
        if len(decode_base58(wallet)) != 32:
            raise InputError(f"invalid Solana address length: {wallet!r}")
        return wallet
    if chain == "bsc":
        address = wallet[2:] if wallet.startswith(("0x", "0X")) else wallet
        if not re.fullmatch(r"[0-9a-fA-F]{40}", address):
            raise InputError(f"invalid EVM address: {wallet!r}")
        return "0x" + address.lower()
    raise InputError(f"unsupported or missing chain: {chain!r}")


def read_wallets(path: Path, default_chain: str | None) -> list[dict[str, Any]]:
    try:
        if path.stat().st_size > MAX_INPUT_BYTES:
            raise InputError(f"input exceeds {MAX_INPUT_BYTES} byte limit")
        if path.suffix.lower() == ".csv":
            with path.open(newline="", encoding="utf-8-sig") as source:
                rows: Any = list(csv.DictReader(source))
        elif path.suffix.lower() in {".json", ".jsonl"}:
            with path.open(encoding="utf-8") as source:
                if path.suffix.lower() == ".jsonl":
                    rows = [json.loads(line, parse_float=Decimal) for line in source if line.strip()]
                else:
                    rows = json.load(source, parse_float=Decimal)
        else:
            raise InputError("input file must be CSV, JSON, or JSONL")
    except (OSError, json.JSONDecodeError) as exc:
        raise InputError(f"cannot read input file: {exc}") from exc
    if isinstance(rows, dict):
        rows = rows.get("wallets", rows.get("items", [rows]))
    if not isinstance(rows, list):
        raise InputError("JSON input must be an object or array")
    if len(rows) > MAX_WALLETS:
        raise InputError(f"input exceeds {MAX_WALLETS} wallet limit")
    normalized: list[dict[str, Any]] = []
    seen: set[tuple[str, str]] = set()
    for index, row in enumerate(rows, start=1):
        if isinstance(row, str):
            row = {"wallet": row}
        if not isinstance(row, dict):
            raise InputError(f"row {index} must be an object or address string")
        original = row.get("wallet") or row.get("address") or row.get("trackedWalletAddress")
        chain = str(row.get("chain") or default_chain or "").strip().lower()
        if not original:
            raise InputError(f"row {index} has no wallet/address/trackedWalletAddress")
        if not chain:
            raise InputError(f"row {index} needs a chain column or --chain")
        address = normalize_wallet(str(original).strip(), chain)
        key = (chain, address)
        if key in seen:
            continue
        seen.add(key)
        normalized.append({**row, "input_wallet": str(original).strip(), "wallet": address, "chain": chain, "input_index": index})
    return normalized


def threshold_metrics(
    min_30d_usd: Decimal | str | None = None, min_max_usd: Decimal | str | None = None
) -> tuple[tuple[str, Decimal], ...]:
    values = (
        ("pnl_30d_total", min_30d_usd if min_30d_usd is not None else WINDOW_30D_USD),
        ("pnl_30d_realized", min_30d_usd if min_30d_usd is not None else WINDOW_30D_USD),
        ("pnl_max_total", min_max_usd if min_max_usd is not None else MAX_USD),
        ("pnl_max_realized", min_max_usd if min_max_usd is not None else MAX_USD),
    )
    result = []
    for key, value in values:
        try:
            amount = value if isinstance(value, Decimal) else Decimal(str(value))
        except InvalidOperation as exc:
            raise InputError("PnL thresholds must be finite positive decimals") from exc
        if not amount.is_finite() or amount <= 0:
            raise InputError("PnL thresholds must be finite positive decimals")
        result.append((key, amount))
    return tuple(result)


def config_fingerprint(
    as_of: str, chain: str | None, thresholds: tuple[tuple[str, Decimal], ...] | None = None
) -> str:
    thresholds = thresholds or METRICS
    config = {
        "as_of": as_of,
        "chain_default": chain,
        "thresholds": {key: decimal_string(value) for key, value in thresholds},
        "source_policy": SOURCE_POLICY,
        "software_version": VERSION,
    }
    encoded = json.dumps(config, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def bind_input_identity(manifest: dict[str, Any], wallets: list[dict[str, Any]]) -> None:
    identities = [(wallet["chain"], wallet["wallet"]) for wallet in wallets]
    encoded = json.dumps(identities, separators=(",", ":")).encode()
    fingerprint = hashlib.sha256(encoded).hexdigest()
    existing = manifest.get("input_fingerprint")
    if existing is not None and existing != fingerprint:
        raise ResumeError("wallet identities or input order differ from checkpoint; use a new output directory")
    manifest["input_fingerprint"] = fingerprint


def now_utc() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def load_manifest(
    path: Path, requested_as_of: str | None, chain: str | None,
    thresholds: tuple[tuple[str, Decimal], ...] | None = None,
) -> dict[str, Any]:
    thresholds = thresholds or METRICS
    manifest_path = path / "manifest.json"
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        as_of = manifest.get("as_of")
        if requested_as_of and requested_as_of != as_of:
            raise ResumeError("--as-of differs from checkpoint; use a new output directory")
        expected = config_fingerprint(as_of, chain, thresholds)
        if manifest.get("config_fingerprint") != expected:
            raise ResumeError("threshold, source policy, chain default, or as-of changed; use a new output directory")
        return manifest
    as_of = requested_as_of or now_utc()
    return {
        "software_version": VERSION,
        "source_policy": SOURCE_POLICY,
        "decoder_version": SOURCE_POLICY,
        "price_policy_version": "axiom-published-usd-metrics-v1",
        "finality": "not_applicable_axiom_analytics",
        "as_of": as_of,
        "scope": {"pnl_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180", "pnl_max": "Axiom Max (allTime) range"},
        "thresholds_usd": {key: decimal_string(value) for key, value in thresholds},
        "chain_default": chain,
        "config_fingerprint": config_fingerprint(as_of, chain, thresholds),
        "status": "in_progress",
        "as_of_semantics": "run start; wallet snapshots are collected live and sequentially",
        "planned_wallet_count": 0,
        "wallet_count": 0,
        "completed_count": 0,
        "passed_count": 0,
        "unknown_count": 0,
    }


def evaluate_snapshot(
    wallet: dict[str, Any], snapshot: dict[str, Any], collected_at: str,
    thresholds: tuple[tuple[str, Decimal], ...] | None = None,
) -> dict[str, Any]:
    """Convert a normalized snapshot into reviewable metrics and a four-gate result."""
    row: dict[str, Any] = {
        **wallet,
        "collected_at": collected_at,
        "scope_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180",
        "scope_max": "Axiom Max (allTime) range",
        "source": snapshot.get("source", SOURCE_POLICY),
        "wallet_match": snapshot.get("wallet_match"),
        "period_match": snapshot.get("period_match"),
        "source_consistent": snapshot.get("source_consistent", True),
        "status": "UNKNOWN",
        "reason": None,
        "retryable": bool(snapshot.get("retryable", False)),
        "provenance": snapshot.get("provenance"),
        "raw_ui_values": snapshot.get("raw_ui_values"),
    }
    results: dict[str, str] = {}
    for key, threshold in (thresholds or METRICS):
        value = snapshot.get(key)
        try:
            amount = parse_ui_amount(value) if key in {"pnl_30d_total", "pnl_max_total"} else parse_amount(value)
        except InputError:
            row[key] = {"raw": None if value is None else str(value), "lower": None, "upper_exclusive": None, "exact": None}
            row[f"{key}_status"] = "UNKNOWN"
            results[key] = "UNKNOWN"
            continue
        row[key] = amount.as_json()
        status = classify_amount(amount, threshold)
        row[f"{key}_status"] = status
        results[key] = status
    if snapshot.get("wallet_match") is not True:
        row["reason"] = "requested wallet identity was not confirmed"
    elif snapshot.get("period_match") is not True:
        row["reason"] = "requested Axiom period was not confirmed"
    elif snapshot.get("source_consistent", True) is not True:
        row["reason"] = "Axiom API and Performance UI values did not agree"
    elif all(status == "PASS" for status in results.values()):
        row["status"] = "PASS"
    elif "FAIL" in results.values():
        row["status"] = "FAIL"
        row["reason"] = "one or more of the four PnL thresholds failed"
    else:
        row["reason"] = "one or more PnL values are missing or uncertain at a threshold"
    return row


def rescore_existing_row(
    row: dict[str, Any], thresholds: tuple[tuple[str, Decimal], ...]
) -> dict[str, Any]:
    """Reapply thresholds to a saved row without changing its observed evidence."""
    ui = row.get("raw_ui_values") or {}
    api_snapshot: dict[str, Any] = {
        "wallet_match": row.get("wallet_match"),
        "period_match": row.get("period_match"),
        "source": row.get("source", SOURCE_POLICY),
        "provenance": row.get("provenance"),
        "retryable": row.get("retryable", False),
    }
    for key in ("pnl_30d_realized", "pnl_max_realized"):
        saved = row.get(key)
        api_snapshot[key] = saved.get("raw") if isinstance(saved, dict) else saved
    snapshot = merge_metrics_snapshot(api_snapshot, ui)
    return evaluate_snapshot(row, snapshot, row.get("collected_at") or "", thresholds)


def merge_metrics_snapshot(api_snapshot: dict[str, Any], ui_values: dict[str, str]) -> dict[str, Any]:
    """Combine exact realized API amounts and rounded USD Total UI amounts."""
    snapshot = dict(api_snapshot)
    snapshot["pnl_30d_total"] = ui_values.get("pnl_30d_total")
    snapshot["pnl_max_total"] = ui_values.get("pnl_max_total")
    snapshot["raw_ui_values"] = dict(ui_values)
    consistent = True
    for api_key, ui_key in (
        ("pnl_30d_realized", "pnl_30d_realized"),
        ("pnl_max_realized", "pnl_max_realized"),
    ):
        try:
            api_amount = parse_amount(snapshot.get(api_key))
            ui_amount = parse_ui_amount(ui_values.get(ui_key))
        except InputError:
            consistent = False
            continue
        if ui_amount.exact:
            same = api_amount.lower == ui_amount.lower
        else:
            same = ui_amount.lower <= api_amount.lower < (ui_amount.upper or ui_amount.lower)
        consistent = consistent and same
    snapshot["source_consistent"] = consistent
    return snapshot


def extract_performance_ui(text: str, period: str) -> dict[str, str]:
    """Read exact Total/Realized labels from the observed Performance section."""
    labels = {
        "30d": {"pnl_30d_total": "30d Total PNL", "pnl_30d_realized": "30d Realized PNL"},
        "Max": {"pnl_max_total": "Total Pnl", "pnl_max_realized": "Realized PNL"},
    }
    if period not in labels:
        raise InputError(f"unsupported Axiom performance period: {period}")
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    result: dict[str, str] = {}
    for metric, label in labels[period].items():
        positions = [index for index, line in enumerate(lines) if line == label]
        if len(positions) != 1 or positions[0] + 1 >= len(lines):
            raise InputError(f"Performance section is missing unique {label!r} label/value")
        raw_value = lines[positions[0] + 1]
        parse_amount(raw_value)
        result[metric] = raw_value
    return result


def map_wallet_metrics_payload(payload: dict[str, Any]) -> dict[str, Any]:
    """Map only observed USD realized fields; native-token totalPnl is never USD."""
    portfolio = payload.get("portfolio", payload)
    if not isinstance(portfolio, dict):
        raise InputError("portfolio response has no portfolio object")
    performance = portfolio.get("performanceMetrics")
    if not isinstance(performance, dict):
        raise InputError("portfolio response has no performanceMetrics object")
    thirty_day = performance.get("thirtyDay")
    all_time = performance.get("allTime")
    if not isinstance(thirty_day, dict) or not isinstance(all_time, dict):
        raise InputError("portfolio response is missing thirtyDay or allTime")
    native_token = thirty_day.get("nativeToken")
    all_time_native = all_time.get("nativeToken")
    if not native_token or not all_time_native:
        raise InputError("Axiom response is missing native-token identity for a required window")
    if str(native_token).lower() != str(all_time_native).lower():
        raise InputError("Axiom metrics windows use different native-token units")
    return {
        "pnl_30d_total": None,
        "pnl_30d_realized": thirty_day.get("realizedUsdPnl"),
        "pnl_max_total": None,
        "pnl_max_realized": all_time.get("realizedUsdPnl"),
        "source": "Axiom Performance UI + chain-specific metrics API",
        "wallet_match": None,
        "period_match": None,
        "provenance": {
            "native_token": native_token,
            "thirty_day_total_native": thirty_day.get("totalPnl"),
            "all_time_total_native": all_time.get("totalPnl"),
            "total_native_unit": native_token,
            "max_window_source_key": "allTime",
            "thirty_day_transfer_funded_usd": thirty_day.get("transferFundedUsd"),
            "thirty_day_transfer_funded_closes": thirty_day.get("transferFundedCloses"),
            "all_time_transfer_funded_usd": all_time.get("transferFundedUsd"),
            "all_time_transfer_funded_closes": all_time.get("transferFundedCloses"),
        },
    }


class CollectionError(RuntimeError):
    pass


class StopCollection(CollectionError):
    pass


class BrowserAdapter:
    """Read Axiom Performance from an authenticated Yandex Browser tab."""

    API_HOSTS = {
        "solana": re.compile(r"^api[0-9]*\.axiom\.trade$", re.I),
        "bsc": re.compile(r"^bnb-api[0-9]+\.axiom\.trade$", re.I),
    }
    NATIVE_TOKENS = {"solana": {"sol"}, "bsc": {"bnb"}}
    BADGE_SELECTORS = {"solana": "img[src*='/images/sol-fill.svg']", "bsc": "img[alt='BNB']"}
    HOME_URL = "https://axiom.trade"
    RESPONSE_LIMIT = 2 * 1024 * 1024

    def __init__(self, page: Any):
        self.page = page
        self.page.set_default_timeout(10_000)
        self._has_navigated = False
        self._expected_wallet: dict[str, Any] | None = None
        self._matching_responses: list[Any] = []
        self._stop_reason: str | None = None
        self.page.on("response", self._observe_response)

    def _prepare_page_for_search(self) -> None:
        if not self._has_navigated:
            self.page.goto(self.HOME_URL, wait_until="domcontentloaded", timeout=20_000)
            self._has_navigated = True
            return
        self.page.keyboard.press("Escape")
        self.page.keyboard.press("Escape")

    @staticmethod
    def _matches_wallet_request(request: Any, wallet: dict[str, Any]) -> bool:
        try:
            parsed_url = urlparse(request.url)
            host_pattern = BrowserAdapter.API_HOSTS[wallet["chain"]]
            if (
                request.method != "POST"
                or parsed_url.path != "/portfolio"
                or not host_pattern.fullmatch(parsed_url.hostname or "")
            ):
                return False
            post_data = request.post_data
            if not post_data or len(post_data) > 64 * 1024:
                return False
            body = json.loads(post_data, parse_float=Decimal)
            addresses = body.get("walletAddresses") if isinstance(body, dict) else None
            if not (
                isinstance(addresses, list)
                and len(addresses) == 1
                and normalize_wallet(str(addresses[0]), wallet["chain"]) == wallet["wallet"]
            ):
                return False
            return (
                body.get("isOtherWallet") is True
                and body.get("denomination") == "usd"
                and body.get("timeOffsetMinutes") == -180
                and body.get("activesSort") == "recent"
                and wallet["chain"] in {"solana", "bsc"}
            )
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            return False

    def _map_response(self, response: Any, wallet: dict[str, Any]) -> dict[str, Any]:
        if response.status in {401, 403}:
            raise StopCollection("Axiom authentication or anti-bot challenge detected; sign in manually, then resume.")
        if response.status == 429:
            raise StopCollection("Axiom rate limit detected; pause, then resume later.")
        if response.status != 200:
            raise CollectionError("Axiom metrics request did not succeed")
        content_length = response.headers.get("content-length")
        if content_length and content_length.isdigit() and int(content_length) > self.RESPONSE_LIMIT:
            raise CollectionError("Axiom metrics response exceeded the size limit")
        body_text = response.text()
        if len(body_text.encode("utf-8")) > self.RESPONSE_LIMIT:
            raise CollectionError("Axiom metrics response exceeded the size limit")
        try:
            payload = json.loads(body_text, parse_float=Decimal)
            mapped = map_wallet_metrics_payload(payload)
        except (InputError, json.JSONDecodeError) as exc:
            raise CollectionError("Axiom metrics response did not match the observed schema") from exc
        mapped["source"] = f"{urlparse(response.url).hostname}/portfolio + Axiom Performance UI"
        native = str((mapped.get("provenance") or {}).get("native_token") or "").lower()
        if native not in self.NATIVE_TOKENS[wallet["chain"]]:
            raise CollectionError("Axiom response native-token identity did not match the selected network")
        return mapped

    def _locate_search_input(self, wallet: dict[str, Any]) -> Any:
        self._raise_if_stopped()
        self._prepare_page_for_search()
        search_button = self.page.get_by_text("Search by token or CA", exact=True)
        search_button.first.wait_for(state="visible", timeout=15_000)
        if search_button.count() != 1:
            raise CollectionError("Axiom search control was not uniquely identified")
        self._raise_if_stopped()
        search_button.click()
        wallets_tab = self.page.locator("button").filter(
            has=self.page.get_by_text("Wallets", exact=True),
        )
        wallets_tab.first.wait_for(state="visible", timeout=10_000)
        if wallets_tab.count() != 1:
            raise CollectionError("Axiom Wallets search tab was not uniquely identified")
        self._raise_if_stopped()
        if wallet_tab_needs_click(wallets_tab.get_attribute("class") or ""):
            wallets_tab.click()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            self._raise_if_stopped()
            if not wallet_tab_needs_click(wallets_tab.get_attribute("class") or ""):
                break
            self.page.wait_for_timeout(100)
        else:
            raise CollectionError("Axiom Wallets search tab did not become selected")
        search_input = self.page.get_by_placeholder("Search by name, ticker, CA, or wallet", exact=True)
        search_input.wait_for(state="visible", timeout=10_000)
        search_input.fill(wallet["wallet"])
        self._raise_if_stopped()
        return search_input

    def _raise_if_stopped(self) -> None:
        if self._stop_reason:
            raise StopCollection(self._stop_reason)

    def _observe_response(self, response: Any) -> None:
        try:
            parsed = urlparse(response.url)
            host = parsed.hostname or ""
            if (host == "axiom.trade" or host.endswith(".axiom.trade")) and response.status in {401, 403}:
                self._stop_reason = "Axiom authentication or anti-bot challenge detected; sign in manually, then resume."
            elif (host == "axiom.trade" or host.endswith(".axiom.trade")) and response.status == 429:
                self._stop_reason = "Axiom rate limit detected; pause, then resume later."
            wallet = self._expected_wallet
            if wallet is not None and len(self._matching_responses) < 4 and self._matches_wallet_request(response.request, wallet):
                self._matching_responses.append(response)
        except Exception:
            return

    def _open_wallet_card(self, wallet: dict[str, Any]) -> dict[str, Any]:
        cards = self.page.locator("div.group.relative.isolate.cursor-pointer").filter(
            has=self.page.locator(self.BADGE_SELECTORS[wallet["chain"]]),
            has_text=re.compile(r"\S"),
        )
        results_heading = self.page.get_by_text("Results", exact=True)
        deadline = time.monotonic() + 30
        card_count = 0
        while time.monotonic() < deadline:
            self._raise_if_stopped()
            card_count = cards.count()
            results_visible = any(results_heading.nth(index).is_visible() for index in range(results_heading.count()))
            if search_results_ready(results_visible, card_count) and cards.first.is_visible():
                break
            self.page.wait_for_timeout(100)
        else:
            self._raise_if_stopped()
            if card_count > 1:
                raise CollectionError("Axiom wallet search returned an ambiguous chain result")
            raise CollectionError("Axiom did not show a matching chain wallet card")
        box = cards.first.bounding_box()
        if not box or box["width"] <= 450 or box["height"] <= 40:
            raise CollectionError("Axiom wallet card did not match the observed clickable layout")
        self._expected_wallet = wallet
        self._matching_responses.clear()
        cards.first.click(position={"x": 450, "y": 40})
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            self._raise_if_stopped()
            while self._matching_responses:
                response = self._matching_responses.pop(0)
                try:
                    return self._map_response(response, wallet)
                except CollectionError:
                    continue
            self.page.wait_for_timeout(100)
        self._raise_if_stopped()
        raise CollectionError("Axiom returned no fresh metrics response bound to the requested wallet")

    def _verify_header(self, wallet: dict[str, Any]) -> None:
        short = f"{wallet['wallet'][:4]}...{wallet['wallet'][-4:]}"
        header = self.page.get_by_text(short, exact=True)
        try:
            header.wait_for(state="visible", timeout=15_000)
        except Exception as exc:
            raise CollectionError("Axiom wallet modal did not confirm the requested wallet") from exc
        if header.count() != 1:
            raise CollectionError("Axiom wallet header was ambiguous")

    def _read_period(self, period: str) -> dict[str, str]:
        button = self.page.get_by_role("button", name=period, exact=True)
        if button.count() != 1:
            raise CollectionError(f"Axiom {period} period control was not uniquely identified")
        self._raise_if_stopped()
        button.click()
        deadline = time.monotonic() + 8
        section = self.page.get_by_text("Performance", exact=True).locator("xpath=../../..")
        last_error: Exception | None = None
        while time.monotonic() < deadline:
            self._raise_if_stopped()
            try:
                class_name = button.get_attribute("class") or ""
                if not has_css_token(class_name, "text-primaryBlue"):
                    self.page.wait_for_timeout(100)
                    continue
                text = section.inner_text()
                return extract_performance_ui(text, period)
            except Exception as exc:
                last_error = exc
                self.page.wait_for_timeout(100)
        raise CollectionError(f"Axiom {period} Performance values were not confirmed") from last_error

    def _challenge_reason(self) -> str | None:
        try:
            body_text = self.page.locator("body").evaluate(
                "element => (element.innerText || '').slice(0, 12000)"
            ).lower()
        except Exception:
            return None
        if any(marker in body_text for marker in ("captcha", "verify you are human", "verify you're human")):
            return "Axiom CAPTCHA challenge detected; complete it manually, then resume."
        if any(marker in body_text for marker in ("too many requests", "rate limit", "rate limited")):
            return "Axiom rate limit detected; pause, then resume later."
        if any(marker in body_text for marker in ("session expired", "sign in to continue", "log in to continue", "please sign in")):
            return "Axiom sign-in is required; sign in manually, then resume."
        return None

    def collect(self, wallet: dict[str, Any]) -> dict[str, Any]:
        try:
            self._locate_search_input(wallet)
            api_snapshot = self._open_wallet_card(wallet)
            self._verify_header(wallet)
            ui_values = {}
            ui_values.update(self._read_period("30d"))
            ui_values.update(self._read_period("Max"))
            snapshot = merge_metrics_snapshot(api_snapshot, ui_values)
            snapshot["wallet_match"] = True
            snapshot["period_match"] = True
            snapshot["retryable"] = not snapshot.get("source_consistent", False)
            return snapshot
        except StopCollection:
            raise
        except Exception as exc:
            reason = self._challenge_reason()
            if reason:
                raise StopCollection(reason) from exc
            if isinstance(exc, CollectionError):
                raise
            raise CollectionError("Axiom page did not match the verified read workflow") from exc


def ensure_private_directory(path: Path) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(path, 0o700)


def _atomic_write(path: Path, writer: Any, newline: str | None = None) -> None:
    ensure_private_directory(path.parent)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    temporary_path = Path(temporary_name)
    try:
        os.chmod(temporary_path, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline=newline) as target:
            writer(target)
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary_path, path)
        os.chmod(path, 0o600)
        directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        temporary_path.unlink(missing_ok=True)


def write_outputs(
    out_dir: Path, rows: list[dict[str, Any]], manifest: dict[str, Any], planned_count: int | None = None
) -> None:
    ensure_private_directory(out_dir)
    passed = [row for row in rows if row.get("status") == "PASS"]
    if planned_count is not None:
        manifest["planned_wallet_count"] = planned_count
    planned = manifest.get("planned_wallet_count", len(rows))
    manifest["wallet_count"] = len(rows)
    manifest["completed_count"] = sum(not row.get("retryable", False) for row in rows)
    manifest["passed_count"] = len(passed)
    manifest["unknown_count"] = sum(row.get("status") == "UNKNOWN" for row in rows)
    manifest["status"] = "complete" if len(rows) == planned and all(not row.get("retryable", False) for row in rows) else "in_progress"

    def write_jsonl(target: Any) -> None:
        for row in rows:
            target.write(json.dumps(row, ensure_ascii=False, default=str) + "\n")

    def write_passed_json(target: Any) -> None:
        json.dump(passed, target, indent=2, ensure_ascii=False, default=str)
        target.write("\n")

    _atomic_write(out_dir / "all.jsonl", write_jsonl)
    _atomic_write(out_dir / "passed.json", write_passed_json)
    _write_csv_atomic(out_dir / "all.csv", rows)
    _write_csv_atomic(out_dir / "passed.csv", passed)
    _atomic_write(
        out_dir / "manifest.json",
        lambda target: target.write(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"),
    )


def _write_csv_atomic(path: Path, rows: list[dict[str, Any]]) -> None:
    metric_keys = [key for key, _ in METRICS]
    flattened = {
        key: [f"{key}_raw", f"{key}_lower_usd", f"{key}_upper_exclusive_usd", f"{key}_exact"]
        for key in metric_keys
    }
    columns: list[str] = []
    for row in rows:
        for key in row:
            if key not in metric_keys and key not in columns:
                columns.append(key)
        for key in metric_keys:
            for column in flattened[key]:
                if column not in columns:
                    columns.append(column)

    def write_csv(target: Any) -> None:
        if not columns:
            return
        writer = csv.DictWriter(target, fieldnames=columns, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            projected = {key: value for key, value in row.items() if key not in metric_keys}
            for key in metric_keys:
                amount = row.get(key)
                fields = flattened[key]
                if isinstance(amount, dict):
                    for field, source_key in zip(fields, ("raw", "lower", "upper_exclusive", "exact")):
                        projected[field] = amount.get(source_key)
                elif amount is not None:
                    projected[fields[0]] = amount
            writer.writerow({key: json.dumps(value, ensure_ascii=False, default=str) if isinstance(value, (dict, list)) else value for key, value in projected.items()})

    _atomic_write(path, write_csv, newline="")


def command_login(args: argparse.Namespace) -> int:
    profile = Path(args.profile).expanduser().resolve()
    ensure_private_directory(profile)
    command = [
        args.browser,
        f"--user-data-dir={profile}",
        f"--remote-debugging-address=127.0.0.1",
        f"--remote-debugging-port={args.cdp_port}",
        "--no-first-run",
        args.url,
    ]
    try:
        subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError as exc:
        print(f"cannot start Yandex Browser: {exc}", file=sys.stderr)
        return 2
    print(f"Opened Yandex Browser with local CDP on 127.0.0.1:{args.cdp_port}.")
    print("Sign in manually in that browser. Leave it open while collect runs.")
    input("Press Enter here when sign-in is complete: ")
    return 0


def validate_cdp_url(value: str) -> str:
    parsed = urlparse(value)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost"} or parsed.username or parsed.password:
        raise InputError("--cdp-url must be an http URL on 127.0.0.1 or localhost")
    if parsed.path not in {"", "/"} or parsed.query or parsed.fragment:
        raise InputError("--cdp-url must be an origin URL without path, query, or fragment")
    return value.rstrip("/")


def connect_cdp(playwright: Any, cdp_url: str) -> tuple[Any, Any, bool]:
    """Attach to the existing authenticated context and create an owned tab."""
    browser = playwright.chromium.connect_over_cdp(validate_cdp_url(cdp_url), timeout=10_000)
    if not browser.contexts:
        raise RuntimeError("attached Yandex Browser has no authenticated context")
    page = browser.contexts[0].new_page()
    return browser, page, True


def command_collect(args: argparse.Namespace) -> int:
    try:
        cdp_url = validate_cdp_url(args.cdp_url)
        wallets = read_wallets(Path(args.input), args.chain)
        thresholds = threshold_metrics(args.min_30d_usd, args.min_max_usd)
        out_dir = Path(args.out).expanduser().resolve()
        manifest = load_manifest(out_dir, args.as_of, args.chain, thresholds)
        bind_input_identity(manifest, wallets)
    except (InputError, ResumeError, OSError, json.JSONDecodeError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    if (out_dir / "manifest.json").exists() and not args.resume:
        print("output directory already has a manifest; pass --resume or choose a new directory", file=sys.stderr)
        return 2
    rows_by_key: dict[tuple[str, str], dict[str, Any]] = {}
    checkpoint = out_dir / "all.jsonl"
    if args.resume and checkpoint.exists() and not (out_dir / "manifest.json").exists():
        print("checkpoint has no manifest; cannot verify resume policy", file=sys.stderr)
        return 2
    if args.resume and checkpoint.exists():
        for line in checkpoint.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                old = json.loads(line)
            except json.JSONDecodeError as exc:
                print(f"invalid checkpoint row: {exc}", file=sys.stderr)
                return 2
            rows_by_key[(old.get("chain", ""), old.get("wallet", ""))] = old

    pending = [wallet for wallet in wallets if not (prior := rows_by_key.get((wallet["chain"], wallet["wallet"]))) or prior.get("retryable", False)]
    if not pending:
        ordered = [rows_by_key[(item["chain"], item["wallet"])] for item in wallets if (item["chain"], item["wallet"]) in rows_by_key]
        write_outputs(out_dir, ordered, manifest, planned_count=len(wallets))
        print(f"Saved {len(ordered)} wallets to {out_dir}")
        return 0
    ordered = [rows_by_key[(item["chain"], item["wallet"])] for item in wallets if (item["chain"], item["wallet"]) in rows_by_key]
    write_outputs(out_dir, ordered, manifest, planned_count=len(wallets))
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        print("Playwright is not installed. Install requirements.txt first.", file=sys.stderr)
        return 2
    playwright = sync_playwright().start()
    created_page = False
    page = None
    stopped = False
    try:
        try:
            _, page, created_page = connect_cdp(playwright, cdp_url)
            adapter = BrowserAdapter(page)
        except Exception:
            print("could not attach to the local Yandex Browser session", file=sys.stderr)
            return 2
        for index, wallet in enumerate(pending):
            try:
                snapshot = adapter.collect(wallet)
                row = evaluate_snapshot(wallet, snapshot, now_utc(), thresholds)
            except StopCollection as exc:
                row = {
                    **wallet,
                    "collected_at": now_utc(),
                    "scope_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180",
                    "scope_max": "Axiom Max (allTime) range",
                    "source": SOURCE_POLICY,
                    "status": "UNKNOWN",
                    "reason": str(exc),
                    "retryable": True,
                }
                rows_by_key[(wallet["chain"], wallet["wallet"])] = row
                remaining = pending[index + 1 :]
                for unattempted in remaining:
                    key = (unattempted["chain"], unattempted["wallet"])
                    rows_by_key[key] = {
                        **unattempted,
                        "collected_at": None,
                        "scope_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180",
                        "scope_max": "Axiom Max (allTime) range",
                        "source": SOURCE_POLICY,
                        "status": "UNKNOWN",
                        "reason": "not attempted because collection paused for Axiom sign-in or rate-limit handling",
                        "retryable": True,
                    }
                ordered = [rows_by_key[(item["chain"], item["wallet"])] for item in wallets if (item["chain"], item["wallet"]) in rows_by_key]
                write_outputs(out_dir, ordered, manifest, planned_count=len(wallets))
                print("Axiom requested a pause; checkpoint saved for resume.")
                stopped = True
                break
            except CollectionError as exc:
                row = {
                    **wallet,
                    "collected_at": now_utc(),
                    "scope_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180",
                    "scope_max": "Axiom Max (allTime) range",
                    "source": SOURCE_POLICY,
                    "status": "UNKNOWN",
                    "reason": str(exc),
                    "retryable": True,
                }
            except Exception:
                row = {
                    **wallet,
                    "collected_at": now_utc(),
                    "scope_30d": "Axiom 30d Performance window; API timeOffsetMinutes=-180",
                    "scope_max": "Axiom Max (allTime) range",
                    "source": SOURCE_POLICY,
                    "status": "UNKNOWN",
                    "reason": "browser read failed; use --resume to retry",
                    "retryable": True,
                }
            rows_by_key[(wallet["chain"], wallet["wallet"])] = row
            ordered = [rows_by_key[(item["chain"], item["wallet"])] for item in wallets if (item["chain"], item["wallet"]) in rows_by_key]
            write_outputs(out_dir, ordered, manifest, planned_count=len(wallets))
            print(f"{wallet['chain']} {wallet['wallet']}: {row['status']}")
            if index + 1 < len(pending):
                page.wait_for_timeout(2000)
    finally:
        if created_page and page is not None:
            try:
                page.close()
            except Exception:
                pass
        playwright.stop()

    ordered = [rows_by_key[(item["chain"], item["wallet"])] for item in wallets if (item["chain"], item["wallet"]) in rows_by_key]
    write_outputs(out_dir, ordered, manifest, planned_count=len(wallets))
    if not stopped:
        print(f"Saved {len(ordered)} wallets to {out_dir}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    login = commands.add_parser("login", help="open Yandex Browser for manual sign-in")
    login.add_argument("--browser", default="/usr/bin/yandex-browser")
    login.add_argument("--profile", default="~/.local/share/crypto-tools/axiom-wallet-pnl/profile")
    login.add_argument("--url", default="https://axiom.trade")
    login.add_argument("--cdp-port", type=int, default=9225)
    login.set_defaults(func=command_login)
    collect = commands.add_parser("collect", help="validate wallets and write checkpointed result files")
    collect.add_argument("--input", required=True)
    collect.add_argument("--cdp-url", default="http://127.0.0.1:9225")
    collect.add_argument("--out", required=True)
    collect.add_argument("--chain", choices=("solana", "bsc"))
    collect.add_argument("--as-of", help="fixed UTC timestamp; resumes must use the checkpoint's value")
    collect.add_argument("--min-30d-usd", default="10000", help="minimum for both 30d PnL metrics")
    collect.add_argument("--min-max-usd", default="30000", help="minimum for both Max PnL metrics")
    collect.add_argument("--resume", action="store_true")
    collect.set_defaults(func=command_collect)
    return parser


def main(argv: Iterable[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
