#!/usr/bin/env python3
"""Quick diagnostics for recent bot orders and Hermes trade reports.

Usage:
  python3 scripts/recent_trade_diagnostics.py
  python3 scripts/recent_trade_diagnostics.py --market eth-updown-5m-1777959900
  python3 scripts/recent_trade_diagnostics.py --limit 20
"""

import argparse
import glob
import json
import os
from pathlib import Path
from typing import Any, Dict

DEFAULT_STATE_PATHS = [
    "/var/lib/trading-bot/state.json",
    "/root/lightweight_trading_bot_3/data/state.json",
]
DEFAULT_REPORT_DIR = "/var/lib/trading-bot/hermes-reports"


def load_json(path: str) -> Dict[str, Any]:
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def pick_state_path() -> str | None:
    for path in DEFAULT_STATE_PATHS:
        if os.path.exists(path):
            return path
    return None


def matches_market(value: str | None, market_filter: str | None) -> bool:
    if not market_filter:
        return True
    return bool(value and market_filter.lower() in value.lower())


def main() -> int:
    parser = argparse.ArgumentParser(description="Show recent Polymarket bot orders and Hermes trade reports.")
    parser.add_argument("--market", help="Filter by market slug substring, e.g. eth-updown-5m-1777959900")
    parser.add_argument("--limit", type=int, default=10, help="Number of recent reports/orders to show")
    parser.add_argument("--state", default=None, help="Override state.json path")
    parser.add_argument("--report-dir", default=DEFAULT_REPORT_DIR, help="Hermes report directory")
    args = parser.parse_args()

    state_path = args.state or pick_state_path()
    print(f"state_path {state_path or 'NOT_FOUND'}")

    if state_path and os.path.exists(state_path):
        try:
            state = load_json(state_path)
            orders = list((state.get("bot_orders") or {}).items())
            if args.market:
                orders = [(oid, o) for oid, o in orders if matches_market(o.get("market_slug"), args.market)]
            print("orders")
            for oid, order in orders[-args.limit:]:
                print(
                    oid,
                    order.get("market_slug"),
                    order.get("outcome"),
                    order.get("side"),
                    order.get("created_at"),
                    order.get("reason") or order.get("phase"),
                    order.get("limit_price"),
                    order.get("amount_usd"),
                )
            print("positions")
            positions = state.get("bot_positions") or {}
            for key, pos in positions.items():
                market_slug = pos.get("market_slug") if isinstance(pos, dict) else str(key)
                if matches_market(market_slug or str(key), args.market):
                    print(key, pos)
        except Exception as exc:
            print(f"state_err {exc}")
    else:
        print("orders")
        print("state file not found")
        print("positions")

    print("\nrecent hermes trade reports")
    files = sorted(glob.glob(str(Path(args.report_dir) / "*trade-report.json")))
    selected = []
    for path in files:
        try:
            report = load_json(path)
            if matches_market(report.get("market_slug") or Path(path).name, args.market):
                selected.append((path, report))
        except Exception as exc:
            print(f"{path} {exc}")
    for path, report in selected[-args.limit:]:
        print(
            Path(path).name,
            report.get("event_type"),
            report.get("side"),
            report.get("outcome"),
            report.get("phase"),
            report.get("price"),
            report.get("amount_usd"),
            report.get("success"),
            report.get("reason"),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
