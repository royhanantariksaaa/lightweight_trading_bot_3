use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BotState {
    pub bot_orders: HashMap<String, BotOrder>,
    pub bot_positions: HashMap<String, BotPosition>,
    pub recent_exits: HashMap<String, i64>,
    pub signal_counts: HashMap<String, SignalCounter>,
    #[serde(default)]
    pub reported_closed_markets: HashSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotOrder {
    pub id: String,
    pub market_slug: String,
    pub outcome: String,
    pub limit_price: f64,
    pub shares: f64,
    #[serde(default)]
    pub phase: Option<String>,
    pub created_at_ms: i64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotPosition {
    pub market_slug: String,
    pub outcome: String,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(alias = "entry_price")]
    pub avg_entry_price: f64,
    #[serde(alias = "shares")]
    pub total_shares: f64,
    #[serde(default)]
    pub total_cost_usd: f64,
    pub opened_at_ms: i64,
    #[serde(default)]
    pub last_buy_at_ms: i64,
    #[serde(default)]
    pub confirmation_status: Option<String>,
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
            Ok(raw) => {
                let mut state: Self = serde_json::from_str(&raw)?;
                state.migrate_positions();
                Ok(state)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn migrate_positions(&mut self) {
        for position in self.bot_positions.values_mut() {
            if position.total_cost_usd <= 0.0 {
                position.total_cost_usd = position.avg_entry_price * position.total_shares;
            }
            if position.last_buy_at_ms <= 0 {
                position.last_buy_at_ms = position.opened_at_ms;
            }
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
        self.record_bot_order_with_id(id, market_slug, outcome, limit_price, shares, None);
    }

    pub fn record_bot_order_with_id(
        &mut self,
        id: String,
        market_slug: String,
        outcome: String,
        limit_price: f64,
        shares: f64,
        phase: Option<String>,
    ) {
        self.bot_orders.insert(
            id.clone(),
            BotOrder {
                id,
                market_slug,
                outcome,
                limit_price,
                shares,
                phase,
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

    pub fn mark_order_resolved(&mut self, order_id: &str) {
        if let Some(order) = self.bot_orders.get_mut(order_id) {
            order.status = "resolved".to_string();
        }
    }

    pub fn record_exit(&mut self, market_slug: &str, outcome: &str) {
        let key = position_key(market_slug, outcome);
        self.bot_positions.remove(&key);
        self.recent_exits.insert(key, now_ms());
    }

    pub fn record_position(
        &mut self,
        market_slug: String,
        outcome: String,
        entry_price: f64,
        shares: f64,
    ) {
        self.record_position_with_phase(market_slug, outcome, entry_price, shares, None);
    }

    pub fn record_position_with_phase(
        &mut self,
        market_slug: String,
        outcome: String,
        entry_price: f64,
        shares: f64,
        phase: Option<String>,
    ) {
        let key = position_key(&market_slug, &outcome);
        let cost = entry_price * shares;
        self.bot_positions.insert(
            key,
            BotPosition {
                market_slug,
                outcome,
                phase,
                avg_entry_price: entry_price,
                total_shares: shares,
                total_cost_usd: cost,
                opened_at_ms: now_ms(),
                last_buy_at_ms: now_ms(),
                confirmation_status: None,
            },
        );
    }

    pub fn record_optimistic_position_with_phase(
        &mut self,
        market_slug: String,
        outcome: String,
        entry_price: f64,
        shares: f64,
        phase: Option<String>,
    ) {
        self.record_position_with_phase(
            market_slug.clone(),
            outcome.clone(),
            entry_price,
            shares,
            phase,
        );
        if let Some(position) = self
            .bot_positions
            .get_mut(&position_key(&market_slug, &outcome))
        {
            position.confirmation_status = Some("pending_exchange_confirmation".to_string());
        }
    }

    pub fn confirm_position(&mut self, market_slug: &str, outcome: &str) {
        if let Some(position) = self
            .bot_positions
            .get_mut(&position_key(market_slug, outcome))
        {
            position.confirmation_status = Some("confirmed".to_string());
        }
    }

    pub fn remove_optimistic_position(&mut self, market_slug: &str, outcome: &str) -> bool {
        let key = position_key(market_slug, outcome);
        let should_remove = self
            .bot_positions
            .get(&key)
            .and_then(|position| position.confirmation_status.as_deref())
            == Some("pending_exchange_confirmation");
        if should_remove {
            self.bot_positions.remove(&key);
        }
        should_remove
    }

    pub fn replace_order_id(&mut self, old_id: &str, new_id: String) {
        if old_id == new_id {
            return;
        }
        if let Some(mut order) = self.bot_orders.remove(old_id) {
            order.id = new_id.clone();
            self.bot_orders.insert(new_id, order);
        }
    }

    pub fn record_position_addition(
        &mut self,
        market_slug: &str,
        outcome: &str,
        price: f64,
        shares: f64,
        phase: Option<String>,
    ) {
        let key = position_key(market_slug, outcome);
        if let Some(pos) = self.bot_positions.get_mut(&key) {
            let additional_cost = price * shares;
            pos.total_shares += shares;
            pos.total_cost_usd += additional_cost;
            pos.avg_entry_price = pos.total_cost_usd / pos.total_shares;
            pos.last_buy_at_ms = now_ms();
            if pos.phase.is_none() {
                pos.phase = phase;
            }
        }
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

    pub fn stale_open_order_for_market(&self, market_slug: &str, ttl_ms: i64) -> Option<&BotOrder> {
        let cutoff = now_ms() - ttl_ms;
        self.bot_orders
            .values()
            .filter(|order| order.market_slug == market_slug && order.status == "open")
            .filter(|order| order.created_at_ms <= cutoff)
            .min_by_key(|order| order.created_at_ms)
    }

    pub fn bot_position_opened_at_ms(&self, market_slug: &str, outcome: &str) -> Option<i64> {
        self.bot_positions
            .get(&position_key(market_slug, outcome))
            .map(|position| position.opened_at_ms)
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

    pub fn closed_market_reported(&self, slug: &str) -> bool {
        self.reported_closed_markets.contains(slug)
    }

    pub fn mark_closed_market_reported(&mut self, slug: String) {
        self.reported_closed_markets.insert(slug);
    }

    pub fn clear_inactive_markets(&mut self, active_slugs: &HashSet<String>) -> (usize, usize) {
        let mut resolved_orders = 0;
        for order in self.bot_orders.values_mut() {
            if order.status == "open" && !active_slugs.contains(&order.market_slug) {
                order.status = "resolved".to_string();
                resolved_orders += 1;
            }
        }
        let before_positions = self.bot_positions.len();
        self.bot_positions
            .retain(|_, position| active_slugs.contains(&position.market_slug));
        (resolved_orders, before_positions - self.bot_positions.len())
    }
}

pub fn position_key(market_slug: &str, outcome: &str) -> String {
    format!("{}::{}", market_slug, outcome)
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
