use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BotState {
    pub bot_orders: HashMap<String, BotOrder>,
    pub bot_positions: HashMap<String, BotPosition>,
    pub recent_exits: HashMap<String, i64>,
    pub signal_counts: HashMap<String, SignalCounter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotOrder {
    pub id: String,
    pub market_slug: String,
    pub outcome: String,
    pub limit_price: f64,
    pub shares: f64,
    pub created_at_ms: i64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotPosition {
    pub market_slug: String,
    pub outcome: String,
    pub entry_price: f64,
    pub shares: f64,
    pub opened_at_ms: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SignalCounter {
    pub entry_ticks: usize,
    pub exit_ticks: usize,
    pub last_seen_ms: i64,
}

impl BotState {
    pub async fn load_or_default(path: &Path) -> Result<Self> {
        match tokio::fs::read_to_string(path).await {
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let body = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, body).await?;
        Ok(())
    }

    pub fn record_bot_order(
        &mut self,
        market_slug: String,
        outcome: String,
        limit_price: f64,
        shares: f64,
    ) {
        let id = Uuid::new_v4().to_string();
        self.record_bot_order_with_id(id, market_slug, outcome, limit_price, shares);
    }

    pub fn record_bot_order_with_id(
        &mut self,
        id: String,
        market_slug: String,
        outcome: String,
        limit_price: f64,
        shares: f64,
    ) {
        self.bot_orders.insert(
            id.clone(),
            BotOrder {
                id,
                market_slug,
                outcome,
                limit_price,
                shares,
                created_at_ms: now_ms(),
                status: "open".to_string(),
            },
        );
    }

    pub fn mark_order_cancelled(&mut self, order_id: &str) {
        if let Some(order) = self.bot_orders.get_mut(order_id) {
            order.status = "cancelled".to_string();
        }
    }

    pub fn record_exit(&mut self, market_slug: &str, outcome: &str) {
        let key = position_key(market_slug, outcome);
        self.bot_positions.remove(&key);
        self.recent_exits.insert(key, now_ms());
    }

    pub fn bot_owns_position(&self, market_slug: &str, outcome: &str) -> bool {
        self.bot_positions
            .contains_key(&position_key(market_slug, outcome))
    }

    pub fn recent_exit_ms(&self, market_slug: &str, outcome: &str) -> Option<i64> {
        self.recent_exits
            .get(&position_key(market_slug, outcome))
            .copied()
    }

    pub fn open_orders_for_market(&self, market_slug: &str) -> usize {
        self.bot_orders
            .values()
            .filter(|order| order.market_slug == market_slug && order.status == "open")
            .count()
    }

    pub fn update_signal_counts(
        &mut self,
        key: &str,
        entry_valid: bool,
        exit_valid: bool,
    ) -> SignalCounter {
        let counter = self.signal_counts.entry(key.to_string()).or_default();
        counter.entry_ticks = if entry_valid {
            counter.entry_ticks + 1
        } else {
            0
        };
        counter.exit_ticks = if exit_valid {
            counter.exit_ticks + 1
        } else {
            0
        };
        counter.last_seen_ms = now_ms();
        counter.clone()
    }
}

pub fn position_key(market_slug: &str, outcome: &str) -> String {
    format!("{}::{}", market_slug, outcome)
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
