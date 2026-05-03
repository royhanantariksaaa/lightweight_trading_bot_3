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
use tracing::{info, warn};

use crate::config::Settings;
use crate::dashboard::{DashboardState, SharedDashboard, serve_dashboard};
use crate::live::{buy_request_from_snipe, post_live_order};
use crate::polymarket::PolymarketClient;
use crate::snipe::find_last_minute_5m_snipes;
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

    let dashboard_state: SharedDashboard = Arc::new(RwLock::new(DashboardState {
        dry_run: settings.dry_run,
        allow_live_buys: settings.allow_live_buys,
        ..DashboardState::default()
    }));

    let dashboard_settings = settings.clone();
    let whale_settings = settings.clone();
    let bot_settings = settings;
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

async fn run_bot(settings: Settings, dashboard_state: SharedDashboard) -> Result<()> {
    let mut state = BotState::load_or_default(&settings.state_path).await?;
    let polymarket = PolymarketClient::new(&settings)?;
    let mut last_live_order_ms = 0_i64;

    loop {
        if settings.enable_last_minute_5m_snipe {
            match polymarket.fetch_active_5m_markets(&settings).await {
                Ok(markets) => {
                    let signals = find_last_minute_5m_snipes(&settings, &markets);
                    for signal in &signals {
                        if signal.dry_run {
                            info!(?signal, "DRY RUN: last-minute 5m snipe candidate");
                        } else if now_ms() - last_live_order_ms < settings.live_order_cooldown_ms {
                            info!(?signal, "live order cooldown active: skipping candidate");
                        } else {
                            match buy_request_from_snipe(&settings, signal) {
                                Ok(request) => match post_live_order(&settings, &request).await {
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
                                        last_live_order_ms = now_ms();
                                        info!(?request, raw = ?response.raw, "placed live Polymarket CLOB V2 buy order");
                                    }
                                    Ok(response) => {
                                        warn!(?request, raw = ?response.raw, "Polymarket CLOB V2 order was not accepted");
                                    }
                                    Err(error) => {
                                        warn!(%error, ?request, "failed to place live Polymarket CLOB V2 order");
                                    }
                                },
                                Err(error) => {
                                    warn!(%error, ?signal, "blocked live snipe candidate");
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
                }
                Err(error) => {
                    warn!(%error, "snipe scan failed");
                    dashboard_state.write().await.last_error = Some(error.to_string());
                }
            }
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
