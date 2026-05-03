use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use tokio_tungstenite::connect_async;

const MINI_WHALE_USD: f64 = 5_000.0;
const WHALE_USD: f64 = 10_000.0;
const SUPER_WHALE_USD: f64 = 25_000.0;
const DEFAULT_CONTRACT_SIZE_USD: f64 = 100.0;
const TRACKING_WINDOW_MS: i64 = 5 * 60 * 1000;
const PRE_WHALE_LOOKBACK_MS: i64 = 5_000;
const PRICE_HISTORY_KEEP_MS: i64 = 30_000;
const PROGRESS_PRINT_STEP: f64 = 0.10;
const DEPTH_STREAM_SUFFIX: &str = "depth20@100ms";
const WALL_MIN_NOTIONAL_USD: f64 = 25_000.0;

mod config {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct MarketConfig {
        pub market: String,
        pub symbol: String,
        pub url: String,
        pub contract_size_usd: Option<f64>,
        pub uses_usdt_conversion: bool,
    }

    #[derive(Debug, Clone)]
    pub struct AppConfig {
        pub spot: MarketConfig,
        pub coinm: Option<MarketConfig>,
    }

    impl AppConfig {
        pub fn from_env() -> Self {
            let base = env::var("WHALE_SYMBOL")
                .unwrap_or_else(|_| "BTC".to_string())
                .to_ascii_uppercase();
            let spot_symbol = env::var("WHALE_SPOT_SYMBOL")
                .unwrap_or_else(|_| format!("{base}USDT"))
                .to_ascii_uppercase();

            let spot = MarketConfig::spot(&spot_symbol);
            let coinm = env::var("WHALE_COINM_SYMBOL")
                .ok()
                .or_else(|| (base == "BTC").then(|| "BTCUSD_PERP".to_string()))
                .map(|symbol| {
                    let contract_size = env::var("WHALE_COINM_CONTRACT_USD")
                        .ok()
                        .and_then(|x| x.parse::<f64>().ok())
                        .unwrap_or(super::DEFAULT_CONTRACT_SIZE_USD);
                    MarketConfig::coinm(&symbol.to_ascii_uppercase(), contract_size)
                });

            Self { spot, coinm }
        }

        pub fn markets(&self) -> Vec<MarketConfig> {
            let mut markets = vec![self.spot.clone()];
            if let Some(coinm) = &self.coinm {
                markets.push(coinm.clone());
            }
            markets
        }
    }

    impl MarketConfig {
        fn stream_symbol(symbol: &str) -> String {
            symbol.to_ascii_lowercase()
        }

        pub fn spot(symbol: &str) -> Self {
            let stream_symbol = Self::stream_symbol(symbol);
            let url = format!(
                "wss://stream.binance.com:9443/stream?streams=usdtusd@bookTicker/{stream_symbol}@bookTicker/{stream_symbol}@aggTrade/{stream_symbol}@{}",
                super::DEPTH_STREAM_SUFFIX
            );

            Self {
                market: format!("{symbol} SPOT"),
                symbol: symbol.to_string(),
                url,
                contract_size_usd: None,
                uses_usdt_conversion: symbol.ends_with("USDT"),
            }
        }

        pub fn coinm(symbol: &str, contract_size_usd: f64) -> Self {
            let stream_symbol = Self::stream_symbol(symbol);
            let url = format!(
                "wss://dstream.binance.com/stream?streams={stream_symbol}@bookTicker/{stream_symbol}@aggTrade/{stream_symbol}@{}",
                super::DEPTH_STREAM_SUFFIX
            );

            Self {
                market: format!("{symbol} COIN-M"),
                symbol: symbol.to_string(),
                url,
                contract_size_usd: Some(contract_size_usd),
                uses_usdt_conversion: false,
            }
        }
    }
}

mod model {
    use super::*;

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
        pub target_price: f64,
        pub whale_price: f64,
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
        pub need_up_5: f64,
        pub need_up_10: f64,
        pub need_up_25: f64,
        pub need_down_5: f64,
        pub need_down_10: f64,
        pub need_down_25: f64,
    }
}

mod util {
    use super::*;

    pub fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    pub fn timestamp_ms_or_now(ms: Option<i64>) -> i64 {
        ms.unwrap_or_else(now_ms)
    }

    pub fn format_timestamp(ms: Option<i64>) -> String {
        match ms.and_then(DateTime::<Utc>::from_timestamp_millis) {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        }
    }

    pub fn side_from_m_flag(buyer_is_market_maker: bool) -> &'static str {
        if buyer_is_market_maker { "SELL" } else { "BUY" }
    }

    pub fn whale_tier(notional: f64) -> &'static str {
        if notional >= super::SUPER_WHALE_USD {
            "SUPER_WHALE"
        } else if notional >= super::WHALE_USD {
            "WHALE"
        } else if notional >= super::MINI_WHALE_USD {
            "MINI_WHALE"
        } else {
            "IGNORE"
        }
    }

    pub fn symbol_from_stream(stream: &str) -> String {
        stream.split('@').next().unwrap_or("").to_ascii_uppercase()
    }
}

mod parser {
    use super::model::*;

    pub enum StreamEvent {
        BookTicker(BookTicker),
        Depth(DepthUpdate),
        AggTrade(AggTrade),
        Ignore,
    }

    pub fn parse_stream(text: &str) -> Option<(String, StreamEvent)> {
        let wrapper: CombinedStream = serde_json::from_str(text).ok()?;
        let event = if wrapper.stream.ends_with("@bookTicker") {
            serde_json::from_value(wrapper.data)
                .ok()
                .map(StreamEvent::BookTicker)?
        } else if wrapper.stream.contains(super::DEPTH_STREAM_SUFFIX) {
            serde_json::from_value(wrapper.data)
                .ok()
                .map(StreamEvent::Depth)?
        } else if wrapper.stream.ends_with("@aggTrade") {
            serde_json::from_value(wrapper.data)
                .ok()
                .map(StreamEvent::AggTrade)?
        } else {
            StreamEvent::Ignore
        };
        Some((wrapper.stream, event))
    }
}

mod state {
    use super::config::MarketConfig;
    use super::model::*;
    use super::util;
    use super::*;

    #[derive(Debug, Default)]
    pub struct MarketState {
        pub prices: HashMap<String, PriceState>,
        pub books: HashMap<String, OrderBook>,
        pub history: HashMap<String, Vec<PriceSnapshot>>,
        pub trackers: HashMap<String, FlowTracker>,
    }

    impl MarketState {
        pub fn remember_price(&mut self, symbol: &str, timestamp_ms: i64, price: f64) {
            let entries = self.history.entry(symbol.to_string()).or_default();
            entries.push(PriceSnapshot {
                timestamp_ms,
                price,
            });

            let cutoff = timestamp_ms - super::PRICE_HISTORY_KEEP_MS;
            entries.retain(|x| x.timestamp_ms >= cutoff);
        }

        pub fn price_before_whale(&self, symbol: &str, whale_time_ms: i64) -> Option<f64> {
            let target_time = whale_time_ms - super::PRE_WHALE_LOOKBACK_MS;
            self.history
                .get(symbol)?
                .iter()
                .filter(|x| x.timestamp_ms <= target_time)
                .min_by_key(|x| (target_time - x.timestamp_ms).abs())
                .map(|x| x.price)
        }

        pub fn update_orderbook(&mut self, stream: &str, depth: DepthUpdate) {
            let symbol = depth
                .symbol
                .unwrap_or_else(|| util::symbol_from_stream(stream));
            self.books.insert(
                symbol,
                OrderBook {
                    bids: parse_levels(&depth.bids),
                    asks: parse_levels(&depth.asks),
                },
            );
        }

        pub fn update_book_ticker(&mut self, book: BookTicker) {
            if let Some(state) = parse_mid_price(&book) {
                self.prices.insert(book.symbol, state);
            }
        }

        pub fn converted_price(&self, market: &MarketConfig, raw_price: f64) -> f64 {
            if market.uses_usdt_conversion {
                match self.prices.get("USDTUSD") {
                    Some(usdtusd) => raw_price * usdtusd.mid,
                    None => raw_price,
                }
            } else {
                raw_price
            }
        }

        pub fn notional(&self, market: &MarketConfig, raw_price: f64, qty: f64) -> f64 {
            match market.contract_size_usd {
                Some(contract_size) => qty * contract_size,
                None => self.converted_price(market, raw_price) * qty,
            }
        }
    }

    fn parse_mid_price(book: &BookTicker) -> Option<PriceState> {
        let bid: f64 = book.bid_price.parse().ok()?;
        let ask: f64 = book.ask_price.parse().ok()?;

        (bid > 0.0 && ask > 0.0).then(|| PriceState {
            mid: (bid + ask) / 2.0,
        })
    }

    fn parse_levels(raw: &[[String; 2]]) -> Vec<BookLevel> {
        raw.iter()
            .filter_map(|level| {
                let price: f64 = level[0].parse().ok()?;
                let qty: f64 = level[1].parse().ok()?;
                (price > 0.0 && qty > 0.0).then_some(BookLevel { price, qty })
            })
            .collect()
    }
}

mod book {
    use super::config::MarketConfig;
    use super::model::*;
    use super::state::MarketState;

    pub fn required_buy_notional(
        market: &MarketConfig,
        book: &OrderBook,
        current_price: f64,
        target_price: f64,
        state: &MarketState,
    ) -> f64 {
        if target_price <= current_price {
            return 0.0;
        }

        book.asks
            .iter()
            .filter_map(|level| {
                let p = state.converted_price(market, level.price);
                (p >= current_price && p <= target_price)
                    .then(|| state.notional(market, level.price, level.qty))
            })
            .sum()
    }

    pub fn required_sell_notional(
        market: &MarketConfig,
        book: &OrderBook,
        current_price: f64,
        target_price: f64,
        state: &MarketState,
    ) -> f64 {
        if target_price >= current_price {
            return 0.0;
        }

        book.bids
            .iter()
            .filter_map(|level| {
                let p = state.converted_price(market, level.price);
                (p <= current_price && p >= target_price)
                    .then(|| state.notional(market, level.price, level.qty))
            })
            .sum()
    }

    pub fn calculate_metrics(
        market: &MarketConfig,
        current_price: f64,
        book: &OrderBook,
        state: &MarketState,
    ) -> Option<BookMetrics> {
        book.bids.first()?;
        book.asks.first()?;

        let total_bid_notional: f64 = book
            .bids
            .iter()
            .map(|x| state.notional(market, x.price, x.qty))
            .sum();
        let total_ask_notional: f64 = book
            .asks
            .iter()
            .map(|x| state.notional(market, x.price, x.qty))
            .sum();
        let denom = total_bid_notional + total_ask_notional;
        let imbalance_pct = if denom > 0.0 {
            (total_bid_notional - total_ask_notional) / denom * 100.0
        } else {
            0.0
        };

        Some(BookMetrics {
            largest_bid_wall: largest_wall(market, &book.bids, state),
            largest_ask_wall: largest_wall(market, &book.asks, state),
            imbalance_pct,
            need_up_5: required_buy_notional(
                market,
                book,
                current_price,
                current_price + 5.0,
                state,
            ),
            need_up_10: required_buy_notional(
                market,
                book,
                current_price,
                current_price + 10.0,
                state,
            ),
            need_up_25: required_buy_notional(
                market,
                book,
                current_price,
                current_price + 25.0,
                state,
            ),
            need_down_5: required_sell_notional(
                market,
                book,
                current_price,
                current_price - 5.0,
                state,
            ),
            need_down_10: required_sell_notional(
                market,
                book,
                current_price,
                current_price - 10.0,
                state,
            ),
            need_down_25: required_sell_notional(
                market,
                book,
                current_price,
                current_price - 25.0,
                state,
            ),
        })
    }

    fn largest_wall(
        market: &MarketConfig,
        levels: &[BookLevel],
        state: &MarketState,
    ) -> Option<WallInfo> {
        levels
            .iter()
            .map(|level| WallInfo {
                price: state.converted_price(market, level.price),
                notional_usd: state.notional(market, level.price, level.qty),
            })
            .filter(|wall| wall.notional_usd >= super::WALL_MIN_NOTIONAL_USD)
            .max_by(|a, b| {
                a.notional_usd
                    .partial_cmp(&b.notional_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

mod detector {
    use super::book;
    use super::config::MarketConfig;
    use super::model::*;
    use super::state::MarketState;
    use super::util;

    pub fn detect_whale(
        market: &MarketConfig,
        trade: &AggTrade,
        state: &MarketState,
        previous_price: Option<f64>,
    ) -> Option<WhaleEvent> {
        let raw_price: f64 = trade.price.parse().ok()?;
        let quantity: f64 = trade.quantity.parse().ok()?;

        if raw_price <= 0.0 || quantity <= 0.0 {
            return None;
        }

        let trade_price = state.converted_price(market, raw_price);
        let notional_usd = state.notional(market, raw_price, quantity);
        let tier = util::whale_tier(notional_usd);

        if tier == "IGNORE" {
            return None;
        }

        Some(WhaleEvent {
            timestamp_ms: util::timestamp_ms_or_now(trade.trade_time_ms),
            timestamp: util::format_timestamp(trade.trade_time_ms),
            tier: tier.to_string(),
            market: market.market.clone(),
            symbol: market.symbol.clone(),
            side: util::side_from_m_flag(trade.buyer_is_market_maker).to_string(),
            trade_price,
            quantity,
            notional_usd,
            target_price: previous_price.unwrap_or(trade_price),
        })
    }

    pub fn create_tracker(
        event: &WhaleEvent,
        market: &MarketConfig,
        state: &MarketState,
    ) -> FlowTracker {
        let required_notional = calculate_required_notional(event, market, state);

        FlowTracker {
            market: event.market.clone(),
            symbol: event.symbol.clone(),
            whale_side: event.side.clone(),
            start_time_ms: event.timestamp_ms,
            target_price: event.target_price,
            whale_price: event.trade_price,
            required_notional,
            buy_notional: 0.0,
            sell_notional: 0.0,
            last_printed_bucket: -1,
            last_signal: "NEW".to_string(),
        }
    }

    fn calculate_required_notional(
        event: &WhaleEvent,
        market: &MarketConfig,
        state: &MarketState,
    ) -> f64 {
        let Some(book) = state.books.get(&event.symbol) else {
            return 0.0;
        };

        match event.side.as_str() {
            "SELL" => book::required_buy_notional(
                market,
                book,
                event.trade_price,
                event.target_price,
                state,
            ),
            "BUY" => book::required_sell_notional(
                market,
                book,
                event.trade_price,
                event.target_price,
                state,
            ),
            _ => 0.0,
        }
    }

    pub fn trade_notional(
        market: &MarketConfig,
        trade: &AggTrade,
        state: &MarketState,
    ) -> Option<(String, f64)> {
        let raw_price: f64 = trade.price.parse().ok()?;
        let qty: f64 = trade.quantity.parse().ok()?;

        if raw_price <= 0.0 || qty <= 0.0 {
            return None;
        }

        Some((
            util::side_from_m_flag(trade.buyer_is_market_maker).to_string(),
            state.notional(market, raw_price, qty),
        ))
    }
}

mod tracker {
    use super::model::*;

    pub fn update(tracker: &mut FlowTracker, trade_side: &str, trade_notional: f64) {
        if trade_side == "BUY" {
            tracker.buy_notional += trade_notional;
        } else {
            tracker.sell_notional += trade_notional;
        }
    }

    pub fn desired_flow(tracker: &FlowTracker) -> f64 {
        if tracker.whale_side == "SELL" {
            tracker.buy_notional
        } else {
            tracker.sell_notional
        }
    }

    pub fn opposite_flow(tracker: &FlowTracker) -> f64 {
        if tracker.whale_side == "SELL" {
            tracker.sell_notional
        } else {
            tracker.buy_notional
        }
    }

    pub fn progress(tracker: &FlowTracker) -> f64 {
        if tracker.required_notional <= 0.0 {
            0.0
        } else {
            desired_flow(tracker) / tracker.required_notional
        }
    }

    pub fn net_pressure(tracker: &FlowTracker) -> f64 {
        desired_flow(tracker) - opposite_flow(tracker)
    }

    pub fn signal(tracker: &FlowTracker) -> String {
        let p = progress(tracker);
        let net = net_pressure(tracker);

        if tracker.required_notional <= 0.0 {
            return "NO_LIQUIDITY_ESTIMATE".to_string();
        }

        match (tracker.whale_side.as_str(), p, net > 0.0) {
            ("SELL", p, true) if p >= 1.0 => "RECOVERY_CONFIRMED",
            ("BUY", p, true) if p >= 1.0 => "PULLBACK_CONFIRMED",
            ("SELL", p, true) if p >= 0.70 => "RECOVERY_LIKELY",
            ("BUY", p, true) if p >= 0.70 => "PULLBACK_LIKELY",
            ("SELL", p, _) if p >= 0.30 => "RECOVERY_BUILDING",
            ("BUY", p, _) if p >= 0.30 => "PULLBACK_BUILDING",
            ("SELL", _, _) => "RECOVERY_WEAK",
            _ => "PULLBACK_WEAK",
        }
        .to_string()
    }

    pub fn eta_seconds(tracker: &FlowTracker, now_ms: i64) -> Option<f64> {
        let elapsed_sec = ((now_ms - tracker.start_time_ms) as f64 / 1000.0).max(1.0);
        let flow_per_sec = desired_flow(tracker) / elapsed_sec;

        if flow_per_sec <= 0.0 {
            return None;
        }

        let remaining = (tracker.required_notional - desired_flow(tracker)).max(0.0);
        Some(remaining / flow_per_sec)
    }

    pub fn should_print_update(tracker: &mut FlowTracker) -> bool {
        let p = progress(tracker);
        let bucket = (p / super::PROGRESS_PRINT_STEP).floor() as i32;
        let signal = signal(tracker);
        let should_print = bucket > tracker.last_printed_bucket || signal != tracker.last_signal;

        if should_print {
            tracker.last_printed_bucket = bucket;
            tracker.last_signal = signal;
        }

        should_print
    }

    pub fn is_expired(tracker: &FlowTracker, now_ms: i64) -> bool {
        now_ms - tracker.start_time_ms > super::TRACKING_WINDOW_MS
    }
}

mod output {
    use super::book;
    use super::config::MarketConfig;
    use super::model::*;
    use super::state::MarketState;
    use super::tracker;
    use chrono::Utc;

    pub fn print_start(markets: &[MarketConfig]) {
        println!("Starting symbol-agnostic micro-whale + recovery + book wall tracker...");
        println!("MINI_WHALE:  ${:.2}", super::MINI_WHALE_USD);
        println!("WHALE:       ${:.2}", super::WHALE_USD);
        println!("SUPER_WHALE: ${:.2}", super::SUPER_WHALE_USD);
        println!(
            "Pre-whale target lookback: {}ms",
            super::PRE_WHALE_LOOKBACK_MS
        );
        println!("Wall minimum: ${:.2}", super::WALL_MIN_NOTIONAL_USD);
        for market in markets {
            println!("Market:      {} via {}", market.market, market.url);
        }
    }

    pub fn print_whale_event(event: &WhaleEvent, market: &MarketConfig, state: &MarketState) {
        let mode = if event.side == "SELL" {
            "RECOVERY"
        } else {
            "PULLBACK"
        };
        let Some(tracker) = state.trackers.get(&event.symbol) else {
            return;
        };

        println!("\n================ MARKET SIGNAL ================");
        println!("Time:        {}", event.timestamp);
        println!("Tier:        {}", event.tier);
        println!("Market:      {}", event.market);
        println!("Side:        {}", event.side);
        println!("Trade Price: {:.2}", event.trade_price);
        println!("Quantity:    {:.6}", event.quantity);
        println!("Notional:    ${:.2}", event.notional_usd);
        println!("{} Target: {:.2}", mode, tracker.target_price);
        println!("Need:        ${:.2}", tracker.required_notional);
        println!("Signal:      {}", tracker::signal(tracker));

        if let Some(book) = state.books.get(&event.symbol) {
            if let Some(metrics) = book::calculate_metrics(market, event.trade_price, book, state) {
                println!(
                    "Book:        bid_wall={} | ask_wall={} | imbalance={:.1}%",
                    print_wall(&metrics.largest_bid_wall),
                    print_wall(&metrics.largest_ask_wall),
                    metrics.imbalance_pct
                );
                println!(
                    "Move Need:   up5=${:.0} up10=${:.0} up25=${:.0} | down5=${:.0} down10=${:.0} down25=${:.0}",
                    metrics.need_up_5,
                    metrics.need_up_10,
                    metrics.need_up_25,
                    metrics.need_down_5,
                    metrics.need_down_10,
                    metrics.need_down_25
                );
            } else {
                println!("Book:        waiting for book metrics");
            }
        } else {
            println!("Book:        waiting for depth stream");
        }

        println!("===============================================");
    }

    pub fn print_tracker_update(
        tracker: &FlowTracker,
        market: &MarketConfig,
        now_ms: i64,
        state: &MarketState,
    ) {
        let eta = match tracker::eta_seconds(tracker, now_ms) {
            Some(v) if v.is_finite() => format!("{v:.1}s"),
            _ => "n/a".to_string(),
        };

        let book_brief = if let Some(book) = state.books.get(&tracker.symbol) {
            if let Some(metrics) = book::calculate_metrics(market, tracker.whale_price, book, state)
            {
                format!(
                    "imb={:.1}% up10=${:.0} down10=${:.0}",
                    metrics.imbalance_pct, metrics.need_up_10, metrics.need_down_10
                )
            } else {
                "book=n/a".to_string()
            }
        } else {
            "book=n/a".to_string()
        };

        println!(
            "[{}] {} | {} | progress={:.0}% | need=${:.0} | buy=${:.0} | sell=${:.0} | net=${:.0} | eta={} | {} | {}",
            Utc::now().format("%H:%M:%S"),
            tracker.market,
            if tracker.whale_side == "SELL" {
                "RECOVERY"
            } else {
                "PULLBACK"
            },
            tracker::progress(tracker) * 100.0,
            tracker.required_notional,
            tracker.buy_notional,
            tracker.sell_notional,
            tracker::net_pressure(tracker),
            eta,
            tracker::signal(tracker),
            book_brief
        );
    }

    fn print_wall(wall: &Option<WallInfo>) -> String {
        match wall {
            Some(w) => format!("${:.0}@{:.2}", w.notional_usd, w.price),
            None => "none".to_string(),
        }
    }
}

use config::{AppConfig, MarketConfig};
use detector::{create_tracker, detect_whale, trade_notional};
use model::AggTrade;
use parser::StreamEvent;
use state::MarketState;

fn handle_trade(market: &MarketConfig, trade: AggTrade, state: &mut MarketState) {
    let trade_ms = util::timestamp_ms_or_now(trade.trade_time_ms);
    let symbol = market.symbol.as_str();

    if state
        .trackers
        .get(symbol)
        .map(|tracker| tracker::is_expired(tracker, trade_ms))
        .unwrap_or(false)
    {
        state.trackers.remove(symbol);
    }

    if let Some((side, notional)) = trade_notional(market, &trade, state) {
        let mut should_remove = false;
        let mut should_print = false;

        if let Some(flow) = state.trackers.get_mut(symbol) {
            tracker::update(flow, &side, notional);
            should_print = tracker::should_print_update(flow);
            should_remove = tracker::signal(flow).ends_with("CONFIRMED");
        }

        if should_print {
            if let Some(flow) = state.trackers.get(symbol) {
                output::print_tracker_update(flow, market, trade_ms, state);
            }
        }

        if should_remove {
            state.trackers.remove(symbol);
        }
    }

    let raw_price = match trade.price.parse::<f64>() {
        Ok(price) => price,
        Err(_) => return,
    };
    let current_price = state.converted_price(market, raw_price);
    let previous_price = state.price_before_whale(symbol, trade_ms);

    if let Some(event) = detect_whale(market, &trade, state, previous_price) {
        let flow = create_tracker(&event, market, state);
        state.trackers.insert(symbol.to_string(), flow);
        output::print_whale_event(&event, market, state);
    }

    state.remember_price(symbol, trade_ms, current_price);
}

async fn run_market(
    market: MarketConfig,
    sender: tokio::sync::mpsc::UnboundedSender<(MarketConfig, String)>,
) {
    let (ws, _) = connect_async(&market.url)
        .await
        .unwrap_or_else(|err| panic!("Failed to connect to {} websocket: {err}", market.market));
    let (_, mut read) = ws.split();

    while let Some(Ok(message)) = read.next().await {
        if !message.is_text() {
            continue;
        }

        let text = message.to_text().unwrap_or("").to_string();
        if sender.send((market.clone(), text)).is_err() {
            break;
        }
    }
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");

    let config = AppConfig::from_env();
    let markets = config.markets();
    output::print_start(&markets);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for market in markets {
        tokio::spawn(run_market(market, tx.clone()));
    }
    drop(tx);

    let mut state = MarketState::default();

    while let Some((market, text)) = rx.recv().await {
        let Some((stream, event)) = parser::parse_stream(&text) else {
            continue;
        };

        match event {
            StreamEvent::BookTicker(book) => state.update_book_ticker(book),
            StreamEvent::Depth(depth) => state.update_orderbook(&stream, depth),
            StreamEvent::AggTrade(trade) => handle_trade(&market, trade, &mut state),
            StreamEvent::Ignore => {}
        }
    }
}
