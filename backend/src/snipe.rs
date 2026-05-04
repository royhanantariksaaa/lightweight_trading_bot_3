use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::dashboard::WhaleSignal;
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
}

/// Context from the whale detector that informs directional bias for each symbol.
#[derive(Clone, Debug)]
pub struct WhaleContext {
    /// Most recent whale signals from the dashboard, keyed by their base symbol (BTC, ETH, etc.)
    pub signals: Vec<WhaleSignal>,
}

impl WhaleContext {

    /// Returns the net directional bias from recent whale activity for a given symbol.
    /// Positive = bullish (whales buying), Negative = bearish (whales selling).
    /// Value is in the range [-1.0, 1.0].
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

        // Weight by notional size and recency (most recent signals first in the vec)
        let mut bullish_weight = 0.0_f64;
        let mut bearish_weight = 0.0_f64;
        for (idx, signal) in relevant.iter().enumerate() {
            // Decay factor: most recent signal has full weight, older ones decay
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
        // Returns [-1, 1]: +1 = all bullish whales, -1 = all bearish whales
        ((bullish_weight - bearish_weight) / total).clamp(-1.0, 1.0)
    }
}

pub fn find_last_minute_5m_snipes(
    settings: &Settings,
    markets: &[MarketSnapshot],
    whale_ctx: &WhaleContext,
) -> Vec<SnipeSignal> {
    let mut signals = markets
        .iter()
        .filter(|market| market.seconds_to_expiry >= 0)
        .filter(|market| market.seconds_to_expiry <= settings.snipe_window_seconds)
        .filter(|market| {
            market.volume >= settings.snipe_min_volume_usd
                || market.liquidity >= settings.snipe_min_liquidity_usd
        })
        .filter_map(|market| pick_directional_outcome(settings, market, whale_ctx))
        .collect::<Vec<_>>();

    signals.sort_by(|a, b| {
        b.expected_edge
            .partial_cmp(&a.expected_edge)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    signals.truncate(settings.snipe_max_signals);
    signals
}

/// Instead of scoring every outcome independently, this function:
/// 1. Determines the likely winning direction using current_price vs price_to_beat
/// 2. Picks the matching outcome ("Up" if price > reference, "Down" otherwise)
/// 3. Adds whale signal awareness as a confidence booster/dampener
/// 4. Buys at the best ask of the LIKELY WINNING side
fn pick_directional_outcome(
    settings: &Settings,
    market: &MarketSnapshot,
    whale_ctx: &WhaleContext,
) -> Option<SnipeSignal> {
    let price_to_beat = market.price_to_beat?;
    let current_price = market.current_price?;

    if price_to_beat <= 0.0 || current_price <= 0.0 {
        return None;
    }

    // Step 1: Determine direction from reference price data
    let price_delta_pct = (current_price - price_to_beat) / price_to_beat;
    let going_up = price_delta_pct > 0.0;

    // Step 2: Find the matching outcome
    let target_outcome_name = if going_up { "Up" } else { "Down" };
    let outcome = market
        .outcomes
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case(target_outcome_name))?;

    // Step 3: Get the buy price (best ask, or fallback to current price)
    let buy_price = outcome.best_ask.or(outcome.best_bid).unwrap_or(outcome.price);
    if buy_price <= 0.0 || buy_price > settings.snipe_max_price {
        return None;
    }

    // Step 4: Calculate directional confidence score
    let symbol = extract_symbol(&market.slug);
    let whale_bias = whale_ctx.directional_bias(&symbol);

    // Price momentum: how far current price has moved from reference
    // Stronger moves = higher confidence in the direction
    let momentum = price_delta_pct.abs().clamp(0.0, 0.02) / 0.02; // normalize to [0, 1]

    // Time pressure: closer to expiry = less time for reversal = more confident
    let time_pressure = 1.0
        - (market.seconds_to_expiry as f64 / settings.snipe_window_seconds as f64).clamp(0.0, 1.0);

    // Liquidity quality: better liquidity = more reliable pricing
    let liquidity_quality =
        ((market.volume + market.liquidity) / settings.snipe_liquidity_scale_usd).clamp(0.0, 1.0);

    // Whale alignment: does whale activity agree with our direction?
    // whale_bias is [-1, 1], we want to check if it agrees with our direction
    let whale_alignment = if going_up {
        whale_bias.max(0.0) // positive bias helps Up
    } else {
        (-whale_bias).max(0.0) // negative bias helps Down
    };

    // Composite confidence score
    let confidence = 0.35 * momentum       // price already moving our way
        + 0.25 * time_pressure             // closer to expiry = safer
        + 0.20 * liquidity_quality          // liquid markets are more reliable
        + 0.20 * whale_alignment;           // whale support

    // Step 5: Urgency Bypass - If last 30 seconds, we always buy the winner
    let is_urgent = market.seconds_to_expiry <= 30;
    
    // Expected edge: confidence minus what we have to pay
    let expected_edge = if is_urgent {
        1.0 // Force it to pass the edge check
    } else {
        confidence - (buy_price - 0.5).max(0.0)
    };

    if !is_urgent && expected_edge < settings.snipe_min_edge {
        return None;
    }

    // Step 6: Calculate stake (scaled by confidence, but maxed if urgent)
    let stake_usd = if is_urgent {
        settings.snipe_max_position_usd
    } else {
        (settings.snipe_max_position_usd * confidence.clamp(0.5, 1.0))
            .min(settings.snipe_max_position_usd)
    };

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
            "{} snipe: {} {} δ={:+.4}% conf={:.3} whale={:+.2} tte={}s",
            if is_urgent { "URGENT" } else { "directional" },
            target_outcome_name,
            symbol,
            price_delta_pct * 100.0,
            confidence,
            whale_bias,
            market.seconds_to_expiry,
        ),
        dry_run: settings.dry_run || !settings.allow_live_buys,
    })
}

fn extract_symbol(slug: &str) -> String {
    slug.split('-')
        .next()
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "UNK".to_string())
}
