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
  last_error?: string | null;
  dry_run: boolean;
  allow_live_buys: boolean;
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
