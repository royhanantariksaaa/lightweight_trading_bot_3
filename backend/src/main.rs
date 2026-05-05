mod config;
mod dashboard;
mod hermes_reporter;
mod live;
mod llm;
mod polymarket;
mod snipe;
mod state;
mod strategy;
mod whale;

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::config::Settings;
use crate::dashboard::{DashboardState, SharedDashboard, serve_dashboard};
use crate::hermes_reporter::HermesReporter;
use crate::live::{
    buy_request_from_snipe, cancel_live_order, fetch_wallet_snapshot, hide_stale_display_orders,
    post_live_order, redeem_winnings, sell_request_from_position,
};
use crate::llm::TradeExecutionReport;
use crate::polymarket::PolymarketClient;
use crate::snipe::{WhaleContext, find_phase1_whale_ride_signals, find_phase2_snipe_signals};
use crate::state::{BotState, now_ms};
use crate::strategy::{Decision, StrategyContext, evaluate_strategy};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let settings = Settings::from_env()?;
    settings.log_safety_summary();
    let runtime_settings = Arc::new(RwLock::new(settings.clone()));

    let dashboard_state: SharedDashboard = Arc::new(RwLock::new(DashboardState {
        dry_run: settings.dry_run,
        allow_live_buys: settings.allow_live_buys,
        allow_live_sells: settings.allow_live_sells,
        live_max_order_usd: settings.live_max_order_usd,
        live_order_type: settings.live_order_type.clone(),
        snipe_max_position_usd: settings.snipe_max_position_usd,
        wallet_configured: settings.polymarket_private_key.is_some(),
        funder_address: settings.polymarket_funder_address.clone(),
        signature_type: settings.polymarket_signature_type,
        enable_llm_market_reports: settings.enable_llm_market_reports,
        llm_api_base: settings.llm_api_base.clone(),
        llm_api_key_configured: settings.llm_api_key.is_some(),
        llm_model: settings.llm_model.clone(),
        llm_report_dir: settings.hermes_report_dir.display().to_string(),
        llm_code_patch_mode: settings.llm_code_patch_mode.clone(),
        active_symbols: settings.active_symbols.clone(),
        ..DashboardState::default()
    }));

    let dashboard_settings = runtime_settings.clone();
    let whale_settings = settings.clone();
    let bot_settings = runtime_settings.clone();
    let dashboard_handle = tokio::spawn({
        let dashboard_state = dashboard_state.clone();
        async move {
            if let Err(error) = serve_dashboard(dashboard_settings, dashboard_state).await {
                warn!(%error, "dashboard stopped");
            }
        }
    });

    let wallet_settings = runtime_settings.clone();
    let wallet_dashboard = dashboard_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let settings = wallet_settings.read().await.clone();
            let mut wallet = fetch_wallet_snapshot(&settings).await;
            hide_stale_display_orders(&settings, &mut wallet);
            wallet_dashboard.write().await.wallet = wallet;
        }
    });

    let bot_handle = tokio::spawn(run_bot(bot_settings, dashboard_state.clone()));
    let whale_handle = if whale_settings.enable_whale_detector {
        let whale_dashboard_state = dashboard_state.clone();
        Some(tokio::spawn(async move {
            if let Err(error) =
                whale::run_whale_detector(whale_settings, whale_dashboard_state).await
            {
                warn!(%error, "whale detector stopped");
            }
        }))
    } else {
        None
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown requested");
        }
        result = bot_handle => {
            result??;
        }
        _ = dashboard_handle => {
            warn!("dashboard task exited");
        }
        _ = wait_optional_task(whale_handle) => {
            warn!("whale detector task exited");
        }
    }

    Ok(())
}

async fn wait_optional_task(handle: Option<tokio::task::JoinHandle<()>>) {
    match handle {
        Some(handle) => {
            let _ = handle.await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn run_bot(
    runtime_settings: Arc<RwLock<Settings>>,
    dashboard_state: SharedDashboard,
) -> Result<()> {
    let initial_settings = runtime_settings.read().await.clone();
    let mut state = BotState::load_or_default(&initial_settings.state_path).await?;
    let polymarket = PolymarketClient::new(&initial_settings)?;
    let hermes_reporter = HermesReporter::new(&initial_settings);
    let mut last_seen_markets: HashMap<String, crate::polymarket::MarketSnapshot> = HashMap::new();
    let mut last_live_order_ms = 0_i64;
    let mut last_phase1_order_ms = 0_i64;
    let mut last_redeem_ms = 0_i64;

    loop {
        let settings = runtime_settings.read().await.clone();
        if settings.enable_last_minute_5m_snipe {
            let fetch_result = tokio::time::timeout(
                Duration::from_secs(20),
                polymarket.fetch_active_5m_markets(&settings),
            )
            .await;
            match fetch_result {
                Ok(Ok(markets)) => {
                    let active_slugs = markets
                        .iter()
                        .map(|market| market.slug.clone())
                        .collect::<HashSet<_>>();
                    let (resolved_orders, resolved_positions) =
                        state.clear_inactive_markets(&active_slugs);
                    if resolved_orders > 0 || resolved_positions > 0 {
                        info!(
                            resolved_orders,
                            resolved_positions, "cleared local bot state for inactive markets"
                        );
                    }
                    let closed_observed = last_seen_markets
                        .values()
                        .filter(|market| {
                            market
                                .end_time
                                .map(|end_time| {
                                    end_time <= Utc::now() - ChronoDuration::seconds(10)
                                })
                                .unwrap_or(false)
                                && !state.closed_market_reported(&market.slug)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    for observed in closed_observed {
                        let whale_signals = dashboard_state.read().await.whale_signals.clone();
                        match polymarket.fetch_closed_market_snapshot(&observed).await {
                            Ok(final_market) => match hermes_reporter
                                .report_closed_market(
                                    &settings,
                                    observed.clone(),
                                    final_market,
                                    &state,
                                    whale_signals,
                                )
                                .await
                            {
                                Ok(true) => {
                                    info!(slug = %observed.slug, "closed market reported to LLM");
                                    state.mark_closed_market_reported(observed.slug);
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    warn!(%error, slug = %observed.slug, "closed market LLM report failed")
                                }
                            },
                            Err(error) => {
                                warn!(%error, slug = %observed.slug, "failed to fetch closed market snapshot")
                            }
                        }
                    }

                    // Read whale signals and Binance books from dashboard to inform directional bias
                    let dash_snapshot = dashboard_state.read().await;
                    let whale_ctx = WhaleContext {
                        signals: dash_snapshot.whale_signals.clone(),
                        binance_books: dash_snapshot.binance_books.clone(),
                    };
                    drop(dash_snapshot);

                    let filtered_markets = if settings.active_symbols.is_empty() {
                        markets.clone()
                    } else {
                        markets
                            .iter()
                            .filter(|market| {
                                let symbol = market
                                    .slug
                                    .split('-')
                                    .next()
                                    .map(str::to_ascii_uppercase)
                                    .unwrap_or_default();
                                settings.active_symbols.contains(&symbol)
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    };

                    let mut signals =
                        find_phase2_snipe_signals(&settings, &filtered_markets, &whale_ctx);
                    signals.extend(find_phase1_whale_ride_signals(
                        &settings,
                        &filtered_markets,
                        &whale_ctx,
                    ));
                    let mut seen = HashSet::new();
                    signals.retain(|signal| seen.insert(signal.market_slug.clone()));
                    let mut wallet = dashboard_state.read().await.wallet.clone();
                    for order in stale_live_orders(&wallet, settings.maker_order_ttl_ms) {
                        match cancel_live_order(&settings, &order.id).await {
                            Ok(response) if response.canceled => {
                                info!(order_id = %order.id, "cancelled stale live Polymarket order");
                                state.mark_order_cancelled(&order.id);
                                dashboard_state.write().await.push_activity(
                                    "warn",
                                    "Stale Order Cancelled",
                                    Some(&format!(
                                        "{} {} @ {:.2}",
                                        order.outcome, order.side, order.price
                                    )),
                                );
                            }
                            Ok(response) => {
                                warn!(order_id = %order.id, raw = ?response.raw, "stale order cancel was not accepted");
                            }
                            Err(error) => {
                                warn!(order_id = %order.id, %error, "failed to cancel stale live order");
                            }
                        }
                    }
                    hide_stale_display_orders(&settings, &mut wallet);
                    let wallet_order_ids = wallet
                        .open_orders
                        .iter()
                        .map(|order| order.id.clone())
                        .collect::<HashSet<_>>();
                    let tracked_open = state
                        .bot_orders
                        .values()
                        .filter(|order| order.status == "open")
                        .map(|order| {
                            (
                                order.id.clone(),
                                order.market_slug.clone(),
                                order.outcome.clone(),
                                order.limit_price,
                                order.shares,
                                order.phase.clone(),
                            )
                        })
                        .collect::<Vec<_>>();
                    for (order_id, slug, outcome, price, shares, phase) in tracked_open {
                        if !wallet_order_ids.contains(&order_id)
                            && !state.bot_owns_position(&slug, &outcome)
                            && active_slugs.contains(&slug)
                        {
                            info!(%order_id, %slug, %outcome, "GTC order filled between scans: recording position");
                            state.record_position_with_phase(
                                slug.clone(),
                                outcome.clone(),
                                price,
                                shares,
                                phase,
                            );
                            state.mark_order_resolved(&order_id);
                            if let Err(error) = state.save(&settings.state_path).await {
                                warn!(%error, "failed to immediately persist GTC fill position");
                            }
                        }
                    }
                    for signal in &signals {
                        if signal.dry_run {
                            info!(?signal, "DRY RUN: snipe candidate");
                        } else if signal.phase == "phase1"
                            && now_ms() - last_phase1_order_ms < settings.phase1_cooldown_ms
                        {
                            info!(?signal, "phase1 cooldown active: skipping candidate");
                        } else if signal.phase != "phase1"
                            && now_ms() - last_live_order_ms < settings.live_order_cooldown_ms
                        {
                            info!(?signal, "phase2 cooldown active: skipping candidate");
                        } else {
                            if let Some(exit_ms) =
                                state.recent_exit_ms(&signal.market_slug, &signal.outcome)
                            {
                                let age = now_ms() - exit_ms;
                                if age < 10_000 {
                                    // 10 second "Greed Cooldown"
                                    info!(
                                        ?signal,
                                        age_ms = age,
                                        "RE-ENTRY BLOCKED: Greed cooldown active (last exit was {}s ago)",
                                        age / 1000
                                    );
                                    continue;
                                }
                            }
                            if state.open_orders_for_market(&signal.market_slug)
                                >= settings.max_open_orders_per_market
                            {
                                info!(?signal, "open order cap active: skipping candidate");
                                continue;
                            }
                            match buy_request_from_snipe(&settings, signal) {
                                Ok(request) => {
                                    info!(?signal, "SNIPE FOUND: placing live Polymarket order");
                                    let mut d = dashboard_state.write().await;
                                    d.last_snipe = Some(signal.clone());
                                    d.push_activity(
                                        "whale",
                                        &format!(
                                            "Snipe: {} {}",
                                            signal.outcome, signal.market_slug
                                        ),
                                        Some(&signal.reason),
                                    );
                                    drop(d);

                                    match tokio::time::timeout(
                                        Duration::from_secs(15),
                                        post_live_order(&settings, &request),
                                    )
                                    .await
                                    {
                                        Ok(Ok(response)) if response.success => {
                                            let order_id = response
                                                .order_id
                                                .unwrap_or_else(|| format!("clob-{}", now_ms()));
                                            state.record_bot_order_with_id(
                                                order_id.clone(),
                                                signal.market_slug.clone(),
                                                signal.outcome.clone(),
                                                signal.price,
                                                request.size,
                                                Some(signal.phase.clone()),
                                            );
                                            if response_filled_immediately(&response.raw) {
                                                if state.bot_owns_position(
                                                    &signal.market_slug,
                                                    &signal.outcome,
                                                ) {
                                                    info!(
                                                        ?signal,
                                                        "AVERAGING DOWN: Adding to existing position"
                                                    );
                                                    state.record_position_addition(
                                                        &signal.market_slug,
                                                        &signal.outcome,
                                                        signal.price,
                                                        request.size,
                                                        Some(signal.phase.clone()),
                                                    );
                                                } else {
                                                    state.record_position_with_phase(
                                                        signal.market_slug.clone(),
                                                        signal.outcome.clone(),
                                                        signal.price,
                                                        request.size,
                                                        Some(signal.phase.clone()),
                                                    );
                                                }
                                                state.mark_order_resolved(&order_id);
                                            }
                                            if let Err(error) =
                                                state.save(&settings.state_path).await
                                            {
                                                warn!(%error, "failed to immediately persist accepted buy order state");
                                            }
                                            last_live_order_ms = now_ms();
                                            if signal.phase == "phase1" {
                                                last_phase1_order_ms = now_ms();
                                            }
                                            info!(?request, raw = ?response.raw, "placed live Polymarket CLOB V2 buy order");
                                            dashboard_state.write().await.push_activity(
                                                "success",
                                                "Order Accepted",
                                                Some(&request.market_slug),
                                            );
                                            if let Err(error) = hermes_reporter
                                                .report_trade_execution(
                                                    &settings,
                                                    TradeExecutionReport {
                                                        generated_at: Utc::now().to_rfc3339(),
                                                        event_type: "live_buy_order".to_string(),
                                                        market_slug: request.market_slug.clone(),
                                                        outcome: request.outcome.clone(),
                                                        side: "BUY".to_string(),
                                                        phase: Some(signal.phase.clone()),
                                                        amount_usd: Some(request.amount_usd),
                                                        price: Some(request.price),
                                                        shares: Some(request.size),
                                                        success: true,
                                                        reason: "order accepted".to_string(),
                                                        exchange_response: Some(
                                                            response.raw.clone(),
                                                        ),
                                                        error: None,
                                                    },
                                                )
                                                .await
                                            {
                                                warn!(%error, "trade execution LLM report failed");
                                            }
                                        }
                                        Ok(Ok(response)) => {
                                            warn!(?request, raw = ?response.raw, "Polymarket CLOB V2 order was not accepted");
                                            dashboard_state.write().await.push_activity(
                                                "error",
                                                "Order Rejected",
                                                Some(&format!("{:?}", response.raw)),
                                            );
                                            if let Err(error) = hermes_reporter
                                                .report_trade_execution(
                                                    &settings,
                                                    TradeExecutionReport {
                                                        generated_at: Utc::now().to_rfc3339(),
                                                        event_type: "live_buy_order".to_string(),
                                                        market_slug: request.market_slug.clone(),
                                                        outcome: request.outcome.clone(),
                                                        side: "BUY".to_string(),
                                                        phase: Some(signal.phase.clone()),
                                                        amount_usd: Some(request.amount_usd),
                                                        price: Some(request.price),
                                                        shares: Some(request.size),
                                                        success: false,
                                                        reason: "order rejected by exchange"
                                                            .to_string(),
                                                        exchange_response: Some(
                                                            response.raw.clone(),
                                                        ),
                                                        error: None,
                                                    },
                                                )
                                                .await
                                            {
                                                warn!(%error, "trade execution LLM report failed");
                                            }
                                        }
                                        Ok(Err(error)) => {
                                            error!(
                                                ?error,
                                                "ERROR: failed to place live Polymarket CLOB V2 order"
                                            );
                                            dashboard_state.write().await.push_activity(
                                                "error",
                                                "Trade Error",
                                                Some(&error.to_string()),
                                            );
                                            if let Err(report_error) = hermes_reporter
                                                .report_trade_execution(
                                                    &settings,
                                                    TradeExecutionReport {
                                                        generated_at: Utc::now().to_rfc3339(),
                                                        event_type: "live_buy_order".to_string(),
                                                        market_slug: request.market_slug.clone(),
                                                        outcome: request.outcome.clone(),
                                                        side: "BUY".to_string(),
                                                        phase: Some(signal.phase.clone()),
                                                        amount_usd: Some(request.amount_usd),
                                                        price: Some(request.price),
                                                        shares: Some(request.size),
                                                        success: false,
                                                        reason: "order submission failed"
                                                            .to_string(),
                                                        exchange_response: None,
                                                        error: Some(error.to_string()),
                                                    },
                                                )
                                                .await
                                            {
                                                warn!(%report_error, "trade execution LLM report failed");
                                            }
                                        }
                                        Err(_timeout) => {
                                            warn!("live buy order timed out after 15s");
                                            dashboard_state.write().await.push_activity(
                                                "error",
                                                "Order Timeout",
                                                Some(&request.market_slug),
                                            );
                                            if let Err(report_error) = hermes_reporter
                                                .report_trade_execution(
                                                    &settings,
                                                    TradeExecutionReport {
                                                        generated_at: Utc::now().to_rfc3339(),
                                                        event_type: "live_buy_order".to_string(),
                                                        market_slug: request.market_slug.clone(),
                                                        outcome: request.outcome.clone(),
                                                        side: "BUY".to_string(),
                                                        phase: Some(signal.phase.clone()),
                                                        amount_usd: Some(request.amount_usd),
                                                        price: Some(request.price),
                                                        shares: Some(request.size),
                                                        success: false,
                                                        reason: "order timed out".to_string(),
                                                        exchange_response: None,
                                                        error: Some(
                                                            "timeout after 15s".to_string(),
                                                        ),
                                                    },
                                                )
                                                .await
                                            {
                                                warn!(%report_error, "trade execution LLM report failed");
                                            }
                                        }
                                    }
                                }
                                Err(error) => {
                                    warn!(%error, ?signal, "blocked live snipe candidate");
                                    dashboard_state.write().await.push_activity(
                                        "error",
                                        "Blocked Snipe",
                                        Some(&error.to_string()),
                                    );
                                }
                            }
                        }
                    }

                    let mut dashboard = dashboard_state.write().await;
                    dashboard.last_scan_at = Some(Utc::now().to_rfc3339());
                    dashboard.scanned_markets = markets.len();
                    dashboard.candidates = signals;
                    dashboard.watched_markets = markets.clone();
                    dashboard.last_error = None;
                    dashboard.dry_run = settings.dry_run;
                    dashboard.allow_live_buys = settings.allow_live_buys;
                    dashboard.live_max_order_usd = settings.live_max_order_usd;
                    dashboard.live_order_type = settings.live_order_type.clone();
                    dashboard.snipe_max_position_usd = settings.snipe_max_position_usd;
                    dashboard.wallet_configured = settings.polymarket_private_key.is_some();
                    dashboard.funder_address = settings.polymarket_funder_address.clone();
                    dashboard.signature_type = settings.polymarket_signature_type;
                    dashboard.wallet = wallet;
                    dashboard.active_symbols = settings.active_symbols.clone();
                    last_seen_markets = markets
                        .iter()
                        .map(|market| (market.slug.clone(), market.clone()))
                        .collect();
                }
                Ok(Err(error)) => {
                    warn!(%error, "snipe scan failed");
                    let mut dash = dashboard_state.write().await;
                    dash.last_error = Some(error.to_string());
                    dash.last_scan_at = Some(Utc::now().to_rfc3339());
                }
                Err(_) => {
                    warn!("snipe market fetch timed out after 20s");
                    let mut dash = dashboard_state.write().await;
                    dash.last_error = Some("market fetch timed out".to_string());
                    dash.last_scan_at = Some(Utc::now().to_rfc3339());
                }
            }
        }

        // --- POSITION MANAGEMENT & EARLY EXIT ---
        if settings.allow_live_sells {
            let dash_snapshot = dashboard_state.read().await;
            let markets = dash_snapshot.watched_markets.clone();
            let whale_ctx = WhaleContext {
                signals: dash_snapshot.whale_signals.clone(),
                binance_books: dash_snapshot.binance_books.clone(),
            };
            drop(dash_snapshot);
            let mut exits_to_process = Vec::new();

            // 1. Identify which positions to exit
            for position in state.bot_positions.values() {
                let Some(market) = markets.iter().find(|m| m.slug == position.market_slug) else {
                    continue;
                };
                if market.seconds_to_expiry < 15 {
                    continue;
                }

                let outcome = market.outcomes.iter().find(|o| o.name == position.outcome);
                // What are the whales doing right now for this symbol?
                let symbol = position
                    .market_slug
                    .split('-')
                    .next()
                    .unwrap_or("")
                    .to_uppercase();
                let whale_bias = whale_ctx.directional_bias(&symbol);

                // EMERGENCY WHALE EXIT: Exit if whales reverse strongly against us, regardless of profit/loss.
                let whale_exit = if position.outcome == "Up" {
                    whale_bias < -0.3 // Increased threshold to avoid panic exits
                } else {
                    whale_bias > 0.3
                };

                // ADAPTIVE TAKE-PROFIT: tighten/loosen based on whale + Binance book support.
                let share_price = outcome.map(|o| o.price).unwrap_or(0.0);
                let take_profit_exit = if share_price > 0.0 && position.avg_entry_price > 0.0 {
                    let profit_pct =
                        (share_price - position.avg_entry_price) / position.avg_entry_price;
                    let whale_support = if position.outcome == "Up" {
                        whale_bias
                    } else {
                        -whale_bias
                    };
                    let book_support = whale_ctx
                        .binance_book_for_symbol(&symbol)
                        .map(|book| {
                            let raw = book.imbalance_pct / 100.0;
                            if position.outcome == "Up" { raw } else { -raw }
                        })
                        .unwrap_or(0.0);
                    let support_score = whale_support * 0.6 + book_support * 0.4;
                    let adaptive_tp = if support_score > 0.5 {
                        0.10
                    } else if support_score > 0.2 {
                        0.05
                    } else if support_score < -0.2 {
                        0.01
                    } else {
                        settings.phase1_tp_pct
                    };
                    profit_pct >= adaptive_tp
                } else {
                    false
                };

                // TREND LOSS: If Binance price crosses the Target (Price to Beat) against us, cut losses.
                let trend_loss_exit = if position.outcome == "Up" {
                    market.current_price.unwrap_or(f64::MAX)
                        < (market.price_to_beat.unwrap_or(0.0) - 1.0) // 1 USD buffer
                } else {
                    market.current_price.unwrap_or(0.0)
                        > (market.price_to_beat.unwrap_or(f64::MAX) + 1.0)
                };

                if whale_exit || take_profit_exit || trend_loss_exit {
                    let reason = if whale_exit {
                        "WHALE REVERSAL"
                    } else if take_profit_exit {
                        "TAKE PROFIT"
                    } else {
                        "TREND LOSS"
                    };
                    info!(%symbol, %reason, %whale_bias, outcome = %position.outcome, "EXIT TRIGGERED");
                    exits_to_process.push((market.clone(), position.clone(), reason));
                }
            }

            // 2. Execute exits
            for (market, position, reason) in exits_to_process {
                match sell_request_from_position(
                    &settings,
                    &market,
                    &position.outcome,
                    position.total_shares,
                ) {
                    Ok(request) => {
                        match tokio::time::timeout(
                            Duration::from_secs(15),
                            post_live_order(&settings, &request),
                        )
                        .await
                        {
                            Ok(Ok(res)) if res.success => {
                                info!(?request, %reason, "POSITION CLOSED: Early exit executed");
                                dashboard_state.write().await.push_activity(
                                    "warn",
                                    &format!("Exit: {}", reason),
                                    Some(&format!(
                                        "Exited {} {}",
                                        position.outcome, position.market_slug
                                    )),
                                );
                                state.record_exit(&position.market_slug, &position.outcome);
                                if let Err(error) = hermes_reporter
                                    .report_trade_execution(
                                        &settings,
                                        TradeExecutionReport {
                                            generated_at: Utc::now().to_rfc3339(),
                                            event_type: "live_sell_order".to_string(),
                                            market_slug: request.market_slug.clone(),
                                            outcome: request.outcome.clone(),
                                            side: "SELL".to_string(),
                                            phase: position.phase.clone(),
                                            amount_usd: Some(request.amount_usd),
                                            price: Some(request.price),
                                            shares: Some(request.size),
                                            success: true,
                                            reason: format!("early exit executed: {reason}"),
                                            exchange_response: Some(res.raw.clone()),
                                            error: None,
                                        },
                                    )
                                    .await
                                {
                                    warn!(%error, "trade execution LLM report failed");
                                }
                            }
                            Ok(Ok(res)) => {
                                warn!(raw = ?res.raw, "Whale exit failed: order not accepted");
                                if let Err(error) = hermes_reporter
                                    .report_trade_execution(
                                        &settings,
                                        TradeExecutionReport {
                                            generated_at: Utc::now().to_rfc3339(),
                                            event_type: "live_sell_order".to_string(),
                                            market_slug: request.market_slug.clone(),
                                            outcome: request.outcome.clone(),
                                            side: "SELL".to_string(),
                                            phase: position.phase.clone(),
                                            amount_usd: Some(request.amount_usd),
                                            price: Some(request.price),
                                            shares: Some(request.size),
                                            success: false,
                                            reason: format!("early exit rejected: {reason}"),
                                            exchange_response: Some(res.raw.clone()),
                                            error: None,
                                        },
                                    )
                                    .await
                                {
                                    warn!(%error, "trade execution LLM report failed");
                                }
                            }
                            Ok(Err(e)) => {
                                let err_str = e.to_string();
                                warn!(error = %err_str, "Whale exit failed: execution error");
                                if let Err(error) = hermes_reporter
                                    .report_trade_execution(
                                        &settings,
                                        TradeExecutionReport {
                                            generated_at: Utc::now().to_rfc3339(),
                                            event_type: "live_sell_order".to_string(),
                                            market_slug: request.market_slug.clone(),
                                            outcome: request.outcome.clone(),
                                            side: "SELL".to_string(),
                                            phase: position.phase.clone(),
                                            amount_usd: Some(request.amount_usd),
                                            price: Some(request.price),
                                            shares: Some(request.size),
                                            success: false,
                                            reason: format!("early exit failed: {reason}"),
                                            exchange_response: None,
                                            error: Some(err_str.clone()),
                                        },
                                    )
                                    .await
                                {
                                    warn!(%error, "trade execution LLM report failed");
                                }
                                if err_str.contains("balance is not enough") {
                                    info!(
                                        "Clearing phantom position (balance 0, likely an unfilled buy order)."
                                    );
                                    dashboard_state.write().await.push_activity(
                                        "info",
                                        "Phantom Position Cleared",
                                        Some(&position.market_slug),
                                    );
                                    state.record_exit(&position.market_slug, &position.outcome);
                                }
                            }
                            Err(_timeout) => {
                                warn!("live sell order timed out after 15s");
                                dashboard_state.write().await.push_activity(
                                    "error",
                                    "Sell Timeout",
                                    Some(&position.market_slug),
                                );
                            }
                        }
                    }
                    Err(e) => warn!(error = ?e, "Failed to build early exit request"),
                }
            }
        }

        // --- AUTO REDEEM ---
        if settings.auto_redeem && now_ms() - last_redeem_ms > 300_000 {
            // Every 5 mins
            if let Err(e) = redeem_winnings(&settings).await {
                warn!(error = ?e, "Auto-redeem failed or not implemented");
            }
            last_redeem_ms = now_ms();
        }

        let context = StrategyContext::placeholder(&settings, &state);
        let decision = evaluate_strategy(&settings, &state, &context);

        match decision {
            Decision::Observe { reason } => info!(%reason, "observe"),
            Decision::PlaceMakerBuy(plan) => {
                if settings.dry_run || !settings.allow_live_buys {
                    info!(?plan, "DRY RUN / buys disabled: would place maker buy");
                } else {
                    warn!(
                        ?plan,
                        "strategy live buy needs a CLOB token_id; only snipe scanner markets currently carry token ids"
                    );
                    state.record_bot_order(
                        plan.market_slug,
                        plan.outcome,
                        plan.limit_price,
                        plan.shares,
                    );
                }
            }
            Decision::CancelOrder { order_id, reason } => {
                if !settings.allow_cancels {
                    info!(%order_id, %reason, "cancels disabled: would cancel order");
                } else {
                    warn!(%order_id, %reason, "live cancel execution not wired yet");
                    state.mark_order_cancelled(&order_id);
                }
            }
            Decision::SellBotOwnedPosition(plan) => {
                if settings.dry_run || !settings.allow_live_sells {
                    info!(
                        ?plan,
                        "DRY RUN / sells disabled: would sell bot-owned position"
                    );
                } else if !state.bot_owns_position(&plan.market_slug, &plan.outcome) {
                    warn!(?plan, "blocked sell: position is not bot-owned");
                } else {
                    warn!(?plan, "live sell execution not wired yet");
                    state.record_exit(&plan.market_slug, &plan.outcome);
                }
            }
        }

        state.save(&settings.state_path).await?;
        sleep(Duration::from_millis(settings.poll_interval_ms)).await;
    }
}

fn stale_live_orders(
    wallet: &crate::live::WalletSnapshot,
    ttl_ms: i64,
) -> Vec<crate::live::OpenOrderSnapshot> {
    let cutoff = Utc::now() - ChronoDuration::milliseconds(ttl_ms);
    wallet
        .open_orders
        .iter()
        .filter(|order| {
            chrono::DateTime::parse_from_rfc3339(&order.created_at)
                .map(|created_at| created_at.with_timezone(&Utc) <= cutoff)
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

fn response_filled_immediately(raw: &serde_json::Value) -> bool {
    raw.get("trade_ids")
        .and_then(serde_json::Value::as_array)
        .map(|trade_ids| !trade_ids.is_empty())
        .unwrap_or(false)
        || raw
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(|status| status.to_ascii_lowercase().contains("matched"))
            .unwrap_or(false)
}
