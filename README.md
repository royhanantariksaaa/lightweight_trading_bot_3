# Lightweight Trading Bot 3

A deliberately small, safety-first Polymarket bot scaffold written in Rust.

The main design goal is to remove the behavior that caused problems in the larger bot:

- no portfolio-level auto-sell loop
- no auto take-profit by default
- no auto redeem by default
- no strategy can sell manual positions
- dry-run by default
- one small control loop
- explicit user settings win over environment defaults

## What is included now

- Rust async bot runtime with `tokio`
- Polymarket Gamma market scanner
- Last-1-minute 5m crypto-market snipe scanner
- Lightweight local dashboard powered by `axum`
- JSON status endpoint at `/api/status`
- Dry-run candidate logging by default
- Guarded live buy execution through Polymarket's Rust CLOB V2 SDK

The snipe scanner looks for active 5-minute Polymarket crypto markets that are within the final configured window, default `60` seconds. It scores candidates with a conservative edge proxy based on time-to-expiry, liquidity/volume, and current outcome price. This is **not** an oracle or guaranteed-profitable prediction engine.

## Safety defaults

```env
DRY_RUN=true
ALLOW_LIVE_BUYS=false
ALLOW_LIVE_SELLS=false
ALLOW_CANCELS=true
AUTO_TAKE_PROFIT=false
AUTO_EXIT_NO_EDGE=false
AUTO_REDEEM=false
```

The bot is intentionally **buy-only** unless `ALLOW_LIVE_SELLS=true` is explicitly set. Even then, sells should only be allowed for positions tagged as bot-owned in local state.

## Suggested first run

```bash
cp .env.example .env
npm install
npm run dev
```

Then open the SolidJS dashboard:

```text
http://127.0.0.1:5173
```

`npm run dev` and `npm run full` both start the Rust backend and the SolidJS frontend together. The Rust API remains available at `http://127.0.0.1:8080/api/status`.

The dashboard exposes:

- last scan time
- number of 5m markets scanned
- current snipe candidates
- dry-run/live-buy status
- latest scanner error, if any

## Last-1-minute 5m snipe settings

```env
ENABLE_LAST_MINUTE_5M_SNIPE=true
SNIPE_WINDOW_SECONDS=60
SNIPE_MIN_EDGE=0.02
SNIPE_MAX_PRICE=0.96
SNIPE_MIN_VOLUME_USD=250
SNIPE_MIN_LIQUIDITY_USD=50
SNIPE_LIQUIDITY_SCALE_USD=5000
SNIPE_MAX_POSITION_USD=5
SNIPE_MAX_SIGNALS=8
```

## Strategy design

This repo is meant to implement an inventory-aware order-flow / market-making strategy in a controlled way:

1. Observe order books and compute a score.
2. Require signal persistence before quoting.
3. Place at most one maker buy per market/outcome.
4. Cancel stale maker orders.
5. Never auto-sell manual positions.
6. Only sell bot-owned positions if explicitly enabled.

The current implementation scans, scores, logs, displays potential 5m last-minute candidates, and can place guarded live buy orders through `polymarket_client_sdk_v2`. Live sells and generic strategy orders remain guarded until they are backed by real token-aware market data and reviewed risk controls.

## Live Polymarket CLOB V2 buys

Live execution is wired only for the last-minute 5m snipe scanner, because those Gamma market snapshots include CLOB outcome token ids. The generic placeholder strategy is still blocked from live trading until it is backed by real CLOB market data.

Disable dry-run, allow live buys, and set the wallet credentials used by the Rust SDK:

```env
DRY_RUN=false
ALLOW_LIVE_BUYS=true
POLYMARKET_PRIVATE_KEY=...
FUNDER_ADDRESS=...
SIGNATURE_TYPE=...
```

The Rust bot enforces `LIVE_MAX_ORDER_USD`, `LIVE_MIN_SECONDS_TO_EXPIRY`, and `LIVE_ORDER_COOLDOWN_MS` before signing and posting a limit order with `polymarket_client_sdk_v2`. The SDK derives API credentials from `POLYMARKET_PRIVATE_KEY` and fetches the token-specific CLOB metadata needed for V2 orders.
