export type Candidate = {
  market_slug: string;
  outcome: string;
  price: number;
  expected_edge: number;
  seconds_to_expiry: number;
  stake_usd: number;
  dry_run: boolean;
};

export type WatchedMarket = {
  slug: string;
  question: string;
  icon?: string | null;
  image?: string | null;
  seconds_to_expiry: number;
  volume: number;
  liquidity: number;
  price_to_beat?: number | null;
  current_price?: number | null;
  outcomes: MarketOutcome[];
};

export type MarketOutcome = {
  name: string;
  token_id?: string | null;
  price: number;
  best_bid?: number | null;
  best_ask?: number | null;
};

export type DashboardStatus = {
  last_scan_at?: string | null;
  scanned_markets: number;
  candidates: Candidate[];
  watched_markets: WatchedMarket[];
  latest_whale_signal?: WhaleSignal | null;
  whale_signals: WhaleSignal[];
  last_error?: string | null;
  dry_run: boolean;
  allow_live_buys: boolean;
  live_max_order_usd: number;
  wallet_configured: boolean;
  funder_address: string;
  signature_type?: number | null;
  wallet: WalletSnapshot;
};

export type WalletSnapshot = {
  address?: string | null;
  cash?: number | null;
  allowance?: number | null;
  position_value?: number | null;
  portfolio_value?: number | null;
  positions_count: number;
  open_orders: OpenOrderSnapshot[];
  updated_at?: string | null;
  error?: string | null;
};

export type OpenOrderSnapshot = {
  id: string;
  market: string;
  outcome: string;
  side: string;
  price: number;
  original_size: number;
  size_matched: number;
  created_at: string;
};

export type ManualOrderRequest = {
  market_slug: string;
  outcome: string;
  amount_usd: number;
};

export type ManualOrderResponse = {
  accepted: boolean;
  live: boolean;
  message: string;
  order_id?: string | null;
};

export type RuntimeSettingsUpdate = {
  dry_run: boolean;
  allow_live_buys: boolean;
  live_max_order_usd: number;
  funder_address: string;
  signature_type?: number | null;
  private_key?: string | null;
};

export type WhaleWallInfo = {
  price: number;
  notional_usd: number;
};

export type WhaleSignal = {
  timestamp: string;
  market: string;
  symbol: string;
  side: "BUY" | "SELL" | string;
  tier: string;
  trade_price: number;
  quantity: number;
  notional_usd: number;
  target_price: number;
  required_notional: number;
  signal: string;
  imbalance_pct: number;
  bid_wall?: WhaleWallInfo | null;
  ask_wall?: WhaleWallInfo | null;
  need_up_10: number;
  need_down_10: number;
};

export type LiveQuote = {
  best_bid?: number;
  best_ask?: number;
  updated_at_ms: number;
  message_timestamp_ms?: number;
};

export type LiveReferencePrice = {
  value: number;
  updated_at_ms: number;
  received_at_ms: number;
  source: ReferenceSource;
  signed_speed_per_second?: number;
  avg_speed_per_second?: number;
  sample_count: number;
  samples: ReferenceSample[];
};

export type ReferenceSource = "chainlink" | "scan";

export type ReferenceSample = {
  value: number;
  timestamp_ms: number;
};
