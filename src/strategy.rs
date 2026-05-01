use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::state::{BotState, now_ms};

#[derive(Clone, Debug)]
pub struct StrategyContext {
    pub market_slug: String,
    pub outcome: String,
    pub fair_price: f64,
    pub best_bid: f64,
    pub best_ask: f64,
    pub quote_age_ms: i64,
    pub ofi_score: f64,
    pub regime_persistence: f64,
    pub depth_support: f64,
    pub spread_quality: f64,
    pub volatility: f64,
    pub seconds_to_expiry: f64,
    pub inventory_shares: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MakerBuyPlan {
    pub market_slug: String,
    pub outcome: String,
    pub limit_price: f64,
    pub shares: f64,
    pub score: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SellPlan {
    pub market_slug: String,
    pub outcome: String,
    pub limit_price: f64,
    pub shares: f64,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub enum Decision {
    Observe { reason: String },
    PlaceMakerBuy(MakerBuyPlan),
    CancelOrder { order_id: String, reason: String },
    SellBotOwnedPosition(SellPlan),
}

impl StrategyContext {
    pub fn placeholder(settings: &Settings, state: &BotState) -> Self {
        let symbol = settings.symbols.first().cloned().unwrap_or_else(|| "BTC".to_string());
        let market_slug = format!("{}-placeholder-market", symbol.to_lowercase());
        let outcome = "DOWN".to_string();
        let inventory_shares = state
            .bot_positions
            .get(&format!("{}::{}", market_slug, outcome))
            .map(|p| p.shares)
            .unwrap_or(0.0);

        Self {
            market_slug,
            outcome,
            fair_price: 0.50,
            best_bid: 0.49,
            best_ask: 0.51,
            quote_age_ms: 0,
            ofi_score: 0.0,
            regime_persistence: 0.0,
            depth_support: 0.0,
            spread_quality: 0.0,
            volatility: 0.02,
            seconds_to_expiry: 600.0,
            inventory_shares,
        }
    }
}

pub fn evaluate_strategy(settings: &Settings, state: &BotState, ctx: &StrategyContext) -> Decision {
    if ctx.quote_age_ms > settings.max_quote_age_ms {
        return Decision::Observe { reason: format!("stale quote age {}ms", ctx.quote_age_ms) };
    }

    if let Some(exit_at) = state.recent_exit_ms(&ctx.market_slug, &ctx.outcome) {
        let elapsed = now_ms() - exit_at;
        if elapsed < settings.reentry_cooldown_ms {
            return Decision::Observe {
                reason: format!("same-side reentry cooldown active: {}ms remaining", settings.reentry_cooldown_ms - elapsed),
            };
        }
    }

    if state.open_orders_for_market(&ctx.market_slug) >= settings.max_open_orders_per_market {
        return Decision::Observe { reason: "open order cap reached for market".to_string() };
    }

    let score = entry_score(ctx);
    let entry_valid = score >= 0.75;
    let exit_valid = score <= 0.45;

    let mut shadow = state.clone();
    let counts = shadow.update_signal_counts(
        &format!("{}::{}", ctx.market_slug, ctx.outcome),
        entry_valid,
        exit_valid,
    );

    if state.bot_owns_position(&ctx.market_slug, &ctx.outcome) {
        if !settings.auto_take_profit && !settings.auto_exit_no_edge {
            return Decision::Observe { reason: "bot-owned position exists; auto exits disabled".to_string() };
        }
        if counts.exit_ticks < settings.exit_confirmation_ticks {
            return Decision::Observe { reason: format!("exit signal waiting for confirmation {}/{}", counts.exit_ticks, settings.exit_confirmation_ticks) };
        }
        return Decision::SellBotOwnedPosition(SellPlan {
            market_slug: ctx.market_slug.clone(),
            outcome: ctx.outcome.clone(),
            limit_price: ctx.best_bid,
            shares: ctx.inventory_shares.max(0.1),
            reason: "confirmed weak score on bot-owned position".to_string(),
        });
    }

    if counts.entry_ticks < settings.entry_confirmation_ticks {
        return Decision::Observe { reason: format!("entry signal waiting for confirmation {}/{} score={:.3}", counts.entry_ticks, settings.entry_confirmation_ticks, score) };
    }

    if score < 0.75 {
        return Decision::Observe { reason: format!("score below entry threshold: {:.3}", score) };
    }

    let limit_price = reservation_bid(settings, ctx);
    if limit_price <= 0.0 || limit_price >= ctx.best_ask {
        return Decision::Observe { reason: format!("reservation bid {:.4} would cross/invalid", limit_price) };
    }

    let shares = (settings.max_position_usd / limit_price).max(0.1);
    Decision::PlaceMakerBuy(MakerBuyPlan {
        market_slug: ctx.market_slug.clone(),
        outcome: ctx.outcome.clone(),
        limit_price,
        shares,
        score,
        reason: "confirmed OFI/inventory-aware maker quote".to_string(),
    })
}

fn entry_score(ctx: &StrategyContext) -> f64 {
    let spread = (ctx.best_ask - ctx.best_bid).max(0.0);
    let inventory_penalty = (ctx.inventory_shares.abs() / 25.0).clamp(0.0, 1.0);
    let adverse_selection_penalty = if ctx.ofi_score < -0.2 { 0.5 } else { 0.0 };
    let latency_freshness = (1.0 - (ctx.quote_age_ms as f64 / 1500.0)).clamp(0.0, 1.0);
    let spread_quality = if spread <= 0.03 { 1.0 } else { (0.06 - spread).max(0.0) / 0.03 };
    let time_quality = (ctx.seconds_to_expiry / 600.0).clamp(0.0, 1.0);

    0.35 * normalize_unit(ctx.ofi_score)
        + 0.20 * ctx.regime_persistence.clamp(0.0, 1.0)
        + 0.15 * ctx.depth_support.clamp(0.0, 1.0)
        + 0.10 * spread_quality.max(ctx.spread_quality.clamp(0.0, 1.0))
        + 0.10 * latency_freshness
        + 0.10 * time_quality
        - 0.35 * inventory_penalty
        - 0.35 * adverse_selection_penalty
}

fn reservation_bid(settings: &Settings, ctx: &StrategyContext) -> f64 {
    let gamma = 0.10;
    let directional_skew = 0.02 * ctx.ofi_score.clamp(-1.0, 1.0);
    let inventory_penalty = ctx.inventory_shares * gamma * ctx.volatility.powi(2) * ctx.seconds_to_expiry;
    let reservation = ctx.fair_price + directional_skew - inventory_penalty;
    let half_spread = ((ctx.best_ask - ctx.best_bid) / 2.0).clamp(0.005, 0.05);
    let safety_buffer = settings.min_edge;
    (reservation - half_spread - safety_buffer).clamp(0.01, 0.99)
}

fn normalize_unit(value: f64) -> f64 {
    ((value + 1.0) / 2.0).clamp(0.0, 1.0)
}
