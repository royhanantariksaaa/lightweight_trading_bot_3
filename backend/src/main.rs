mod config;
mod dashboard;
mod hermes_reporter;
mod live;
mod llm;
mod market_recorder;
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
    buy_request_from_market, buy_request_from_snipe, cancel_live_order, fetch_wallet_snapshot,
    hide_stale_display_orders, is_fak_liquidity_miss, live_order_error_summary, post_live_order,
    redeem_winnings, retry_buy_request_at_price, sell_request_from_position,
};
use crate::llm::TradeExecutionReport;
use crate::polymarket::PolymarketClient;
use crate::snipe::{WhaleContext, find_phase1_whale_ride_signals, find_phase2_snipe_signals};
use crate::state::{
    BotState, ResolutionLock, WALLET_ZERO_CLEAR_GRACE_MS, now_ms, wallet_zero_clear_grace_active,
};
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
        enable_balanced_recovery: settings.enable_balanced_recovery,
        enable_same_side_avg_down: settings.enable_same_side_avg_down,
        enable_opposite_side_hedge: settings.enable_opposite_side_hedge,
        enable_resolution_locked_hedge: settings.enable_resolution_locked_hedge,
        max_market_recovery_cost_usd: settings.max_market_recovery_cost_usd,
        recovery_max_adds_per_market: settings.recovery_max_adds_per_market,
        recovery_min_price_improvement_pct: settings.recovery_min_price_improvement_pct,
        phase1_min_hold_ms: settings.phase1_min_hold_ms,
        exit_confirmation_ticks: settings.exit_confirmation_ticks,
        exit_block_if_book_support: settings.exit_block_if_book_support,
        disable_phase1_price_cap: settings.disable_phase1_price_cap,
        enable_recovery_unwind: settings.enable_recovery_unwind,
        recovery_unwind_profit_pct: settings.recovery_unwind_profit_pct,
        recovery_trailing_drawdown_pct: settings.recovery_trailing_drawdown_pct,
        recovery_partial_sell_pct: settings.recovery_partial_sell_pct,
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

    let recorder_settings = runtime_settings.clone();
    let recorder_dashboard = dashboard_state.clone();
    tokio::spawn(async move {
        market_recorder::run_market_recorder(recorder_settings, recorder_dashboard).await;
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
                        let bot_positions = state
                            .bot_positions
                            .values()
                            .filter(|position| position.market_slug == observed.slug)
                            .cloned()
                            .collect::<Vec<_>>();
                        let bot_orders = state
                            .bot_orders
                            .values()
                            .filter(|order| order.market_slug == observed.slug)
                            .cloned()
                            .collect::<Vec<_>>();
                        let report_settings = settings.clone();
                        let report_polymarket = polymarket.clone();
                        let report_hermes = hermes_reporter.clone();
                        let observed_slug = observed.slug.clone();

                        state.mark_closed_market_reported(observed.slug.clone());
                        if let Err(error) = state.save(&settings.state_path).await {
                            warn!(%error, slug = %observed.slug, "failed to persist closed-market report marker");
                        }

                        tokio::spawn(async move {
                            let fetch_result = tokio::time::timeout(
                                Duration::from_secs(20),
                                report_polymarket.fetch_closed_market_snapshot(&observed),
                            )
                            .await;
                            match fetch_result {
                                Ok(Ok(final_market)) => match report_hermes
                                    .report_closed_market_from_parts(
                                        &report_settings,
                                        observed,
                                        final_market,
                                        &bot_positions,
                                        &bot_orders,
                                        &whale_signals,
                                    )
                                    .await
                                {
                                    Ok(true) => {
                                        info!(slug = %observed_slug, "closed market queued for async LLM report")
                                    }
                                    Ok(false) => {}
                                    Err(error) => {
                                        warn!(%error, slug = %observed_slug, "closed market LLM report failed")
                                    }
                                },
                                Ok(Err(error)) => {
                                    warn!(%error, slug = %observed_slug, "failed to fetch closed market snapshot")
                                }
                                Err(_) => {
                                    warn!(slug = %observed_slug, "closed market snapshot fetch timed out")
                                }
                            }
                        });
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
                        } else if state.bot_owns_any_position(&signal.market_slug) {
                            info!(
                                ?signal,
                                "already own position in this market: skipping candidate"
                            );
                        } else if state.recently_exited_any(&signal.market_slug, 30_000) {
                            info!(?signal, "recently exited this market: skipping candidate");
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
                                Ok(mut request) => {
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

                                    let optimistic_order_id = format!("optimistic-{}", now_ms());
                                    state.record_bot_order_with_id(
                                        optimistic_order_id.clone(),
                                        signal.market_slug.clone(),
                                        signal.outcome.clone(),
                                        signal.price,
                                        request.size,
                                        Some(signal.phase.clone()),
                                    );
                                    state.record_optimistic_position_with_phase(
                                        signal.market_slug.clone(),
                                        signal.outcome.clone(),
                                        signal.price,
                                        request.size,
                                        Some(signal.phase.clone()),
                                    );
                                    if let Err(error) = state.save(&settings.state_path).await {
                                        warn!(%error, "failed to persist optimistic buy order state");
                                    }
                                    dashboard_state.write().await.push_activity(
                                        "info",
                                        "Optimistic Buy Tracked",
                                        Some(&format!(
                                            "{} {} while awaiting Polymarket confirmation",
                                            signal.outcome, signal.market_slug
                                        )),
                                    );

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
                                            state.replace_order_id(
                                                &optimistic_order_id,
                                                order_id.clone(),
                                            );
                                            state.confirm_position(
                                                &signal.market_slug,
                                                &signal.outcome,
                                            );
                                            if response_filled_immediately(&response.raw) {
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
                                            state.mark_order_cancelled(&optimistic_order_id);
                                            state.remove_optimistic_position(
                                                &signal.market_slug,
                                                &signal.outcome,
                                            );
                                            if let Err(error) =
                                                state.save(&settings.state_path).await
                                            {
                                                warn!(%error, "failed to persist rejected optimistic buy rollback");
                                            }
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
                                            let summary = live_order_error_summary(&error);
                                            let retry_candidate = signal.phase == "phase1"
                                                && request.order_type.eq_ignore_ascii_case("FAK")
                                                && is_fak_liquidity_miss(&error)
                                                && request.price < 0.85;
                                            if retry_candidate {
                                                let retry_price = (request.price + 0.02).min(0.85);
                                                match retry_buy_request_at_price(
                                                    &settings,
                                                    &request,
                                                    retry_price,
                                                ) {
                                                    Ok(retry_request) => {
                                                        dashboard_state.write().await.push_activity(
                                                            "info",
                                                            "FAK Retry",
                                                            Some(&format!(
                                                                "{} {} liquidity miss at {:.2}; retrying at {:.2} within $1 cap",
                                                                request.outcome,
                                                                request.market_slug,
                                                                request.price,
                                                                retry_request.price
                                                            )),
                                                        );
                                                        match tokio::time::timeout(
                                                            Duration::from_secs(15),
                                                            post_live_order(
                                                                &settings,
                                                                &retry_request,
                                                            ),
                                                        )
                                                        .await
                                                        {
                                                            Ok(Ok(response))
                                                                if response.success =>
                                                            {
                                                                request = retry_request;
                                                                state.update_optimistic_order_and_position(
                                                                    &optimistic_order_id,
                                                                    &signal.market_slug,
                                                                    &signal.outcome,
                                                                    request.price,
                                                                    request.size,
                                                                );
                                                                let order_id = response
                                                                    .order_id
                                                                    .unwrap_or_else(|| {
                                                                        format!("clob-{}", now_ms())
                                                                    });
                                                                state.replace_order_id(
                                                                    &optimistic_order_id,
                                                                    order_id.clone(),
                                                                );
                                                                state.confirm_position(
                                                                    &signal.market_slug,
                                                                    &signal.outcome,
                                                                );
                                                                if response_filled_immediately(
                                                                    &response.raw,
                                                                ) {
                                                                    state.mark_order_resolved(
                                                                        &order_id,
                                                                    );
                                                                }
                                                                if let Err(error) = state
                                                                    .save(&settings.state_path)
                                                                    .await
                                                                {
                                                                    warn!(%error, "failed to persist accepted retry buy order state");
                                                                }
                                                                last_live_order_ms = now_ms();
                                                                if signal.phase == "phase1" {
                                                                    last_phase1_order_ms = now_ms();
                                                                }
                                                                info!(?request, raw = ?response.raw, "placed live Polymarket CLOB V2 buy order after FAK retry");
                                                                dashboard_state.write().await.push_activity(
                                                                    "success",
                                                                    "Order Accepted After Retry",
                                                                    Some(&format!("{} at {:.2}", request.market_slug, request.price)),
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
                                                                            success: true,
                                                                            reason: "order accepted after FAK liquidity retry".to_string(),
                                                                            exchange_response: Some(response.raw.clone()),
                                                                            error: None,
                                                                        },
                                                                    )
                                                                    .await
                                                                {
                                                                    warn!(%report_error, "trade execution LLM report failed");
                                                                }
                                                                continue;
                                                            }
                                                            Ok(Ok(response)) => {
                                                                warn!(?retry_request, raw = ?response.raw, "Polymarket retry order was not accepted");
                                                            }
                                                            Ok(Err(retry_error)) => {
                                                                let retry_summary =
                                                                    live_order_error_summary(
                                                                        &retry_error,
                                                                    );
                                                                warn!(%retry_summary, ?retry_request, "FAK retry failed");
                                                            }
                                                            Err(_) => warn!(
                                                                ?retry_request,
                                                                "FAK retry timed out after 15s"
                                                            ),
                                                        }
                                                    }
                                                    Err(retry_error) => {
                                                        warn!(%retry_error, "failed to build FAK retry request")
                                                    }
                                                }
                                            }
                                            error!(
                                                ?error,
                                                summary = %summary,
                                                "ERROR: failed to place live Polymarket CLOB V2 order"
                                            );
                                            state.mark_order_cancelled(&optimistic_order_id);
                                            state.remove_optimistic_position(
                                                &signal.market_slug,
                                                &signal.outcome,
                                            );
                                            if let Err(save_error) =
                                                state.save(&settings.state_path).await
                                            {
                                                warn!(%save_error, "failed to persist failed optimistic buy rollback");
                                            }
                                            dashboard_state.write().await.push_activity(
                                                "error",
                                                "Trade Error",
                                                Some(&summary),
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
                                                        reason: summary.clone(),
                                                        exchange_response: None,
                                                        error: Some(summary.clone()),
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
                    dashboard.enable_balanced_recovery = settings.enable_balanced_recovery;
                    dashboard.enable_same_side_avg_down = settings.enable_same_side_avg_down;
                    dashboard.enable_opposite_side_hedge = settings.enable_opposite_side_hedge;
                    dashboard.enable_resolution_locked_hedge =
                        settings.enable_resolution_locked_hedge;
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
            let wallet_positions = dash_snapshot.wallet.positions.clone();
            drop(dash_snapshot);

            for wallet_position in &wallet_positions {
                if wallet_position.redeemable
                    || wallet_position.size <= 0.0
                    || wallet_position.avg_price <= 0.0
                    || !markets
                        .iter()
                        .any(|market| market.slug == wallet_position.market_slug)
                {
                    continue;
                }

                if state.bot_owns_position(&wallet_position.market_slug, &wallet_position.outcome) {
                    if state.reconcile_position_from_wallet(
                        &wallet_position.market_slug,
                        &wallet_position.outcome,
                        wallet_position.avg_price,
                        wallet_position.size,
                        Some("wallet-reconciled".to_string()),
                    ) {
                        warn!(
                            market_slug = %wallet_position.market_slug,
                            outcome = %wallet_position.outcome,
                            size = %wallet_position.size,
                            avg_price = %wallet_position.avg_price,
                            "wallet reconciliation: wallet exposure exceeds bot state; updating tracked position"
                        );
                        dashboard_state.write().await.push_activity(
                            "warn",
                            "Wallet Position Reconciled",
                            Some(&format!(
                                "{} {} wallet size {:.2} @ {:.2}",
                                wallet_position.outcome,
                                wallet_position.market_slug,
                                wallet_position.size,
                                wallet_position.avg_price
                            )),
                        );
                        if let Err(error) = state.save(&settings.state_path).await {
                            warn!(%error, "failed to persist reconciled wallet position");
                        }
                    }
                    continue;
                }

                // Never adopt both sides of the same market. If we already own the
                // opposite outcome for this market, skip — it creates contradictory
                // positions and can exceed the $1 cap unintentionally.
                let opposite = if wallet_position.outcome.eq_ignore_ascii_case("Up") {
                    "Down"
                } else {
                    "Up"
                };
                if state.bot_owns_position(&wallet_position.market_slug, opposite) {
                    info!(
                        market_slug = %wallet_position.market_slug,
                        outcome = %wallet_position.outcome,
                        "wallet reconciliation: skipping — already own opposite side"
                    );
                    continue;
                }
                info!(
                    market_slug = %wallet_position.market_slug,
                    outcome = %wallet_position.outcome,
                    size = %wallet_position.size,
                    avg_price = %wallet_position.avg_price,
                    "wallet reconciliation: adopting active wallet position for TP/exit management"
                );
                state.reconcile_position_from_wallet(
                    &wallet_position.market_slug,
                    &wallet_position.outcome,
                    wallet_position.avg_price,
                    wallet_position.size,
                    Some("wallet-reconciled".to_string()),
                );
                dashboard_state.write().await.push_activity(
                    "info",
                    "Wallet Position Adopted",
                    Some(&format!(
                        "{} {} @ {:.2}",
                        wallet_position.outcome,
                        wallet_position.market_slug,
                        wallet_position.avg_price
                    )),
                );
                if let Err(error) = state.save(&settings.state_path).await {
                    warn!(%error, "failed to persist reconciled wallet position");
                }
            }
            let mut recovery_markets = HashSet::new();
            if settings.enable_balanced_recovery && settings.allow_live_buys && !settings.dry_run {
                let positions = state.bot_positions.values().cloned().collect::<Vec<_>>();
                for position in positions {
                    if state.is_resolution_locked(&position.market_slug) {
                        continue;
                    }
                    let Some(market) = markets.iter().find(|m| m.slug == position.market_slug)
                    else {
                        continue;
                    };
                    if market.seconds_to_expiry < 30 {
                        continue;
                    }
                    let market_positions = state
                        .bot_positions
                        .values()
                        .filter(|pos| pos.market_slug == position.market_slug)
                        .cloned()
                        .collect::<Vec<_>>();
                    if market_positions.iter().any(|pos| {
                        pos.confirmation_status.as_deref() == Some("pending_exchange_confirmation")
                    }) {
                        continue;
                    }
                    let total_cost: f64 =
                        market_positions.iter().map(|pos| pos.total_cost_usd).sum();
                    if total_cost >= settings.max_market_recovery_cost_usd {
                        continue;
                    }
                    let add_count = state
                        .bot_orders
                        .values()
                        .filter(|order| order.market_slug == position.market_slug)
                        .count()
                        .saturating_sub(1);
                    if add_count >= settings.recovery_max_adds_per_market {
                        continue;
                    }
                    let recovery_key = format!("{}::recovery", position.market_slug);
                    if let Some(counter) = state.signal_counts.get(&recovery_key) {
                        if now_ms() - counter.last_seen_ms < settings.recovery_cooldown_ms {
                            continue;
                        }
                    }

                    let up_shares: f64 = market_positions
                        .iter()
                        .filter(|pos| pos.outcome.eq_ignore_ascii_case("Up"))
                        .map(|pos| pos.total_shares)
                        .sum();
                    let down_shares: f64 = market_positions
                        .iter()
                        .filter(|pos| pos.outcome.eq_ignore_ascii_case("Down"))
                        .map(|pos| pos.total_shares)
                        .sum();
                    let has_both_sides = up_shares > 0.0 && down_shares > 0.0;
                    if has_both_sides {
                        // Dual-side exposure is only coherent when it was intentionally
                        // converted into a resolution lock. Do not add more recovery here.
                        continue;
                    }
                    let worst_before = up_shares.min(down_shares) - total_cost;
                    if worst_before >= settings.recovery_target_worst_case_pnl {
                        continue;
                    }

                    let symbol = position
                        .market_slug
                        .split('-')
                        .next()
                        .unwrap_or("")
                        .to_uppercase();
                    let whale_bias = whale_ctx.directional_bias(&symbol);
                    let raw_book_support = whale_ctx
                        .binance_book_for_symbol(&symbol)
                        .map(|book| book.imbalance_pct / 100.0)
                        .unwrap_or(0.0);
                    let aligned_book_support = if position.outcome.eq_ignore_ascii_case("Up") {
                        raw_book_support
                    } else {
                        -raw_book_support
                    };
                    let opposite_whale_against = if position.outcome.eq_ignore_ascii_case("Up") {
                        whale_bias < -0.2
                    } else {
                        whale_bias > 0.2
                    };

                    let Some(current_outcome) = market
                        .outcomes
                        .iter()
                        .find(|outcome| outcome.name.eq_ignore_ascii_case(&position.outcome))
                    else {
                        continue;
                    };
                    let current_price = current_outcome
                        .best_ask
                        .or(current_outcome.best_bid)
                        .unwrap_or(current_outcome.price);
                    let position_age_ms = now_ms() - position.opened_at_ms;
                    let mut recovery_outcome = None;
                    let mut recovery_reason = String::new();
                    let mut recovery_phase = "same-side-recovery".to_string();
                    let mut lock_after_buy = false;

                    // Same-side cliff avg-down: only add to the original outcome, only
                    // after the position has aged, only if the price is materially cheaper,
                    // and only if Binance/whale flow has not invalidated the thesis.
                    if settings.enable_same_side_avg_down
                        && current_price > 0.0
                        && current_price
                            <= position.avg_entry_price
                                * (1.0 - settings.recovery_min_price_improvement_pct)
                        && position_age_ms >= settings.phase1_min_hold_ms
                        && (!settings.recovery_require_book_support || aligned_book_support >= 0.12)
                        && !opposite_whale_against
                    {
                        recovery_outcome = Some(position.outcome.clone());
                        recovery_reason = format!(
                            "same-side avg-down: {} {:.2}->{:.2} age={}s book_support={:+.1}% whale_bias={:+.2}",
                            position.outcome,
                            position.avg_entry_price,
                            current_price,
                            position_age_ms / 1000,
                            aligned_book_support * 100.0,
                            whale_bias
                        );
                    } else if settings.enable_opposite_side_hedge
                        && settings.enable_resolution_locked_hedge
                    {
                        // Opposite-side buy is not recovery scalping. If enabled, it must
                        // create a near-neutral payoff table and then lock the whole market
                        // until resolution/redeem. No early exits are allowed after success.
                        let opposite = if position.outcome.eq_ignore_ascii_case("Up") {
                            "Down"
                        } else {
                            "Up"
                        };
                        if let Some(opposite_outcome) = market
                            .outcomes
                            .iter()
                            .find(|outcome| outcome.name.eq_ignore_ascii_case(opposite))
                        {
                            let opposite_price = opposite_outcome
                                .best_ask
                                .or(opposite_outcome.best_bid)
                                .unwrap_or(opposite_outcome.price);
                            if opposite_price > 0.0 {
                                let add_cost = settings
                                    .live_max_order_usd
                                    .min(settings.max_market_recovery_cost_usd - total_cost);
                                if add_cost >= 0.99 {
                                    let add_shares = add_cost / opposite_price;
                                    let (new_up, new_down) = if opposite == "Up" {
                                        (up_shares + add_shares, down_shares)
                                    } else {
                                        (up_shares, down_shares + add_shares)
                                    };
                                    let new_total_cost = total_cost + add_cost;
                                    let pnl_if_up = new_up - new_total_cost;
                                    let pnl_if_down = new_down - new_total_cost;
                                    let worst_after = pnl_if_up.min(pnl_if_down);
                                    let improves = worst_after
                                        >= worst_before
                                            + settings.recovery_min_worst_case_improvement;
                                    let hits_target =
                                        worst_after >= settings.recovery_target_worst_case_pnl;
                                    if improves && hits_target {
                                        recovery_outcome = Some(opposite.to_string());
                                        recovery_phase = "resolution-hedge".to_string();
                                        lock_after_buy = true;
                                        recovery_reason = format!(
                                            "resolution hedge: payoff Up={:+.2} Down={:+.2} worst {:+.2}->{:+.2} price={:.2}",
                                            pnl_if_up,
                                            pnl_if_down,
                                            worst_before,
                                            worst_after,
                                            opposite_price
                                        );
                                    }
                                }
                            }
                        }
                    }

                    let Some(outcome_to_buy) = recovery_outcome else {
                        continue;
                    };
                    let amount_usd = settings
                        .live_max_order_usd
                        .min(settings.max_market_recovery_cost_usd - total_cost);
                    if amount_usd < 0.99 {
                        continue;
                    }
                    match buy_request_from_market(&settings, market, &outcome_to_buy, amount_usd) {
                        Ok(mut request) => {
                            request.order_type = "FAK".to_string();
                            match tokio::time::timeout(
                                Duration::from_secs(15),
                                post_live_order(&settings, &request),
                            )
                            .await
                            {
                                Ok(Ok(response)) if response.success => {
                                    let order_id = response
                                        .order_id
                                        .clone()
                                        .unwrap_or_else(|| format!("recovery-{}", now_ms()));
                                    state.record_bot_order_with_id(
                                        order_id,
                                        request.market_slug.clone(),
                                        request.outcome.clone(),
                                        request.price,
                                        request.size,
                                        Some(recovery_phase.clone()),
                                    );
                                    if state
                                        .bot_owns_position(&request.market_slug, &request.outcome)
                                    {
                                        state.record_position_addition(
                                            &request.market_slug,
                                            &request.outcome,
                                            request.price,
                                            request.size,
                                            Some(recovery_phase.clone()),
                                        );
                                    } else {
                                        state.record_position_with_phase(
                                            request.market_slug.clone(),
                                            request.outcome.clone(),
                                            request.price,
                                            request.size,
                                            Some(recovery_phase.clone()),
                                        );
                                    }
                                    if lock_after_buy {
                                        let locked_up =
                                            if request.outcome.eq_ignore_ascii_case("Up") {
                                                up_shares + request.size
                                            } else {
                                                up_shares
                                            };
                                        let locked_down =
                                            if request.outcome.eq_ignore_ascii_case("Down") {
                                                down_shares + request.size
                                            } else {
                                                down_shares
                                            };
                                        let locked_cost = total_cost + request.amount_usd;
                                        let pnl_if_up = locked_up - locked_cost;
                                        let pnl_if_down = locked_down - locked_cost;
                                        let worst_case_pnl = pnl_if_up.min(pnl_if_down);
                                        state.lock_resolution_market(ResolutionLock {
                                            market_slug: request.market_slug.clone(),
                                            locked_at_ms: now_ms(),
                                            reason: recovery_reason.clone(),
                                            up_shares: locked_up,
                                            down_shares: locked_down,
                                            total_cost_usd: locked_cost,
                                            pnl_if_up,
                                            pnl_if_down,
                                            worst_case_pnl,
                                        });
                                    }
                                    recovery_markets.insert(request.market_slug.clone());
                                    let recovery_counter = state
                                        .signal_counts
                                        .entry(recovery_key.clone())
                                        .or_default();
                                    recovery_counter.entry_ticks += 1;
                                    recovery_counter.last_seen_ms = now_ms();
                                    dashboard_state.write().await.push_activity(
                                        "warn",
                                        if lock_after_buy {
                                            "Resolution Lock Entered"
                                        } else {
                                            "Same-Side Recovery Buy Executed"
                                        },
                                        Some(&format!(
                                            "{} {} ${:.2} @ {:.2} — {}",
                                            request.outcome,
                                            request.market_slug,
                                            request.amount_usd,
                                            request.price,
                                            recovery_reason
                                        )),
                                    );
                                    if let Err(error) = hermes_reporter
                                        .report_trade_execution(
                                            &settings,
                                            TradeExecutionReport {
                                                generated_at: Utc::now().to_rfc3339(),
                                                event_type: if lock_after_buy {
                                                    "live_resolution_hedge_buy_order".to_string()
                                                } else {
                                                    "live_recovery_buy_order".to_string()
                                                },
                                                market_slug: request.market_slug.clone(),
                                                outcome: request.outcome.clone(),
                                                side: "BUY".to_string(),
                                                phase: Some(recovery_phase.clone()),
                                                amount_usd: Some(request.amount_usd),
                                                price: Some(request.price),
                                                shares: Some(request.size),
                                                success: true,
                                                reason: recovery_reason.clone(),
                                                exchange_response: Some(response.raw.clone()),
                                                error: None,
                                            },
                                        )
                                        .await
                                    {
                                        warn!(%error, "recovery buy LLM report failed");
                                    }
                                }
                                Ok(Ok(response)) => {
                                    warn!(raw = ?response.raw, %recovery_reason, "recovery buy rejected");
                                }
                                Ok(Err(error)) => {
                                    warn!(%error, %recovery_reason, "recovery buy failed");
                                }
                                Err(_) => {
                                    warn!(%recovery_reason, "recovery buy timed out");
                                }
                            }
                        }
                        Err(error) => {
                            warn!(%error, %recovery_reason, "failed to build recovery buy")
                        }
                    }
                }
                if let Err(error) = state.save(&settings.state_path).await {
                    warn!(%error, "failed to persist recovery state");
                }
            }

            let mut exits_to_process = Vec::new();

            // 1. Identify which positions to exit
            for position in state.bot_positions.values().cloned() {
                let Some(market) = markets.iter().find(|m| m.slug == position.market_slug) else {
                    continue;
                };
                if state.is_resolution_locked(&position.market_slug) {
                    info!(
                        market_slug = %position.market_slug,
                        outcome = %position.outcome,
                        "RESOLUTION LOCKED: skipping early exit"
                    );
                    continue;
                }
                if recovery_markets.contains(&position.market_slug) {
                    continue;
                }
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
                let book_support = whale_ctx
                    .binance_book_for_symbol(&symbol)
                    .map(|book| {
                        let raw = book.imbalance_pct / 100.0;
                        if position.outcome == "Up" { raw } else { -raw }
                    })
                    .unwrap_or(0.0);
                let whale_support = if position.outcome == "Up" {
                    whale_bias
                } else {
                    -whale_bias
                };
                let support_score = whale_support * 0.6 + book_support * 0.4;
                let position_age_ms = now_ms() - position.opened_at_ms;

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

                // RECOVERY UNWIND: once an avg-down/hedge leg bounces, do not wait for
                // the generic exit stack. Take controlled profit or trail from the peak.
                let mut recovery_unwind_exit = false;
                let mut recovery_sell_shares = position.total_shares;
                if settings.enable_recovery_unwind
                    && share_price > 0.0
                    && position.avg_entry_price > 0.0
                {
                    let market_order_count = state
                        .bot_orders
                        .values()
                        .filter(|order| order.market_slug == position.market_slug)
                        .count();
                    let is_recovery_position =
                        position.phase.as_deref() == Some("recovery") || market_order_count > 1;
                    if is_recovery_position {
                        let profit_pct =
                            (share_price - position.avg_entry_price) / position.avg_entry_price;
                        let unwind_key = format!(
                            "{}::{}::recovery-unwind",
                            position.market_slug, position.outcome
                        );
                        let counter = state.signal_counts.entry(unwind_key).or_default();
                        if share_price > counter.peak_price {
                            counter.peak_price = share_price;
                        }
                        counter.last_seen_ms = now_ms();
                        let drawdown_from_peak = if counter.peak_price > 0.0 {
                            (counter.peak_price - share_price) / counter.peak_price
                        } else {
                            0.0
                        };
                        let profit_ready = profit_pct >= settings.recovery_unwind_profit_pct;
                        let trail_hit = counter.peak_price
                            >= position.avg_entry_price
                                * (1.0 + settings.recovery_unwind_profit_pct)
                            && drawdown_from_peak >= settings.recovery_trailing_drawdown_pct;
                        let support_fading = support_score <= 0.20;
                        recovery_unwind_exit = profit_ready && (support_fading || trail_hit);
                        if recovery_unwind_exit {
                            let requested = position.total_shares
                                * settings.recovery_partial_sell_pct.clamp(0.10, 1.0);
                            recovery_sell_shares =
                                if requested >= 5.0 && position.total_shares - requested >= 1.0 {
                                    requested
                                } else {
                                    position.total_shares
                                };
                        }
                    }
                }

                // TREND LOSS: If Binance price crosses the Target (Price to Beat) against us, cut losses.
                let trend_loss_exit = if position.outcome == "Up" {
                    market.current_price.unwrap_or(f64::MAX)
                        < (market.price_to_beat.unwrap_or(0.0) - 1.0) // 1 USD buffer
                } else {
                    market.current_price.unwrap_or(0.0)
                        > (market.price_to_beat.unwrap_or(f64::MAX) + 1.0)
                };

                let raw_exit_triggered =
                    recovery_unwind_exit || whale_exit || take_profit_exit || trend_loss_exit;
                if raw_exit_triggered {
                    let reason = if recovery_unwind_exit {
                        "RECOVERY UNWIND"
                    } else if whale_exit {
                        "WHALE REVERSAL"
                    } else if take_profit_exit {
                        "TAKE PROFIT"
                    } else {
                        "TREND LOSS"
                    };

                    let exit_key = format!("{}::{}::exit", position.market_slug, position.outcome);
                    if !take_profit_exit
                        && !recovery_unwind_exit
                        && position_age_ms < settings.phase1_min_hold_ms
                    {
                        let counter = state.signal_counts.entry(exit_key).or_default();
                        counter.exit_ticks = 0;
                        counter.last_seen_ms = now_ms();
                        info!(
                            %symbol,
                            %reason,
                            age_ms = position_age_ms,
                            min_hold_ms = settings.phase1_min_hold_ms,
                            outcome = %position.outcome,
                            "EXIT DELAYED: minimum hold window active"
                        );
                        continue;
                    }
                    if !take_profit_exit
                        && !recovery_unwind_exit
                        && settings.exit_block_if_book_support
                        && book_support >= settings.exit_book_support_threshold
                    {
                        let counter = state.signal_counts.entry(exit_key).or_default();
                        counter.exit_ticks = 0;
                        counter.last_seen_ms = now_ms();
                        info!(
                            %symbol,
                            %reason,
                            book_support = book_support,
                            outcome = %position.outcome,
                            "EXIT BLOCKED: Binance book still supports position"
                        );
                        continue;
                    }
                    if !take_profit_exit
                        && !recovery_unwind_exit
                        && settings.exit_confirmation_ticks > 1
                    {
                        let counter = state.signal_counts.entry(exit_key.clone()).or_default();
                        let now = now_ms();
                        if now - counter.last_seen_ms > 30_000 {
                            counter.exit_ticks = 0;
                        }
                        counter.exit_ticks += 1;
                        counter.last_seen_ms = now;
                        if counter.exit_ticks < settings.exit_confirmation_ticks {
                            info!(
                                %symbol,
                                %reason,
                                ticks = counter.exit_ticks,
                                required = settings.exit_confirmation_ticks,
                                outcome = %position.outcome,
                                "EXIT DELAYED: waiting for reversal confirmation"
                            );
                            continue;
                        }
                    }

                    info!(%symbol, %reason, %whale_bias, book_support = book_support, outcome = %position.outcome, entry = %position.avg_entry_price, current = %share_price, "EXIT TRIGGERED");
                    if take_profit_exit {
                        let profit_pct = if position.avg_entry_price > 0.0 {
                            (share_price - position.avg_entry_price) / position.avg_entry_price
                        } else {
                            0.0
                        };
                        dashboard_state.write().await.push_activity(
                            "success",
                            "Adaptive TP Triggered",
                            Some(&format!(
                                "{} {} profit={:+.1}% price {:.2}->{:.2}",
                                position.outcome,
                                position.market_slug,
                                profit_pct * 100.0,
                                position.avg_entry_price,
                                share_price
                            )),
                        );
                    }
                    exits_to_process.push((
                        market.clone(),
                        position.clone(),
                        reason,
                        recovery_sell_shares,
                    ));
                }
            }

            // 2. Execute exits
            for (market, position, reason, sell_shares) in exits_to_process {
                let exit_key = format!("{}::{}", position.market_slug, position.outcome);
                let now = now_ms();
                if let Some(last_attempt) = state.last_exit_attempt_ms.get(&exit_key) {
                    if now - last_attempt < 20_000 {
                        info!(
                            market_slug = %position.market_slug,
                            outcome = %position.outcome,
                            ms_since_last = now - last_attempt,
                            "exit retry cooldown active: skipping exit attempt"
                        );
                        continue;
                    }
                }
                let actual_wallet_shares = wallet_positions
                    .iter()
                    .filter(|wallet_position| {
                        !wallet_position.redeemable
                            && wallet_position.market_slug == position.market_slug
                            && wallet_position
                                .outcome
                                .eq_ignore_ascii_case(&position.outcome)
                    })
                    .map(|wallet_position| wallet_position.size)
                    .fold(0.0, f64::max);
                if actual_wallet_shares <= 0.0 {
                    if wallet_zero_clear_grace_active(&position, now, WALLET_ZERO_CLEAR_GRACE_MS) {
                        info!(
                            market_slug = %position.market_slug,
                            outcome = %position.outcome,
                            age_since_buy_ms = now - position.last_buy_at_ms.max(position.opened_at_ms),
                            grace_ms = WALLET_ZERO_CLEAR_GRACE_MS,
                            "exit reconciliation: wallet has zero shares inside post-buy grace; keeping bot position"
                        );
                        continue;
                    }
                    info!(
                        market_slug = %position.market_slug,
                        outcome = %position.outcome,
                        "exit reconciliation: wallet has zero shares; clearing stale bot position"
                    );
                    dashboard_state.write().await.push_activity(
                        "info",
                        "Stale Position Cleared",
                        Some(&format!(
                            "{} {} wallet shares=0 before sell",
                            position.outcome, position.market_slug
                        )),
                    );
                    state.record_exit(&position.market_slug, &position.outcome);
                    state.last_exit_attempt_ms.remove(&exit_key);
                    continue;
                }
                let sell_shares = sell_shares.min(actual_wallet_shares);
                state.last_exit_attempt_ms.insert(exit_key.clone(), now);
                match sell_request_from_position(&settings, &market, &position.outcome, sell_shares)
                {
                    Ok(request) => {
                        match tokio::time::timeout(
                            Duration::from_secs(15),
                            post_live_order(&settings, &request),
                        )
                        .await
                        {
                            Ok(Ok(res)) if res.success => {
                                info!(?request, %reason, "POSITION CLOSED: Early exit executed");
                                state.last_exit_attempt_ms.remove(&exit_key);
                                dashboard_state.write().await.push_activity(
                                    "warn",
                                    &format!("Exit: {}", reason),
                                    Some(&format!(
                                        "Exited {} {}",
                                        position.outcome, position.market_slug
                                    )),
                                );
                                if request.size >= position.total_shares * 0.995 {
                                    state.record_exit(&position.market_slug, &position.outcome);
                                } else {
                                    state.reduce_position(
                                        &position.market_slug,
                                        &position.outcome,
                                        request.size,
                                    );
                                }
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
                                    if wallet_zero_clear_grace_active(
                                        &position,
                                        now_ms(),
                                        WALLET_ZERO_CLEAR_GRACE_MS,
                                    ) {
                                        info!(
                                            market_slug = %position.market_slug,
                                            outcome = %position.outcome,
                                            "balance-not-enough sell error inside post-buy grace; keeping bot position"
                                        );
                                    } else {
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
