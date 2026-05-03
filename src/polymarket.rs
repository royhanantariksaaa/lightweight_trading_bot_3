use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::config::Settings;

const GAMMA_MARKETS_URL: &str = "https://gamma-api.polymarket.com/markets";
const GAMMA_MARKET_BY_SLUG_URL: &str = "https://gamma-api.polymarket.com/markets/slug";
const CLOB_BOOK_URL: &str = "https://clob.polymarket.com/book";

#[derive(Clone, Debug, serde::Serialize)]
pub struct MarketSnapshot {
    pub slug: String,
    pub question: String,
    pub icon: Option<String>,
    pub image: Option<String>,
    pub end_time: Option<DateTime<Utc>>,
    pub seconds_to_expiry: i64,
    pub volume: f64,
    pub liquidity: f64,
    pub price_to_beat: Option<f64>,
    pub current_price: Option<f64>,
    pub outcomes: Vec<OutcomeSnapshot>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct OutcomeSnapshot {
    pub name: String,
    pub token_id: Option<String>,
    pub price: f64,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
}

#[derive(Clone)]
pub struct PolymarketClient {
    http: Client,
}

impl PolymarketClient {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
        }
    }

    pub async fn fetch_active_5m_markets(
        &self,
        settings: &Settings,
    ) -> Result<Vec<MarketSnapshot>> {
        let slug_markets = self.fetch_active_5m_markets_by_slug(settings).await?;
        if !slug_markets.is_empty() {
            return self.hydrate_live_market_data(slug_markets).await;
        }

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

        let raw: Vec<GammaMarket> = response
            .json()
            .await
            .context("failed to decode Gamma markets")?;
        let now = Utc::now();

        let mut markets = raw
            .into_iter()
            .filter_map(|market| market.into_snapshot(now))
            .filter(|market| is_wanted_5m_crypto_market(market, &settings.symbols))
            .take(settings.max_markets)
            .collect::<Vec<_>>();

        markets.sort_by_key(|market| market.seconds_to_expiry);
        self.hydrate_live_market_data(markets).await
    }

    async fn fetch_active_5m_markets_by_slug(
        &self,
        settings: &Settings,
    ) -> Result<Vec<MarketSnapshot>> {
        let now = Utc::now();
        let base_window = floor_to_5m(now);
        let windows = [base_window, base_window - ChronoDuration::minutes(5)];
        let mut markets = Vec::new();

        for symbol in &settings.symbols {
            let prefix = format!("{}-updown-5m", symbol.to_ascii_lowercase());
            for window in windows {
                let slug = format!("{}-{}", prefix, window.timestamp());
                let url = format!("{}/{}", GAMMA_MARKET_BY_SLUG_URL, slug);
                let Ok(response) = self.http.get(&url).send().await else {
                    continue;
                };
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    continue;
                }

                let market = response
                    .error_for_status()
                    .with_context(|| format!("Gamma slug lookup returned error for {slug}"))?
                    .json::<GammaMarket>()
                    .await
                    .with_context(|| format!("failed to decode Gamma market {slug}"))?;

                if let Some(snapshot) = market.into_snapshot(now) {
                    if is_wanted_5m_crypto_market(&snapshot, &settings.symbols)
                        && snapshot.seconds_to_expiry > 0
                        && slug_window_has_started(&snapshot.slug, now)
                    {
                        markets.push(snapshot);
                        break;
                    }
                }
            }
        }

        markets.sort_by_key(|market| market.seconds_to_expiry);
        markets.dedup_by(|left, right| left.slug == right.slug);
        markets.truncate(settings.max_markets);
        Ok(markets)
    }

    async fn hydrate_live_market_data(
        &self,
        mut markets: Vec<MarketSnapshot>,
    ) -> Result<Vec<MarketSnapshot>> {
        for market in &mut markets {
            let symbol = symbol_from_slug(&market.slug);
            if market.price_to_beat.is_none() {
                market.price_to_beat = self
                    .fetch_event_page_price_to_beat(&symbol, &market.slug)
                    .await
                    .ok()
                    .flatten();
            }

            market.current_price = self
                .fetch_chainlink_live_price(&symbol)
                .await
                .ok()
                .flatten();

            for outcome in &mut market.outcomes {
                let Some(token_id) = outcome.token_id.as_deref() else {
                    continue;
                };
                let Ok(book) = self.fetch_book(token_id).await else {
                    continue;
                };
                outcome.best_bid = book.best_bid();
                outcome.best_ask = book.best_ask();
                if let Some(best_ask) = outcome.best_ask.or(outcome.best_bid) {
                    outcome.price = best_ask;
                }
            }
        }
        Ok(markets)
    }

    async fn fetch_book(&self, token_id: &str) -> Result<BookSnapshot> {
        self.http
            .get(CLOB_BOOK_URL)
            .query(&[("token_id", token_id)])
            .send()
            .await
            .context("failed to fetch CLOB book")?
            .error_for_status()
            .context("CLOB book returned error status")?
            .json::<BookSnapshot>()
            .await
            .context("failed to decode CLOB book")
    }

    async fn fetch_chainlink_live_price(&self, symbol: &str) -> Result<Option<f64>> {
        let Some(feed_id) = chainlink_feed_id(symbol) else {
            return Ok(None);
        };
        let response = self
            .http
            .get("https://data.chain.link/api/live-data-engine-stream-data")
            .query(&[
                ("feedId", feed_id),
                ("abiIndex", "0"),
                ("queryWindow", "1m"),
                ("attributeName", "benchmark"),
            ])
            .send()
            .await
            .context("failed to fetch Chainlink live price")?
            .error_for_status()
            .context("Chainlink live price returned error status")?;

        let value = response
            .json::<serde_json::Value>()
            .await
            .context("failed to decode Chainlink live price")?;
        let latest = value
            .get("data")
            .and_then(|node| node.get("allStreamValuesGenerics"))
            .and_then(|node| node.get("nodes"))
            .and_then(serde_json::Value::as_array)
            .and_then(|nodes| {
                nodes.iter().max_by_key(|node| {
                    node.get("validAfterTs")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
            });

        Ok(latest
            .and_then(|node| node.get("valueNumeric"))
            .and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.as_str().and_then(|raw| raw.parse::<f64>().ok()))
            }))
    }

    async fn fetch_event_page_price_to_beat(&self, symbol: &str, slug: &str) -> Result<Option<f64>> {
        let Some(start_seconds) = slug.rsplit('-').next().and_then(|raw| raw.parse::<i64>().ok())
        else {
            return Ok(None);
        };
        let Some(start) = DateTime::<Utc>::from_timestamp(start_seconds, 0) else {
            return Ok(None);
        };
        let end = start + ChronoDuration::minutes(5);
        let start_key = start.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let end_key = end.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let html = self
            .http
            .get(format!("https://polymarket.com/event/{slug}"))
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await
            .context("failed to fetch Polymarket event page")?
            .error_for_status()
            .context("Polymarket event page returned error status")?
            .text()
            .await
            .context("failed to read Polymarket event page")?;

        let query_key = format!(
            "\"queryKey\":[\"crypto-prices\",\"price\",\"{}\",\"{}\",\"fiveminute\",\"{}\"]",
            symbol.to_ascii_uppercase(),
            start_key,
            end_key
        );
        let Some(query_idx) = html.find(&query_key) else {
            return Ok(None);
        };
        let prefix_start = query_idx.saturating_sub(1_500);
        let prefix = &html[prefix_start..query_idx];
        Ok(prefix
            .rfind("\"openPrice\":")
            .and_then(|idx| extract_number_after(&prefix[idx..], "\"openPrice\":")))
    }
}

fn is_wanted_5m_crypto_market(market: &MarketSnapshot, symbols: &[String]) -> bool {
    let haystack = format!("{} {}", market.slug, market.question).to_ascii_lowercase();
    let looks_5m =
        haystack.contains("5m") || haystack.contains("5 minute") || haystack.contains("5-minute");
    let has_symbol = symbols
        .iter()
        .any(|symbol| haystack.contains(&symbol.to_ascii_lowercase()));
    looks_5m && has_symbol
}

fn floor_to_5m(now: DateTime<Utc>) -> DateTime<Utc> {
    now.with_minute((now.minute() / 5) * 5)
        .and_then(|dt| dt.with_second(0))
        .and_then(|dt| dt.with_nanosecond(0))
        .unwrap_or(now)
}

fn slug_window_has_started(slug: &str, now: DateTime<Utc>) -> bool {
    slug.rsplit('-')
        .next()
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(|start_seconds| start_seconds <= now.timestamp() + 2)
        .unwrap_or(true)
}

fn symbol_from_slug(slug: &str) -> String {
    slug.split('-')
        .next()
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "UNK".to_string())
}

fn chainlink_feed_id(symbol: &str) -> Option<&'static str> {
    match symbol.to_ascii_uppercase().as_str() {
        "SOL" => Some("0x0003b778d3f6b2ac4991302b89cb313f99a42467d6c9c5f96f57c29c0d2bc24f"),
        "ETH" => Some("0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9"),
        "BTC" => Some("0x00039d9e45394f473ab1f050a1b963e6b05351e52d71e507509ada0c95ed75b8"),
        "XRP" => Some("0x0003c16c6aed42294f5cb4741f6e59ba2d728f0eae2eb9e6d3f555808c59fc45"),
        _ => None,
    }
}

fn extract_number_after(haystack: &str, marker: &str) -> Option<f64> {
    let start = haystack.find(marker)? + marker.len();
    let rest = &haystack[start..];
    let raw = rest
        .chars()
        .skip_while(|ch| ch.is_whitespace())
        .take_while(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+'))
        .collect::<String>();
    raw.parse::<f64>().ok()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    slug: Option<String>,
    question: Option<String>,
    icon: Option<String>,
    image: Option<String>,
    end_date: Option<String>,
    end_date_iso: Option<String>,
    volume: Option<serde_json::Value>,
    volume_num: Option<f64>,
    liquidity: Option<serde_json::Value>,
    liquidity_num: Option<f64>,
    outcomes: Option<serde_json::Value>,
    outcome_prices: Option<serde_json::Value>,
    clob_token_ids: Option<serde_json::Value>,
    event_metadata: Option<EventMetadata>,
    events: Option<Vec<GammaEvent>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaEvent {
    event_metadata: Option<EventMetadata>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventMetadata {
    price_to_beat: Option<f64>,
}

impl GammaMarket {
    fn into_snapshot(self, now: DateTime<Utc>) -> Option<MarketSnapshot> {
        let price_to_beat = self.price_to_beat();
        let slug = self.slug?;
        let question = self.question.unwrap_or_else(|| slug.clone());
        let end_time = self
            .end_date
            .as_deref()
            .or(self.end_date_iso.as_deref())
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let seconds_to_expiry = end_time
            .map(|dt| (dt - now).num_seconds())
            .unwrap_or(i64::MAX);

        let outcome_names =
            parse_string_array(self.outcomes).unwrap_or_else(|| vec!["Yes".into(), "No".into()]);
        let prices = parse_f64_array(self.outcome_prices).unwrap_or_default();
        let token_ids = parse_string_array(self.clob_token_ids).unwrap_or_default();

        let outcomes = outcome_names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| OutcomeSnapshot {
                name,
                token_id: token_ids.get(idx).cloned(),
                price: prices.get(idx).copied().unwrap_or(0.0),
                best_bid: None,
                best_ask: None,
            })
            .filter(|outcome| outcome.price > 0.0)
            .collect::<Vec<_>>();

        if outcomes.is_empty() {
            return None;
        }

        Some(MarketSnapshot {
            slug,
            question,
            icon: self.icon,
            image: self.image,
            end_time,
            seconds_to_expiry,
            volume: self
                .volume_num
                .or_else(|| parse_f64_value(self.volume))
                .unwrap_or(0.0),
            liquidity: self
                .liquidity_num
                .or_else(|| parse_f64_value(self.liquidity))
                .unwrap_or(0.0),
            price_to_beat,
            current_price: None,
            outcomes,
        })
    }

    fn price_to_beat(&self) -> Option<f64> {
        self.event_metadata
            .as_ref()
            .and_then(|metadata| metadata.price_to_beat)
            .or_else(|| {
                self.events
                    .as_ref()?
                    .iter()
                    .find_map(|event| event.event_metadata.as_ref()?.price_to_beat)
            })
    }
}

#[derive(Debug, Deserialize)]
struct BookSnapshot {
    bids: Option<Vec<BookLevel>>,
    asks: Option<Vec<BookLevel>>,
}

#[derive(Debug, Deserialize)]
struct BookLevel {
    price: String,
}

impl BookSnapshot {
    fn best_bid(&self) -> Option<f64> {
        self.bids
            .as_ref()?
            .iter()
            .filter_map(|level| level.price.parse::<f64>().ok())
            .max_by(|left, right| left.total_cmp(right))
    }

    fn best_ask(&self) -> Option<f64> {
        self.asks
            .as_ref()?
            .iter()
            .filter_map(|level| level.price.parse::<f64>().ok())
            .min_by(|left, right| left.total_cmp(right))
    }
}

fn parse_string_array(value: Option<serde_json::Value>) -> Option<Vec<String>> {
    match value? {
        serde_json::Value::Array(items) => Some(
            items
                .into_iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect(),
        ),
        serde_json::Value::String(raw) => serde_json::from_str::<Vec<String>>(&raw).ok(),
        _ => None,
    }
}

fn parse_f64_array(value: Option<serde_json::Value>) -> Option<Vec<f64>> {
    match value? {
        serde_json::Value::Array(items) => Some(
            items
                .into_iter()
                .filter_map(|item| parse_f64_value(Some(item)))
                .collect(),
        ),
        serde_json::Value::String(raw) => serde_json::from_str::<Vec<serde_json::Value>>(&raw)
            .ok()
            .map(|items| {
                items
                    .into_iter()
                    .filter_map(|item| parse_f64_value(Some(item)))
                    .collect()
            }),
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
