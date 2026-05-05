import type { DashboardStatus } from "./types";

export const symbols = ["BTC", "ETH", "SOL", "XRP"];

export const emptyStatus: DashboardStatus = {
  scanned_markets: 0,
  candidates: [],
  watched_markets: [],
  binance_books: {},
  latest_whale_signal: null,
  whale_signals: [],
  global_activity_score: 0,
  activities: [],
  last_snipe: null,
  dry_run: true,
  allow_live_buys: false,
  allow_live_sells: false,
  live_max_order_usd: 1,
  live_order_type: "FAK",
  snipe_max_position_usd: 1,
  active_symbols: [],
  wallet_configured: false,
  funder_address: "",
  signature_type: null,
  enable_llm_market_reports: false,
  llm_api_base: "https://api.openai.com/v1",
  llm_api_key_configured: false,
  llm_model: "",
  llm_report_dir: "/var/lib/trading-bot/llm-reports",
  llm_code_patch_mode: "proposal_only",
  wallet: {
    positions_count: 0,
    open_orders: [],
    error: "wallet not loaded",
  },
};
