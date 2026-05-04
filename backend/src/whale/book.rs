use crate::config::Settings;

use super::config::MarketConfig;
use super::model::{BookLevel, BookMetrics, MarketState, OrderBook, WallInfo};
use super::state::converted_price;

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
            let price = converted_price(state, market, level.price);
            (price >= current_price && price <= target_price).then(|| price * level.qty)
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
            let price = converted_price(state, market, level.price);
            (price <= current_price && price >= target_price).then(|| price * level.qty)
        })
        .sum()
}

pub fn calculate_book_metrics(
    settings: &Settings,
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
        .map(|x| converted_price(state, market, x.price) * x.qty)
        .sum();
    let total_ask_notional: f64 = book
        .asks
        .iter()
        .map(|x| converted_price(state, market, x.price) * x.qty)
        .sum();
    let denom = total_bid_notional + total_ask_notional;
    let imbalance_pct = if denom > 0.0 {
        (total_bid_notional - total_ask_notional) / denom * 100.0
    } else {
        0.0
    };

    Some(BookMetrics {
        largest_bid_wall: largest_wall(settings, market, &book.bids, state),
        largest_ask_wall: largest_wall(settings, market, &book.asks, state),
        imbalance_pct,
        need_up_10: required_buy_notional(market, book, current_price, current_price + 10.0, state),
        need_down_10: required_sell_notional(
            market,
            book,
            current_price,
            current_price - 10.0,
            state,
        ),
    })
}

fn largest_wall(
    settings: &Settings,
    market: &MarketConfig,
    levels: &[BookLevel],
    state: &MarketState,
) -> Option<WallInfo> {
    levels
        .iter()
        .map(|level| {
            let price = converted_price(state, market, level.price);
            WallInfo {
                price,
                notional_usd: price * level.qty,
            }
        })
        .filter(|wall| wall.notional_usd >= settings.whale_wall_min_usd)
        .max_by(|a, b| {
            a.notional_usd
                .partial_cmp(&b.notional_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}
