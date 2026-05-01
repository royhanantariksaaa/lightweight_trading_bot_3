# Lightweight Trading Bot 3

A deliberately small, safety-first Polymarket bot scaffold.

The main design goal is to remove the behavior that caused problems in the larger bot:

- no portfolio-level auto-sell loop
- no auto take-profit by default
- no auto redeem by default
- no strategy can sell manual positions
- dry-run by default
- one small control loop
- explicit user settings win over environment defaults

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
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
cp .env.example .env
python -m lightweight_bot.main
```

## Strategy design

This repo is meant to implement an inventory-aware order-flow / market-making strategy in a controlled way:

1. Observe order books and compute a score.
2. Require signal persistence before quoting.
3. Place at most one maker buy per market/outcome.
4. Cancel stale maker orders.
5. Never auto-sell manual positions.
6. Only sell bot-owned positions if explicitly enabled.

The first implementation is intentionally conservative and mostly logs decisions until live trading is deliberately enabled.
