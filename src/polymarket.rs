use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::config::Settings;

const GAMMA_MARKETS_URL: &str = "https://gamma-api.polymarket.com/markets";

#[derive(Clone, Debug, serde::Serialize)]
pub struct MarketSnapshot {
    pub slug: String,
    pub question: String,
    pub end_time: Option<DateTime<Utc>>,
    pub seconds_to_expiry: i64,
    pub volume: f64,
    pub liquidity: f64,
    pub outcomes: Vec<OutcomeSnapshot>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct OutcomeSnapshot {
    pub name: String,
    pub token_id: Option<String>,
    pub price: f64,
}

#[derive(Clone)]
pub struct PolymarketClient {
    http: Client,
}

impl PolymarketClient {
    pub fn new() -> Self {
        Self { http: Client::new() }
    }

    pub async fn fetch_active_5m_markets(&self, settings: &Settings) -> Result<Vec<MarketSnapshot>> {
        let limit = settings.max_markets.max(10).min(100).to_string();
        let response = self
            .http
            .get(GAMMA_MARKETS_URL)
            .query(&[
                ("active", "true"),
                ("closed", "false"),
                ("limit", limit.as_str()),
                ("order", "endDate"),
                ("ascending", "true"),
            ])
            .send()
            .await
            .context("failed to fetch Polymarket Gamma markets")?
            .error_for_status()
            .context("Polymarket Gamma markets returned error status")?;

        let raw: Vec<GammaMarket> = response.json().await.context("failed to decode Gamma markets")?;
        let now = Utc::now();

        let mut markets = raw
            .into_iter()
            .filter_map(|market| market.into_snapshot(now))
            .filter(|market| is_wanted_5m_crypto_market(market, &settings.symbols))
            .take(settings.max_markets)
            .collect::<Vec<_>>();

        markets.sort_by_key(|market| market.seconds_to_expiry);
        Ok(markets)
    }
}

fn is_wanted_5m_crypto_market(market: &MarketSnapshot, symbols: &[String]) -> bool {
    let haystack = format!("{} {}", market.slug, market.question).to_ascii_lowercase();
    let looks_5m = haystack.contains("5m") || haystack.contains("5 minute") || haystack.contains("5-minute");
    let has_symbol = symbols.iter().any(|symbol| haystack.contains(&symbol.to_ascii_lowercase()));
    looks_5m && has_symbol
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    slug: Option<String>,
    question: Option<String>,
    end_date: Option<String>,
    end_date_iso: Option<String>,
    volume: Option<serde_json::Value>,
    volume_num: Option<f64>,
    liquidity: Option<serde_json::Value>,
    liquidity_num: Option<f64>,
    outcomes: Option<serde_json::Value>,
    outcome_prices: Option<serde_json::Value>,
    clob_token_ids: Option<serde_json::Value>,
}

impl GammaMarket {
    fn into_snapshot(self, now: DateTime<Utc>) -> Option<MarketSnapshot> {
        let slug = self.slug?;
        let question = self.question.unwrap_or_else(|| slug.clone());
        let end_raw = self.end_date_iso.or(self.end_date);
        let end_time = end_raw
            .as_deref()
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let seconds_to_expiry = end_time.map(|dt| (dt - now).num_seconds()).unwrap_or(i64::MAX);

        let outcome_names = parse_string_array(self.outcomes).unwrap_or_else(|| vec!["Yes".into(), "No".into()]);
        let prices = parse_f64_array(self.outcome_prices).unwrap_or_default();
        let token_ids = parse_string_array(self.clob_token_ids).unwrap_or_default();

        let outcomes = outcome_names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| OutcomeSnapshot {
                name,
                token_id: token_ids.get(idx).cloned(),
                price: prices.get(idx).copied().unwrap_or(0.0),
            })
            .filter(|outcome| outcome.price > 0.0)
            .collect::<Vec<_>>();

        if outcomes.is_empty() {
            return None;
        }

        Some(MarketSnapshot {
            slug,
            question,
            end_time,
            seconds_to_expiry,
            volume: self.volume_num.or_else(|| parse_f64_value(self.volume)).unwrap_or(0.0),
            liquidity: self.liquidity_num.or_else(|| parse_f64_value(self.liquidity)).unwrap_or(0.0),
            outcomes,
        })
    }
}

fn parse_string_array(value: Option<serde_json::Value>) -> Option<Vec<String>> {
    match value? {
        serde_json::Value::Array(items) => Some(items.into_iter().filter_map(|item| item.as_str().map(ToOwned::to_owned)).collect()),
        serde_json::Value::String(raw) => serde_json::from_str::<Vec<String>>(&raw).ok(),
        _ => None,
    }
}

fn parse_f64_array(value: Option<serde_json::Value>) -> Option<Vec<f64>> {
    match value? {
        serde_json::Value::Array(items) => Some(items.into_iter().filter_map(|item| parse_f64_value(Some(item))).collect()),
        serde_json::Value::String(raw) => serde_json::from_str::<Vec<serde_json::Value>>(&raw)
            .ok()
            .map(|items| items.into_iter().filter_map(|item| parse_f64_value(Some(item))).collect()),
        _ => None,
    }
}

fn parse_f64_value(value: Option<serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(raw) => raw.parse::<f64>().ok(),
        _ => None,
    }
}
