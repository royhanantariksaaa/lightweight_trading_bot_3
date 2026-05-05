use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::Settings;
use crate::dashboard::{BinanceBookInfo, WhaleSignal};
use crate::polymarket::MarketSnapshot;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnipeSignal {
    pub market_slug: String,
    pub question: String,
    pub outcome: String,
    pub token_id: Option<String>,
    pub price: f64,
    pub expected_edge: f64,
    pub seconds_to_expiry: i64,
    pub volume: f64,
    pub liquidity: f64,
    pub stake_usd: f64,
    pub reason: String,
    pub dry_run: bool,
    #[serde(default = "default_phase")]
    pub phase: String,
}

fn default_phase() -> String {
    "phase2".to_string()
}

#[derive(Clone, Debug, Default)]
pub struct WhaleContext {
    pub signals: Vec<WhaleSignal>,
    pub binance_books: HashMap<String, BinanceBookInfo>,
}

impl WhaleContext {
    pub fn directional_bias(&self, symbol: &str) -> f64 {
        let symbol_upper = symbol.to_ascii_uppercase();
        let relevant: Vec<&WhaleSignal> = self
            .signals
            .iter()
            .filter(|s| s.symbol.to_ascii_uppercase().starts_with(&symbol_upper))
            .collect();

        if relevant.is_empty() {
            return 0.0;
        }

        let mut bullish_weight = 0.0_f64;
        let mut bearish_weight = 0.0_f64;
        for (idx, signal) in relevant.iter().enumerate() {
            let recency = 1.0 / (1.0 + idx as f64 * 0.3);
            let tier_multiplier = match signal.tier.as_str() {
                "SUPER_WHALE" => 3.0,
                "WHALE" => 2.0,
                "MINI_WHALE" => 1.0,
                _ => 0.5,
            };
            let weight = recency * tier_multiplier;
            match signal.side.as_str() {
                "BUY" => bullish_weight += weight,
                "SELL" => bearish_weight += weight,
                _ => {}
            }
        }

        let total = bullish_weight + bearish_weight;
        if total <= 0.0 {
            return 0.0;
        }
        ((bullish_weight - bearish_weight) / total).clamp(-1.0, 1.0)
    }

    pub fn global_activity_score(&self) -> f64 {
        if self.signals.is_empty() {
            return 0.0;
        }
        (self.signals.len() as f64 / 15.0).clamp(0.0, 1.0)
    }

    pub fn binance_book_for_symbol(&self, symbol: &str) -> Option<&BinanceBookInfo> {
        let key = resolve_binance_key(symbol, &self.binance_books)?;
        self.binance_books.get(&key)
    }
}

pub fn find_phase1_whale_ride_signals(
    settings: &Settings,
    markets: &[MarketSnapshot],
    whale_ctx: &WhaleContext,
) -> Vec<SnipeSignal> {
    if !settings.enable_phase1_whale_ride {
        return Vec::new();
    }

    let mut signals = markets
        .iter()
        .filter(|market| market.seconds_to_expiry > settings.snipe_window_seconds)
        .filter(|market| market.seconds_to_expiry <= 300)
        .filter(|market| {
            market.volume >= settings.snipe_min_volume_usd
                || market.liquidity >= settings.snipe_min_liquidity_usd
        })
        .filter_map(|market| pick_phase1_outcome(settings, market, whale_ctx))
        .collect::<Vec<_>>();

    signals.sort_by(|a, b| {
        b.expected_edge
            .partial_cmp(&a.expected_edge)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    signals.truncate(settings.snipe_max_signals);
    signals
}

pub fn find_phase2_snipe_signals(
    settings: &Settings,
    markets: &[MarketSnapshot],
    whale_ctx: &WhaleContext,
) -> Vec<SnipeSignal> {
    if !settings.enable_phase2_snipe {
        return Vec::new();
    }

    let mut signals = markets
        .iter()
        .filter(|market| market.seconds_to_expiry >= 0)
        .filter(|market| market.seconds_to_expiry <= settings.snipe_window_seconds)
        .filter(|market| {
            market.volume >= settings.snipe_min_volume_usd
                || market.liquidity >= settings.snipe_min_liquidity_usd
        })
        .filter_map(|market| pick_phase2_outcome(settings, market, whale_ctx))
        .collect::<Vec<_>>();

    signals.sort_by(|a, b| {
        b.expected_edge
            .partial_cmp(&a.expected_edge)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    signals.truncate(settings.snipe_max_signals);
    signals
}

pub fn find_last_minute_5m_snipes(
    settings: &Settings,
    markets: &[MarketSnapshot],
    whale_ctx: &WhaleContext,
) -> Vec<SnipeSignal> {
    find_phase2_snipe_signals(settings, markets, whale_ctx)
}

fn pick_phase1_outcome(
    settings: &Settings,
    market: &MarketSnapshot,
    whale_ctx: &WhaleContext,
) -> Option<SnipeSignal> {
    let symbol = extract_symbol(&market.slug);
    let book = whale_ctx.binance_book_for_symbol(&symbol)?;
    let imbalance = book.imbalance_pct / 100.0;
    let threshold = settings.phase1_imbalance_threshold.abs();

    let going_up = if imbalance >= threshold {
        true
    } else if imbalance <= -threshold {
        false
    } else {
        return None;
    };

    let whale_bias = whale_ctx.directional_bias(&symbol);
    if going_up && whale_bias < -0.2 {
        return None;
    }
    if !going_up && whale_bias > 0.2 {
        return None;
    }

    let target_outcome_name = if going_up { "Up" } else { "Down" };
    let outcome = market
        .outcomes
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case(target_outcome_name))?;
    let buy_price = outcome.best_ask.or(outcome.best_bid).unwrap_or(outcome.price);
    if buy_price <= 0.0 || buy_price > 0.85 {
        return None;
    }

    let whale_alignment = if going_up { whale_bias.max(0.0) } else { (-whale_bias).max(0.0) };
    let book_strength = (imbalance.abs() / threshold.max(0.01)).clamp(0.0, 2.0) / 2.0;
    let liquidity_quality = ((market.volume + market.liquidity) / settings.snipe_liquidity_scale_usd).clamp(0.0, 1.0);
    let confidence = (0.55 * book_strength + 0.25 * whale_alignment + 0.20 * liquidity_quality).clamp(0.0, 1.0);
    if confidence < 0.45 {
        return None;
    }

    let stake_usd = settings
        .phase1_max_position_usd
        .min(settings.live_max_order_usd)
        .max(1.0);

    Some(SnipeSignal {
        market_slug: market.slug.clone(),
        question: market.question.clone(),
        outcome: outcome.name.clone(),
        token_id: outcome.token_id.clone(),
        price: buy_price,
        expected_edge: confidence,
        seconds_to_expiry: market.seconds_to_expiry,
        volume: market.volume,
        liquidity: market.liquidity,
        stake_usd,
        reason: format!(
            "phase1-whale: {} {} imbalance={:+.1}% whale_bias={:+.2} conf={:.3} tte={}s",
            target_outcome_name,
            symbol,
            book.imbalance_pct,
            whale_bias,
            confidence,
            market.seconds_to_expiry,
        ),
        dry_run: settings.dry_run || !settings.allow_live_buys,
        phase: "phase1".to_string(),
    })
}

fn pick_phase2_outcome(
    settings: &Settings,
    market: &MarketSnapshot,
    whale_ctx: &WhaleContext,
) -> Option<SnipeSignal> {
    let symbol = extract_symbol(&market.slug);
    let (going_up, price_delta_pct) = match (market.price_to_beat, market.current_price) {
        (Some(price_to_beat), Some(current_price)) if price_to_beat > 0.0 && current_price > 0.0 => {
            let delta = (current_price - price_to_beat) / price_to_beat;
            if delta.abs() < settings.snipe_min_price_delta_pct {
                return None;
            }
            (delta > 0.0, delta)
        }
        _ => implied_direction_from_outcomes(market)?,
    };

    let target_outcome_name = if going_up { "Up" } else { "Down" };
    let outcome = market
        .outcomes
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case(target_outcome_name))?;
    let buy_price = outcome.best_ask.or(outcome.best_bid).unwrap_or(outcome.price);
    if buy_price <= 0.0 || buy_price > settings.snipe_max_price {
        return None;
    }

    let whale_bias = whale_ctx.directional_bias(&symbol);
    let momentum = price_delta_pct.abs().clamp(0.0, 0.02) / 0.02;
    let time_pressure = 1.0
        - (market.seconds_to_expiry as f64 / settings.snipe_window_seconds as f64).clamp(0.0, 1.0);
    let liquidity_quality = ((market.volume + market.liquidity) / settings.snipe_liquidity_scale_usd).clamp(0.0, 1.0);
    let whale_alignment = if going_up { whale_bias.max(0.0) } else { (-whale_bias).max(0.0) };
    let winner_clarity = ((buy_price - 0.50).abs() / 0.40).clamp(0.0, 1.0);
    let confidence = (0.25 * momentum
        + 0.20 * time_pressure
        + 0.15 * liquidity_quality
        + 0.15 * whale_alignment
        + 0.25 * winner_clarity)
        .clamp(0.0, 1.0);

    let hail_mary = market.seconds_to_expiry <= settings.phase2_hail_mary_seconds;
    let min_edge = if hail_mary { (settings.snipe_min_edge * 0.5).max(0.005) } else { settings.snipe_min_edge };
    let expected_edge = confidence - (buy_price - 0.5).max(0.0);
    if !hail_mary && confidence < 0.45 {
        return None;
    }
    if expected_edge < min_edge {
        return None;
    }

    let stake_usd = (settings.snipe_max_position_usd * confidence.clamp(0.5, 1.0))
        .min(settings.snipe_max_position_usd)
        .min(settings.live_max_order_usd)
        .max(1.0);

    Some(SnipeSignal {
        market_slug: market.slug.clone(),
        question: market.question.clone(),
        outcome: outcome.name.clone(),
        token_id: outcome.token_id.clone(),
        price: buy_price,
        expected_edge,
        seconds_to_expiry: market.seconds_to_expiry,
        volume: market.volume,
        liquidity: market.liquidity,
        stake_usd,
        reason: format!(
            "phase2-{}: {} {} δ={:+.4}% conf={:.3} whale={:+.2} clarity={:.2} tte={}s",
            if hail_mary { "hail-mary" } else { "confident" },
            target_outcome_name,
            symbol,
            price_delta_pct * 100.0,
            confidence,
            whale_bias,
            winner_clarity,
            market.seconds_to_expiry,
        ),
        dry_run: settings.dry_run || !settings.allow_live_buys,
        phase: "phase2".to_string(),
    })
}

fn implied_direction_from_outcomes(market: &MarketSnapshot) -> Option<(bool, f64)> {
    let up = market.outcomes.iter().find(|o| o.name.eq_ignore_ascii_case("Up"))?;
    let down = market.outcomes.iter().find(|o| o.name.eq_ignore_ascii_case("Down"))?;
    let up_price = up.best_ask.or(up.best_bid).unwrap_or(up.price);
    let down_price = down.best_ask.or(down.best_bid).unwrap_or(down.price);
    if up_price <= 0.0 || down_price <= 0.0 || (up_price - down_price).abs() < 0.01 {
        return None;
    }
    Some((up_price > down_price, (up_price - down_price).abs() / 100.0))
}

fn resolve_binance_key(symbol: &str, books: &HashMap<String, BinanceBookInfo>) -> Option<String> {
    let upper = symbol.to_ascii_uppercase();
    let candidates = [upper.clone(), format!("{}USDT", upper)];
    candidates.into_iter().find(|candidate| books.contains_key(candidate))
}

fn extract_symbol(slug: &str) -> String {
    slug.split('-')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}
