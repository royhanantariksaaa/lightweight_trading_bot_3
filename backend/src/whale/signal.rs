use crate::config::Settings;
use crate::dashboard::{WhaleSignal, WhaleWallInfo};
use crate::state::now_ms;

use super::book::{calculate_book_metrics, required_buy_notional, required_sell_notional};
use super::config::MarketConfig;
use super::model::{AggTrade, FlowTracker, MarketState, WallInfo, WhaleEvent};
use super::state::converted_price;
use super::tracker::tracker_signal;
use super::util::{format_timestamp, side_from_m_flag};

pub fn detect_whale(
    settings: &Settings,
    market: &MarketConfig,
    trade: &AggTrade,
    state: &MarketState,
    previous_price: Option<f64>,
) -> Option<WhaleEvent> {
    let raw_price = trade.price.parse::<f64>().ok()?;
    let quantity = trade.quantity.parse::<f64>().ok()?;
    if raw_price <= 0.0 || quantity <= 0.0 {
        return None;
    }

    let trade_price = converted_price(state, market, raw_price);
    let notional_usd = trade_price * quantity;
    let tier = whale_tier(settings, notional_usd);
    if tier == "IGNORE" {
        return None;
    }

    Some(WhaleEvent {
        timestamp_ms: trade.trade_time_ms.unwrap_or_else(now_ms),
        timestamp: format_timestamp(trade.trade_time_ms),
        tier: tier.to_string(),
        market: market.market.clone(),
        symbol: market.symbol.clone(),
        side: side_from_m_flag(trade.buyer_is_market_maker).to_string(),
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
        required_notional,
        buy_notional: 0.0,
        sell_notional: 0.0,
        last_printed_bucket: -1,
        last_signal: "NEW".to_string(),
    }
}

pub fn signal_from_event(
    settings: &Settings,
    event: &WhaleEvent,
    market: &MarketConfig,
    state: &MarketState,
    tracker: &FlowTracker,
) -> WhaleSignal {
    let metrics = state
        .books
        .get(&event.symbol)
        .and_then(|book| calculate_book_metrics(settings, market, event.trade_price, book, state));

    WhaleSignal {
        timestamp: event.timestamp.clone(),
        market: event.market.clone(),
        symbol: event.symbol.clone(),
        side: event.side.clone(),
        tier: event.tier.clone(),
        trade_price: event.trade_price,
        quantity: event.quantity,
        notional_usd: event.notional_usd,
        target_price: event.target_price,
        required_notional: tracker.required_notional,
        signal: tracker_signal(tracker),
        imbalance_pct: metrics.as_ref().map(|m| m.imbalance_pct).unwrap_or(0.0),
        bid_wall: metrics
            .as_ref()
            .and_then(|m| m.largest_bid_wall.clone())
            .map(wall_signal),
        ask_wall: metrics
            .as_ref()
            .and_then(|m| m.largest_ask_wall.clone())
            .map(wall_signal),
        need_up_10: metrics.as_ref().map(|m| m.need_up_10).unwrap_or(0.0),
        need_down_10: metrics.as_ref().map(|m| m.need_down_10).unwrap_or(0.0),
    }
}

pub fn trade_notional(
    market: &MarketConfig,
    trade: &AggTrade,
    state: &MarketState,
) -> Option<(String, f64)> {
    let raw_price = trade.price.parse::<f64>().ok()?;
    let qty = trade.quantity.parse::<f64>().ok()?;
    if raw_price <= 0.0 || qty <= 0.0 {
        return None;
    }
    Some((
        side_from_m_flag(trade.buyer_is_market_maker).to_string(),
        converted_price(state, market, raw_price) * qty,
    ))
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
        "SELL" => required_buy_notional(market, book, event.trade_price, event.target_price, state),
        "BUY" => required_sell_notional(market, book, event.trade_price, event.target_price, state),
        _ => 0.0,
    }
}

fn wall_signal(wall: WallInfo) -> WhaleWallInfo {
    WhaleWallInfo {
        price: wall.price,
        notional_usd: wall.notional_usd,
    }
}

fn whale_tier(settings: &Settings, notional: f64) -> &'static str {
    if notional >= settings.whale_super_usd {
        "SUPER_WHALE"
    } else if notional >= settings.whale_usd {
        "WHALE"
    } else if notional >= settings.whale_mini_usd {
        "MINI_WHALE"
    } else {
        "IGNORE"
    }
}
