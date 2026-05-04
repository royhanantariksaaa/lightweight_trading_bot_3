mod config;
mod dashboard;
mod live;
mod polymarket;
mod snipe;
mod state;
mod strategy;
mod whale;

use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::config::Settings;
use crate::dashboard::{DashboardState, SharedDashboard, serve_dashboard};
use crate::live::{buy_request_from_snipe, fetch_wallet_snapshot, post_live_order, redeem_winnings, sell_request_from_position};
use crate::polymarket::PolymarketClient;
use crate::snipe::{WhaleContext, find_last_minute_5m_snipes};
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
        live_max_order_usd: settings.live_max_order_usd,
        snipe_max_position_usd: settings.snipe_max_position_usd,
        wallet_configured: settings.polymarket_private_key.is_some(),
        funder_address: settings.polymarket_funder_address.clone(),
        signature_type: settings.polymarket_signature_type,
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
    let mut last_live_order_ms = 0_i64;
    let mut last_redeem_ms = 0_i64;

    loop {
        let settings = runtime_settings.read().await.clone();
        if settings.enable_last_minute_5m_snipe {
            match polymarket.fetch_active_5m_markets(&settings).await {
                Ok(markets) => {
                    // Read whale signals from dashboard to inform directional bias
                    let whale_signals = dashboard_state.read().await.whale_signals.clone();
                    let whale_ctx = WhaleContext { signals: whale_signals };
                    let signals = find_last_minute_5m_snipes(&settings, &markets, &whale_ctx);
                    let wallet = fetch_wallet_snapshot(&settings).await;
                    for signal in &signals {
                        if signal.dry_run {
                            info!(?signal, "DRY RUN: last-minute 5m snipe candidate");
                        } else if now_ms() - last_live_order_ms < settings.live_order_cooldown_ms {
                            info!(?signal, "live order cooldown active: skipping candidate");
                        } else {
                            match buy_request_from_snipe(&settings, signal) {
                                Ok(request) => {
                                    info!(?signal, "SNIPE FOUND: placing live Polymarket order");
                                    let mut d = dashboard_state.write().await;
                                    d.last_snipe = Some(signal.clone());
                                    d.push_activity("whale", &format!("Snipe: {} {}", signal.outcome, signal.market_slug), Some(&signal.reason));
                                    drop(d);

                                    match post_live_order(&settings, &request).await {
                                        Ok(response) if response.success => {
                                            let order_id = response
                                                .order_id
                                                .unwrap_or_else(|| format!("clob-{}", now_ms()));
                                            state.record_bot_order_with_id(
                                                order_id,
                                                signal.market_slug.clone(),
                                                signal.outcome.clone(),
                                                signal.price,
                                                request.size,
                                            );
                                            state.record_position(
                                                signal.market_slug.clone(),
                                                signal.outcome.clone(),
                                                signal.price,
                                                request.size,
                                            );
                                            last_live_order_ms = now_ms();
                                            info!(?request, raw = ?response.raw, "placed live Polymarket CLOB V2 buy order");
                                            dashboard_state.write().await.push_activity("success", "Order Accepted", Some(&request.market_slug));
                                        }
                                        Ok(response) => {
                                            warn!(?request, raw = ?response.raw, "Polymarket CLOB V2 order was not accepted");
                                            dashboard_state.write().await.push_activity("error", "Order Rejected", Some(&format!("{:?}", response.raw)));
                                        }
                                        Err(error) => {
                                            error!(?error, "ERROR: failed to place live Polymarket CLOB V2 order");
                                            dashboard_state.write().await.push_activity("error", "Trade Error", Some(&error.to_string()));
                                        }
                                    }
                                }
                                Err(error) => {
                                    warn!(%error, ?signal, "blocked live snipe candidate");
                                    dashboard_state.write().await.push_activity("error", "Blocked Snipe", Some(&error.to_string()));
                                }
                            }
                        }
                    }

                    let mut dashboard = dashboard_state.write().await;
                    dashboard.last_scan_at = Some(Utc::now().to_rfc3339());
                    dashboard.scanned_markets = markets.len();
                    dashboard.candidates = signals;
                    dashboard.watched_markets = markets;
                    dashboard.last_error = None;
                    dashboard.dry_run = settings.dry_run;
                    dashboard.allow_live_buys = settings.allow_live_buys;
                    dashboard.live_max_order_usd = settings.live_max_order_usd;
                    dashboard.snipe_max_position_usd = settings.snipe_max_position_usd;
                    dashboard.wallet_configured = settings.polymarket_private_key.is_some();
                    dashboard.funder_address = settings.polymarket_funder_address.clone();
                    dashboard.signature_type = settings.polymarket_signature_type;
                    dashboard.wallet = wallet;
                }
                Err(error) => {
                    warn!(%error, "snipe scan failed");
                    dashboard_state.write().await.last_error = Some(error.to_string());
                }
            }
        }

        // --- POSITION MANAGEMENT & EARLY EXIT ---
        if settings.allow_live_sells {
            let markets = dashboard_state.read().await.watched_markets.clone();
            let whale_signals = dashboard_state.read().await.whale_signals.clone();
            let whale_ctx = WhaleContext { signals: whale_signals };
            let mut exits_to_process = Vec::new();

            // 1. Identify which positions to exit
            for position in state.bot_positions.values() {
                let Some(market) = markets.iter().find(|m| m.slug == position.market_slug) else { continue; };
                if market.seconds_to_expiry < 15 { continue; }

                let outcome = market.outcomes.iter().find(|o| o.name == position.outcome);
                // What are the whales doing right now for this symbol?
                let symbol = position.market_slug.split('-').next().unwrap_or("").to_uppercase();
                let whale_bias = whale_ctx.directional_bias(&symbol);
                
                // EMERGENCY WHALE EXIT: Exit if whales reverse strongly against us, regardless of profit/loss.
                let exit_triggered = if position.outcome == "Up" {
                    whale_bias < -0.2 // Whales are selling our "Up" position
                } else {
                    whale_bias > 0.2  // Whales are buying against our "Down" position
                };

                if exit_triggered {
                    info!(%symbol, %whale_bias, outcome = %position.outcome, "EMERGENCY WHALE EXIT TRIGGERED");
                    exits_to_process.push((market.clone(), position.clone()));
                }
            }

            // 2. Execute exits
            for (market, position) in exits_to_process {
                match sell_request_from_position(&settings, &market, &position.outcome, position.shares) {
                    Ok(request) => {
                        match post_live_order(&settings, &request).await {
                            Ok(res) if res.success => {
                                info!(?request, "WHALE EXIT: Position closed early for profit");
                                dashboard_state.write().await.push_activity("warn", "Whale Exit Triggered", Some(&format!("Exited {} {}", position.outcome, position.market_slug)));
                                state.record_exit(&position.market_slug, &position.outcome);
                            }
                            Ok(res) => warn!(raw = ?res.raw, "Whale exit failed: order not accepted"),
                            Err(e) => warn!(error = ?e, "Whale exit failed: execution error"),
                        }
                    }
                    Err(e) => warn!(error = ?e, "Failed to build early exit request"),
                }
            }
        }

        // --- AUTO REDEEM ---
        if settings.auto_redeem && now_ms() - last_redeem_ms > 300_000 { // Every 5 mins
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
