use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::warn;

#[derive(Clone, Debug)]
pub struct Settings {
    pub dry_run: bool,
    pub poll_interval_ms: u64,
    pub state_path: PathBuf,
    pub symbols: Vec<String>,
    pub max_markets: usize,

    pub allow_live_buys: bool,
    pub allow_live_sells: bool,
    pub allow_cancels: bool,
    pub auto_take_profit: bool,
    pub auto_exit_no_edge: bool,
    pub auto_redeem: bool,

    pub entry_confirmation_ticks: usize,
    pub exit_confirmation_ticks: usize,
    pub reentry_cooldown_ms: i64,
    pub min_hold_ms: i64,
    pub maker_order_ttl_ms: i64,
    pub min_edge: f64,
    pub cancel_edge: f64,
    pub max_quote_age_ms: i64,
    pub max_position_usd: f64,
    pub max_open_orders_per_market: usize,
}

impl Settings {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            dry_run: env_bool("DRY_RUN", true),
            poll_interval_ms: env_parse("POLL_INTERVAL_MS", 5_000)?,
            state_path: PathBuf::from(env_string("STATE_PATH", "./data/state.json")),
            symbols: env_string("SYMBOLS", "BTC")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            max_markets: env_parse("MAX_MARKETS", 6)?,

            allow_live_buys: env_bool("ALLOW_LIVE_BUYS", false),
            allow_live_sells: env_bool("ALLOW_LIVE_SELLS", false),
            allow_cancels: env_bool("ALLOW_CANCELS", true),
            auto_take_profit: env_bool("AUTO_TAKE_PROFIT", false),
            auto_exit_no_edge: env_bool("AUTO_EXIT_NO_EDGE", false),
            auto_redeem: env_bool("AUTO_REDEEM", false),

            entry_confirmation_ticks: env_parse("ENTRY_CONFIRMATION_TICKS", 3)?,
            exit_confirmation_ticks: env_parse("EXIT_CONFIRMATION_TICKS", 3)?,
            reentry_cooldown_ms: env_parse("REENTRY_COOLDOWN_MS", 120_000)?,
            min_hold_ms: env_parse("MIN_HOLD_MS", 30_000)?,
            maker_order_ttl_ms: env_parse("MAKER_ORDER_TTL_MS", 5_000)?,
            min_edge: env_parse("MIN_EDGE", 0.015)?,
            cancel_edge: env_parse("CANCEL_EDGE", 0.004)?,
            max_quote_age_ms: env_parse("MAX_QUOTE_AGE_MS", 1_500)?,
            max_position_usd: env_parse("MAX_POSITION_USD", 10.0)?,
            max_open_orders_per_market: env_parse("MAX_OPEN_ORDERS_PER_MARKET", 1)?,
        })
    }

    pub fn log_safety_summary(&self) {
        warn!(
            dry_run = self.dry_run,
            allow_live_buys = self.allow_live_buys,
            allow_live_sells = self.allow_live_sells,
            auto_take_profit = self.auto_take_profit,
            auto_exit_no_edge = self.auto_exit_no_edge,
            auto_redeem = self.auto_redeem,
            "safety summary"
        );
    }
}

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(value) => value
            .parse::<T>()
            .with_context(|| format!("failed to parse env {key}={value}")),
        Err(_) => Ok(default),
    }
}
