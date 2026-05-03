use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::polymarket::{MarketSnapshot, OutcomeSnapshot};

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

pub fn find_last_minute_5m_snipes(
    settings: &Settings,
    markets: &[MarketSnapshot],
) -> Vec<SnipeSignal> {
    let mut signals = markets
        .iter()
        .filter(|market| market.seconds_to_expiry >= 0)
        .filter(|market| market.seconds_to_expiry <= settings.snipe_window_seconds)
        .filter(|market| {
            market.volume >= settings.snipe_min_volume_usd
                || market.liquidity >= settings.snipe_min_liquidity_usd
        })
        .flat_map(|market| {
            market
                .outcomes
                .iter()
                .filter_map(move |outcome| score_outcome(settings, market, outcome))
        })
        .collect::<Vec<_>>();

    signals.sort_by(|a, b| {
        b.expected_edge
            .partial_cmp(&a.expected_edge)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    signals.truncate(settings.snipe_max_signals);
    signals
}

fn score_outcome(
    settings: &Settings,
    market: &MarketSnapshot,
    outcome: &OutcomeSnapshot,
) -> Option<SnipeSignal> {
    let price = outcome.price;
    if !(0.01..=settings.snipe_max_price).contains(&price) {
        return None;
    }

    let implied_prob = price;
    let time_pressure = 1.0
        - (market.seconds_to_expiry as f64 / settings.snipe_window_seconds as f64).clamp(0.0, 1.0);
    let liquidity_quality =
        ((market.volume + market.liquidity) / settings.snipe_liquidity_scale_usd).clamp(0.0, 1.0);
    let price_extremity = (0.5 - (implied_prob - 0.5).abs()).clamp(0.0, 0.5) * 2.0;

    // This is a conservative heuristic edge proxy, not a prediction oracle.
    // It favors liquid 5m markets very near expiry where the selected outcome is not already max-priced.
    let expected_edge =
        0.45 * time_pressure + 0.35 * liquidity_quality + 0.20 * price_extremity - implied_prob;

    if expected_edge < settings.snipe_min_edge {
        return None;
    }

    Some(SnipeSignal {
        market_slug: market.slug.clone(),
        question: market.question.clone(),
        outcome: outcome.name.clone(),
        token_id: outcome.token_id.clone(),
        price,
        expected_edge,
        seconds_to_expiry: market.seconds_to_expiry,
        volume: market.volume,
        liquidity: market.liquidity,
        stake_usd: settings.snipe_max_position_usd,
        reason: format!(
            "last-minute 5m candidate: edge_proxy={:.3}, tte={}s, volume={:.0}, liquidity={:.0}",
            expected_edge, market.seconds_to_expiry, market.volume, market.liquidity
        ),
        dry_run: settings.dry_run || !settings.allow_live_buys,
    })
}
