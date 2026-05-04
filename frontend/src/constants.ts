import type { DashboardStatus } from "./types";

export const symbols = ["BTC", "ETH", "SOL", "XRP"];

export const emptyStatus: DashboardStatus = {
  scanned_markets: 0,
  candidates: [],
  watched_markets: [],
  latest_whale_signal: null,
  whale_signals: [],
  dry_run: true,
  allow_live_buys: false,
  live_max_order_usd: 5,
  wallet_configured: false,
  funder_address: "",
  signature_type: null,
  wallet: {
    positions_count: 0,
    open_orders: [],
    error: "wallet not loaded",
  },
};
