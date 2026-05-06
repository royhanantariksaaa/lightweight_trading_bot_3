#!/usr/bin/env python3
import argparse, json, pathlib, sys
from datetime import datetime, timezone


def parse_ts(raw):
    if raw.endswith('Z'):
        raw = raw[:-1] + '+00:00'
    return datetime.fromisoformat(raw).astimezone(timezone.utc)


def main():
    p = argparse.ArgumentParser(description='Replay market-recorder JSONL around a market/time window')
    p.add_argument('--dir', default='/var/lib/trading-bot/market-recorder')
    p.add_argument('--market', required=True, help='market slug substring or exact slug')
    p.add_argument('--start', help='ISO timestamp UTC, inclusive')
    p.add_argument('--end', help='ISO timestamp UTC, inclusive')
    p.add_argument('--limit', type=int, default=300)
    args = p.parse_args()

    start = parse_ts(args.start) if args.start else None
    end = parse_ts(args.end) if args.end else None
    rows = []
    for path in sorted(pathlib.Path(args.dir).glob('market-recorder-*.jsonl')):
        with path.open() as f:
            for line in f:
                try:
                    snap = json.loads(line)
                except Exception:
                    continue
                ts_raw = snap.get('timestamp') or snap.get('generated_at') or snap.get('ts')
                if not ts_raw:
                    continue
                try:
                    ts = parse_ts(ts_raw)
                except Exception:
                    continue
                if start and ts < start: continue
                if end and ts > end: continue
                markets = snap.get('watched_markets') or snap.get('markets') or []
                candidates = snap.get('candidates') or []
                for m in markets:
                    slug = m.get('slug', '')
                    if args.market not in slug:
                        continue
                    outs = m.get('outcomes') or []
                    prices = ','.join(f"{o.get('name')}={o.get('price')} ask={o.get('best_ask')} bid={o.get('best_bid')}" for o in outs)
                    symbol = slug.split('-')[0].upper()
                    book = (snap.get('binance_books') or {}).get(symbol+'USDT') or (snap.get('binance_books') or {}).get(symbol) or {}
                    cand = [c for c in candidates if c.get('market_slug') == slug]
                    rows.append({
                        'ts': ts.isoformat(),
                        'slug': slug,
                        'tte': m.get('seconds_to_expiry'),
                        'current': m.get('current_price'),
                        'ptb': m.get('price_to_beat'),
                        'outcomes': prices,
                        'imbalance_pct': book.get('imbalance_pct'),
                        'candidates': '; '.join(f"{c.get('phase')} {c.get('outcome')} px={c.get('price')} edge={c.get('expected_edge')}" for c in cand),
                    })
                    if len(rows) >= args.limit:
                        break
                if len(rows) >= args.limit:
                    break
        if len(rows) >= args.limit:
            break

    for r in rows:
        print(f"{r['ts']} {r['slug']} tte={r['tte']} cur={r['current']} ptb={r['ptb']} imb={r['imbalance_pct']} | {r['outcomes']} | {r['candidates']}")
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
