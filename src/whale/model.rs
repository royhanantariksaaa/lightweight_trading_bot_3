use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct BookTicker {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "b")]
    pub bid_price: String,
    #[serde(rename = "a")]
    pub ask_price: String,
}

#[derive(Debug, Deserialize)]
pub struct AggTrade {
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "q")]
    pub quantity: String,
    #[serde(rename = "m")]
    pub buyer_is_market_maker: bool,
    #[serde(rename = "T")]
    pub trade_time_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DepthUpdate {
    #[serde(rename = "s")]
    pub symbol: Option<String>,
    #[serde(rename = "b", alias = "bids", default)]
    pub bids: Vec<[String; 2]>,
    #[serde(rename = "a", alias = "asks", default)]
    pub asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
pub struct CombinedStream {
    pub stream: String,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub struct PriceState {
    pub mid: f64,
}

#[derive(Debug, Clone)]
pub struct BookLevel {
    pub price: f64,
    pub qty: f64,
}

#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

#[derive(Debug, Clone)]
pub struct PriceSnapshot {
    pub timestamp_ms: i64,
    pub price: f64,
}

#[derive(Debug, Clone)]
pub struct WhaleEvent {
    pub timestamp_ms: i64,
    pub timestamp: String,
    pub tier: String,
    pub market: String,
    pub symbol: String,
    pub side: String,
    pub trade_price: f64,
    pub quantity: f64,
    pub notional_usd: f64,
    pub target_price: f64,
}

#[derive(Debug, Clone)]
pub struct FlowTracker {
    pub market: String,
    pub symbol: String,
    pub whale_side: String,
    pub start_time_ms: i64,
    pub required_notional: f64,
    pub buy_notional: f64,
    pub sell_notional: f64,
    pub last_printed_bucket: i32,
    pub last_signal: String,
}

#[derive(Debug, Clone)]
pub struct WallInfo {
    pub price: f64,
    pub notional_usd: f64,
}

#[derive(Debug, Clone)]
pub struct BookMetrics {
    pub largest_bid_wall: Option<WallInfo>,
    pub largest_ask_wall: Option<WallInfo>,
    pub imbalance_pct: f64,
    pub need_up_10: f64,
    pub need_down_10: f64,
}

#[derive(Debug, Default)]
pub struct MarketState {
    pub prices: HashMap<String, PriceState>,
    pub books: HashMap<String, OrderBook>,
    pub history: HashMap<String, Vec<PriceSnapshot>>,
    pub trackers: HashMap<String, FlowTracker>,
}

pub enum StreamEvent {
    BookTicker(BookTicker),
    Depth(DepthUpdate),
    AggTrade(AggTrade),
    Ignore,
}
