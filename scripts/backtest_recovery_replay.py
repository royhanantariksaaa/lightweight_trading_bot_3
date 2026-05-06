#!/usr/bin/env python3
"""Replay recent trade reports against market-recorder JSONL snapshots.

This is not a perfect exchange simulator. It validates whether the new recovery/hold
logic would have had market context available: entry price, post-entry cliff, bounce,
book support, and realized BUY->SELL PnL from Hermes trade reports.
"""
import argparse
import csv
import datetime as dt
import glob
import json
import os
from collections import defaultdict, deque

REPORT_DIR_DEFAULT = "/var/lib/trading-bot/hermes-reports"
RECORDER_DIR_DEFAULT = "/var/lib/trading-bot/market-recorder"


def parse_ts(value):
    if not value:
        return None
    if isinstance(value, (int, float)):
        return dt.datetime.fromtimestamp(value / 1000 if value > 10_000_000_000 else value, tz=dt.timezone.utc)
    value = str(value).replace("Z", "+00:00")
    try:
        parsed = dt.datetime.fromisoformat(value)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed.astimezone(dt.timezone.utc)


def load_reports(report_dir, start, end):
    events = []
    for path in sorted(glob.glob(os.path.join(report_dir, "*-trade-report.json"))):
        try:
            with open(path) as f:
                data = json.load(f)
        except Exception:
            continue
        ts = parse_ts(data.get("generated_at"))
        if not ts or (start and ts < start) or (end and ts > end):
            continue
        if data.get("success") is not True:
            continue
        side = str(data.get("side") or "").upper()
        if side not in {"BUY", "SELL"}:
            continue
        events.append({
            "ts": ts,
            "path": path,
            "market": data.get("market_slug") or "",
            "outcome": data.get("outcome") or "",
            "side": side,
            "phase": data.get("phase") or "",
            "price": float(data.get("price") or 0),
            "shares": float(data.get("shares") or 0),
            "amount_usd": float(data.get("amount_usd") or 0),
            "reason": data.get("reason") or "",
        })
    return events


def iter_frames(recorder_dir, start, end):
    paths = sorted(glob.glob(os.path.join(recorder_dir, "market-recorder-*.jsonl")))
    for path in paths:
        try:
            with open(path) as f:
                for line in f:
                    try:
                        frame = json.loads(line)
                    except Exception:
                        continue
                    ts = parse_ts(frame.get("timestamp") or frame.get("timestamp_ms"))
                    if not ts or (start and ts < start) or (end and ts > end):
                        continue
                    yield ts, frame
        except FileNotFoundError:
            continue


def outcome_price(market, outcome):
    for o in market.get("outcomes") or []:
        if str(o.get("name") or "").lower() == outcome.lower():
            return first_float(o.get("best_ask"), o.get("best_bid"), o.get("price"))
    return None


def first_float(*values):
    for value in values:
        try:
            if value is not None:
                return float(value)
        except Exception:
            pass
    return None


def symbol_from_slug(slug):
    return (slug.split("-")[0] if slug else "").upper()


def book_support(frame, slug, outcome):
    sym = symbol_from_slug(slug)
    books = frame.get("binance_books") or {}
    book = books.get(sym) or books.get(sym + "USDT") or {}
    imbalance = first_float(book.get("imbalance_pct")) or 0.0
    return imbalance / 100.0 if outcome.lower() == "up" else -imbalance / 100.0


def index_frames(recorder_dir, start, end):
    by_market = defaultdict(list)
    for ts, frame in iter_frames(recorder_dir, start, end):
        for market in frame.get("watched_markets") or []:
            slug = market.get("slug") or ""
            if slug:
                by_market[slug].append((ts, frame, market))
    return by_market


def pair_trades(events):
    queues = defaultdict(deque)
    pairs = []
    open_buys = []
    for ev in sorted(events, key=lambda e: e["ts"]):
        key = (ev["market"], ev["outcome"].lower())
        if ev["side"] == "BUY":
            queues[key].append(ev)
        else:
            if queues[key]:
                buy = queues[key].popleft()
                pairs.append((buy, ev))
    for q in queues.values():
        open_buys.extend(q)
    return pairs, open_buys


def analyze_pair(buy, sell, frames):
    market_frames = frames.get(buy["market"], [])
    window = [(ts, frame, market) for ts, frame, market in market_frames if buy["ts"] <= ts <= sell["ts"]]
    prices = [outcome_price(market, buy["outcome"]) for _, _, market in window]
    prices = [p for p in prices if p is not None]
    supports = [book_support(frame, buy["market"], buy["outcome"]) for _, frame, _ in window]
    cost = buy["price"] * buy["shares"] if buy["price"] and buy["shares"] else buy["amount_usd"]
    proceeds = sell["price"] * min(buy["shares"], sell["shares"])
    pnl = proceeds - cost
    return {
        "buy_ts": buy["ts"].isoformat(),
        "sell_ts": sell["ts"].isoformat(),
        "market": buy["market"],
        "outcome": buy["outcome"],
        "phase": buy["phase"],
        "buy_price": buy["price"],
        "sell_price": sell["price"],
        "shares": buy["shares"],
        "pnl_usd_est": pnl,
        "pnl_pct_est": (sell["price"] - buy["price"]) / buy["price"] if buy["price"] else 0,
        "min_recorded_price": min(prices) if prices else None,
        "max_recorded_price": max(prices) if prices else None,
        "max_cliff_pct": (min(prices) - buy["price"]) / buy["price"] if prices and buy["price"] else None,
        "max_bounce_pct": (max(prices) - buy["price"]) / buy["price"] if prices and buy["price"] else None,
        "avg_book_support": sum(supports) / len(supports) if supports else None,
        "sell_reason": sell["reason"],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reports", default=REPORT_DIR_DEFAULT)
    ap.add_argument("--recorder", default=RECORDER_DIR_DEFAULT)
    ap.add_argument("--hours", type=float, default=6)
    ap.add_argument("--start")
    ap.add_argument("--end")
    ap.add_argument("--csv", default="/tmp/polymarket_recovery_backtest.csv")
    args = ap.parse_args()

    end = parse_ts(args.end) or dt.datetime.now(dt.timezone.utc)
    start = parse_ts(args.start) or (end - dt.timedelta(hours=args.hours))

    events = load_reports(args.reports, start, end)
    pairs, open_buys = pair_trades(events)
    frames = index_frames(args.recorder, start - dt.timedelta(minutes=5), end + dt.timedelta(minutes=5))
    rows = [analyze_pair(b, s, frames) for b, s in pairs]

    total_pnl = sum(r["pnl_usd_est"] for r in rows)
    losers = [r for r in rows if r["pnl_usd_est"] < 0]
    cliff_bounce = [r for r in rows if (r["max_cliff_pct"] or 0) <= -0.10 and (r["max_bounce_pct"] or 0) >= 0.03]

    if rows:
        with open(args.csv, "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
            writer.writeheader()
            writer.writerows(rows)

    print(f"window={start.isoformat()} -> {end.isoformat()}")
    print(f"events={len(events)} paired={len(rows)} open_buys={len(open_buys)} recorder_markets={len(frames)}")
    print(f"estimated_realized_pnl=${total_pnl:+.2f} losers={len(losers)} cliff_then_bounce_candidates={len(cliff_bounce)}")
    print(f"csv={args.csv if rows else 'none'}")
    for r in sorted(rows, key=lambda x: x["pnl_usd_est"])[:10]:
        print(
            f"{r['buy_ts']} {r['market']} {r['outcome']} {r['phase']} "
            f"{r['buy_price']:.2f}->{r['sell_price']:.2f} pnl=${r['pnl_usd_est']:+.2f} "
            f"cliff={pct(r['max_cliff_pct'])} bounce={pct(r['max_bounce_pct'])} "
            f"book={pct(r['avg_book_support'])} reason={r['sell_reason']}"
        )


def pct(v):
    return "n/a" if v is None else f"{v*100:+.1f}%"


if __name__ == "__main__":
    main()
