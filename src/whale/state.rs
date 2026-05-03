use super::PRICE_HISTORY_KEEP_MS;
use super::config::MarketConfig;
use super::model::{
    BookLevel, BookTicker, DepthUpdate, MarketState, OrderBook, PriceSnapshot, PriceState,
};
use super::util::symbol_from_stream;

pub fn update_book_ticker(state: &mut MarketState, book: BookTicker) {
    let Ok(bid) = book.bid_price.parse::<f64>() else {
        return;
    };
    let Ok(ask) = book.ask_price.parse::<f64>() else {
        return;
    };
    if bid <= 0.0 || ask <= 0.0 {
        return;
    }
    state.prices.insert(
        book.symbol,
        PriceState {
            mid: (bid + ask) / 2.0,
        },
    );
}

pub fn update_orderbook(state: &mut MarketState, stream: &str, depth: DepthUpdate) {
    let symbol = depth
        .symbol
        .unwrap_or_else(|| symbol_from_stream(stream).to_ascii_uppercase());
    state.books.insert(
        symbol,
        OrderBook {
            bids: parse_levels(&depth.bids),
            asks: parse_levels(&depth.asks),
        },
    );
}

pub fn converted_price(state: &MarketState, market: &MarketConfig, raw_price: f64) -> f64 {
    if market.uses_usdt_conversion {
        state
            .prices
            .get("USDTUSD")
            .map(|usdtusd| raw_price * usdtusd.mid)
            .unwrap_or(raw_price)
    } else {
        raw_price
    }
}

pub fn remember_price(state: &mut MarketState, symbol: &str, timestamp_ms: i64, price: f64) {
    let entries = state.history.entry(symbol.to_string()).or_default();
    entries.push(PriceSnapshot {
        timestamp_ms,
        price,
    });
    let cutoff = timestamp_ms - PRICE_HISTORY_KEEP_MS;
    entries.retain(|x| x.timestamp_ms >= cutoff);
}

pub fn price_before_whale(
    state: &MarketState,
    symbol: &str,
    whale_time_ms: i64,
    lookback_ms: i64,
) -> Option<f64> {
    let target_time = whale_time_ms - lookback_ms;
    state
        .history
        .get(symbol)?
        .iter()
        .filter(|x| x.timestamp_ms <= target_time)
        .min_by_key(|x| (target_time - x.timestamp_ms).abs())
        .map(|x| x.price)
}

fn parse_levels(raw: &[[String; 2]]) -> Vec<BookLevel> {
    raw.iter()
        .filter_map(|level| {
            let price = level[0].parse::<f64>().ok()?;
            let qty = level[1].parse::<f64>().ok()?;
            (price > 0.0 && qty > 0.0).then_some(BookLevel { price, qty })
        })
        .collect()
}
