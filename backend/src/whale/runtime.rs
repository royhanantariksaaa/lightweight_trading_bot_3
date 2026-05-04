use anyhow::Result;
use futures_util::StreamExt;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tracing::{info, warn};

use crate::config::Settings;
use crate::dashboard::{SharedDashboard, WhaleSignal};
use crate::state::now_ms;

use super::PRE_WHALE_LOOKBACK_MS;
use super::config::{MarketConfig, build_markets};
use super::model::{AggTrade, MarketState, StreamEvent};
use super::parser::parse_stream;
use super::signal::{create_tracker, detect_whale, signal_from_event, trade_notional};
use super::state::{
    converted_price, price_before_whale, remember_price, update_book_ticker, update_orderbook,
};
use super::tracker::{net_pressure, progress, should_print_tracker_update, tracker_signal};

pub async fn run_whale_detector(settings: Settings, dashboard: SharedDashboard) -> Result<()> {
    if !settings.enable_whale_detector {
        info!("whale detector disabled");
        return Ok(());
    }

    let markets = build_markets(&settings);
    if markets.is_empty() {
        warn!("whale detector has no symbols to watch");
        return Ok(());
    }

    info!(markets = ?markets.iter().map(|m| &m.symbol).collect::<Vec<_>>(), "starting whale detector");

    let (tx, mut rx) = mpsc::unbounded_channel();
    for market in markets {
        tokio::spawn(run_market_stream(market, tx.clone()));
    }
    drop(tx);

    let mut state = MarketState::default();
    let mut recent_signals = VecDeque::new();

    while let Some((market, text)) = rx.recv().await {
        let Some((stream, event)) = parse_stream(&text) else {
            continue;
        };

        match event {
            StreamEvent::BookTicker(book) => update_book_ticker(&mut state, book),
            StreamEvent::Depth(depth) => update_orderbook(&mut state, &stream, depth),
            StreamEvent::AggTrade(trade) => {
                if let Some(signal) =
                    handle_trade(&settings, &market, trade, &mut state, &mut recent_signals)
                {
                    let mut dashboard = dashboard.write().await;
                    dashboard.whale_signals = recent_signals.iter().cloned().collect();
                    dashboard.latest_whale_signal = Some(signal);
                }
            }
            StreamEvent::Ignore => {}
        }
    }

    Ok(())
}

async fn run_market_stream(
    market: MarketConfig,
    sender: mpsc::UnboundedSender<(MarketConfig, String)>,
) {
    loop {
        match connect_async(&market.url).await {
            Ok((ws, _)) => {
                info!(market = %market.market, "whale stream connected");
                let (_, mut read) = ws.split();

                while let Some(message) = read.next().await {
                    match message {
                        Ok(message) if message.is_text() => {
                            let text = message.to_text().unwrap_or("").to_string();
                            if sender.send((market.clone(), text)).is_err() {
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            warn!(%error, market = %market.market, "whale stream message error");
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                warn!(%error, market = %market.market, "failed to connect whale stream");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

fn handle_trade(
    settings: &Settings,
    market: &MarketConfig,
    trade: AggTrade,
    state: &mut MarketState,
    recent_signals: &mut VecDeque<WhaleSignal>,
) -> Option<WhaleSignal> {
    let trade_ms = trade.trade_time_ms.unwrap_or_else(now_ms);
    expire_tracker(settings, market, state, trade_ms);
    update_tracker_from_trade(market, &trade, state, trade_ms);

    let raw_price = trade.price.parse::<f64>().ok()?;
    let converted_price = converted_price(state, market, raw_price);
    let previous_price = price_before_whale(state, &market.symbol, trade_ms, PRE_WHALE_LOOKBACK_MS);
    let event = detect_whale(settings, market, &trade, state, previous_price)?;
    let tracker = create_tracker(&event, market, state);
    state
        .trackers
        .insert(market.symbol.clone(), tracker.clone());
    remember_price(state, &market.symbol, trade_ms, converted_price);

    let signal = signal_from_event(settings, &event, market, state, &tracker);
    recent_signals.push_front(signal.clone());
    recent_signals.truncate(25);
    info!(?signal, "whale signal");
    Some(signal)
}

fn expire_tracker(
    settings: &Settings,
    market: &MarketConfig,
    state: &mut MarketState,
    trade_ms: i64,
) {
    if state
        .trackers
        .get(&market.symbol)
        .map(|tracker| trade_ms - tracker.start_time_ms > settings.whale_tracking_window_ms)
        .unwrap_or(false)
    {
        state.trackers.remove(&market.symbol);
    }
}

fn update_tracker_from_trade(
    market: &MarketConfig,
    trade: &AggTrade,
    state: &mut MarketState,
    trade_ms: i64,
) {
    let Some((side, notional)) = trade_notional(market, trade, state) else {
        return;
    };

    let mut should_remove = false;
    let mut should_log = false;
    if let Some(tracker) = state.trackers.get_mut(&market.symbol) {
        if side == "BUY" {
            tracker.buy_notional += notional;
        } else {
            tracker.sell_notional += notional;
        }
        should_log = should_print_tracker_update(tracker);
        should_remove = tracker_signal(tracker).ends_with("CONFIRMED");
    }

    if should_log && let Some(tracker) = state.trackers.get(&market.symbol) {
        info!(
            market = %tracker.market,
            symbol = %tracker.symbol,
            signal = %tracker_signal(tracker),
            progress = progress(tracker) * 100.0,
            buy_notional = tracker.buy_notional,
            sell_notional = tracker.sell_notional,
            net = net_pressure(tracker),
            trade_ms,
            "whale flow update"
        );
    }

    if should_remove {
        state.trackers.remove(&market.symbol);
    }
}
