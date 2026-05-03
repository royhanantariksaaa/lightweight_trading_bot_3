import type { DashboardStatus } from "./types";

export const symbols = ["BTC", "ETH", "SOL", "XRP"];

export const emptyStatus: DashboardStatus = {
  scanned_markets: 0,
  candidates: [],
  watched_markets: [],
  dry_run: true,
  allow_live_buys: false,
};
