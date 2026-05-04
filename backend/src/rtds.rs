use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RtdsPayload {
    pub symbol: Option<String>,
    pub value: Option<f64>,
    pub timestamp: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RtdsMessage {
    pub topic: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
    pub payload: Option<RtdsPayload>,
}

/// Shared price cache updated in real-time by the Polymarket RTDS websocket.
/// Use `read_price(symbol)` to get the latest Chainlink price for a symbol.
pub struct PriceCache {
    prices: RwLock<HashMap<String, (f64, i64)>>, // symbol -> (price, timestamp_ms)
}

impl PriceCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            prices: RwLock::new(HashMap::new()),
        })
    }

    pub async fn read_price(&self, symbol: &str) -> Option<f64> {
        let prices = self.prices.read().await;
        prices.get(&symbol.to_uppercase()).map(|(p, _)| *p)
    }

    async fn update(&self, symbol: String, price: f64, ts: i64) {
        let mut prices = self.prices.write().await;
        prices.insert(symbol.to_uppercase(), (price, ts));
    }
}

/// Connect to Polymarket's real-time data websocket and stream
/// `crypto_prices_chainlink` updates into the shared PriceCache.
/// Spawn one per symbol.
pub async fn spawn_rtds_stream(
    cache: Arc<PriceCache>,
    symbol: String,
    ws_url: String,
) -> Result<()> {
    let rtds_symbol = format!("{}/usd", symbol.to_lowercase());

    loop {
        match connect_async(&ws_url).await {
            Ok((stream, _)) => {
                info!("rtds: connected for {}", rtds_symbol);
                let (mut write, mut read) = stream.split();

                // Subscribe to crypto_prices_chainlink for this symbol
                let subscribe = serde_json::json!({
                    "action": "subscribe",
                    "subscriptions": [{
                        "topic": "crypto_prices_chainlink",
                        "type": "*",
                        "filters": serde_json::to_string(&serde_json::json!({"symbol": rtds_symbol})).unwrap_or_default(),
                    }],
                });
                if let Err(e) = write.send(Message::Text(subscribe.to_string().into())).await {
                    warn!("rtds: subscribe failed for {}: {}", rtds_symbol, e);
                    continue;
                }

                let mut ping = interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = ping.tick() => {
                            if write.send(Message::Text("PING".into())).await.is_err() {
                                break;
                            }
                        }
                        message = read.next() => {
                            match message {
                                Some(Ok(Message::Text(text))) => {
                                    if text == "PONG" { continue; }
                                    if let Ok(msg) = serde_json::from_str::<RtdsMessage>(&text) {
                                        if let Some(payload) = &msg.payload {
                                            if let Some(price) = payload.value {
                                                let ts = payload.timestamp.unwrap_or(0);
                                                cache.update(symbol.clone(), price, ts).await;
                                            }
                                        }
                                    }
                                }
                                Some(Ok(Message::Binary(bytes))) => {
                                    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                                        if let Ok(msg) = serde_json::from_str::<RtdsMessage>(&text) {
                                            if let Some(payload) = &msg.payload {
                                                if let Some(price) = payload.value {
                                                    let ts = payload.timestamp.unwrap_or(0);
                                                    cache.update(symbol.clone(), price, ts).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Some(Err(e)) => {
                                    warn!("rtds: read error for {}: {}", rtds_symbol, e);
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(e) => warn!("rtds: connect failed for {}: {}", rtds_symbol, e),
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
