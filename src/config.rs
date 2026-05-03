use anyhow::Result;
use std::path::PathBuf;
use tracing::warn;

pub const DEFAULT_SYMBOLS: &str = "BTC,ETH,SOL,XRP";

#[derive(Clone, Debug)]
pub struct Settings {
    pub dry_run: bool,
    pub poll_interval_ms: u64,
    pub state_path: PathBuf,
    pub symbols: Vec<String>,
    pub max_markets: usize,

    pub dashboard_host: String,
    pub dashboard_port: u16,

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

    pub enable_last_minute_5m_snipe: bool,
    pub snipe_window_seconds: i64,
    pub snipe_min_edge: f64,
    pub snipe_max_price: f64,
    pub snipe_min_volume_usd: f64,
    pub snipe_min_liquidity_usd: f64,
    pub snipe_liquidity_scale_usd: f64,
    pub snipe_max_position_usd: f64,
    pub snipe_max_signals: usize,

    pub live_executor_command: String,
    pub live_max_order_usd: f64,
    pub live_min_seconds_to_expiry: i64,
    pub live_order_cooldown_ms: i64,
    pub live_order_type: String,
    pub polymarket_clob_host: String,
    pub polymarket_chain_id: u64,
    pub polymarket_signature_type: Option<u8>,
    pub polymarket_funder_address: String,
}

impl Settings {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            dry_run: env_bool("DRY_RUN", true),
            poll_interval_ms: env_parse("POLL_INTERVAL_MS", 5_000)?,
            state_path: PathBuf::from(env_string("STATE_PATH", "./data/state.json")),
            symbols: parse_symbols(&env_string("SYMBOLS", DEFAULT_SYMBOLS)),
            max_markets: env_parse("MAX_MARKETS", 12)?,

            dashboard_host: env_string("DASHBOARD_HOST", "127.0.0.1"),
            dashboard_port: env_parse("DASHBOARD_PORT", 8080)?,

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

            enable_last_minute_5m_snipe: env_bool("ENABLE_LAST_MINUTE_5M_SNIPE", true),
            snipe_window_seconds: env_parse("SNIPE_WINDOW_SECONDS", 60)?,
            snipe_min_edge: env_parse("SNIPE_MIN_EDGE", 0.02)?,
            snipe_max_price: env_parse("SNIPE_MAX_PRICE", 0.96)?,
            snipe_min_volume_usd: env_parse("SNIPE_MIN_VOLUME_USD", 250.0)?,
            snipe_min_liquidity_usd: env_parse("SNIPE_MIN_LIQUIDITY_USD", 50.0)?,
            snipe_liquidity_scale_usd: env_parse("SNIPE_LIQUIDITY_SCALE_USD", 5_000.0)?,
            snipe_max_position_usd: env_parse("SNIPE_MAX_POSITION_USD", 5.0)?,
            snipe_max_signals: env_parse("SNIPE_MAX_SIGNALS", 8)?,

            live_executor_command: env_string(
                "LIVE_EXECUTOR_COMMAND",
                "python scripts/live_clob_v2_order.py",
            ),
            live_max_order_usd: env_parse("LIVE_MAX_ORDER_USD", 5.0)?,
            live_min_seconds_to_expiry: env_parse("LIVE_MIN_SECONDS_TO_EXPIRY", 3)?,
            live_order_cooldown_ms: env_parse("LIVE_ORDER_COOLDOWN_MS", 20_000)?,
            live_order_type: env_string("LIVE_ORDER_TYPE", "GTC"),
            polymarket_clob_host: env_string("POLYMARKET_CLOB_HOST", "https://clob.polymarket.com"),
            polymarket_chain_id: env_parse("POLYMARKET_CHAIN_ID", 137)?,
            polymarket_signature_type: env_optional_parse("SIGNATURE_TYPE")?,
            polymarket_funder_address: env_string("FUNDER_ADDRESS", ""),
        })
    }

    pub fn log_safety_summary(&self) {
        warn!(
            dry_run = self.dry_run,
            symbols = ?self.symbols,
            max_markets = self.max_markets,
            dashboard = %format!("{}:{}", self.dashboard_host, self.dashboard_port),
            allow_live_buys = self.allow_live_buys,
            allow_live_sells = self.allow_live_sells,
            live_executor_configured = !self.live_executor_command.trim().is_empty(),
            live_max_order_usd = self.live_max_order_usd,
            polymarket_clob_host = %self.polymarket_clob_host,
            polymarket_chain_id = self.polymarket_chain_id,
            signature_type_configured = self.polymarket_signature_type.is_some(),
            funder_configured = !self.polymarket_funder_address.trim().is_empty(),
            auto_take_profit = self.auto_take_profit,
            auto_exit_no_edge = self.auto_exit_no_edge,
            auto_redeem = self.auto_redeem,
            enable_last_minute_5m_snipe = self.enable_last_minute_5m_snipe,
            snipe_window_seconds = self.snipe_window_seconds,
            snipe_max_position_usd = self.snipe_max_position_usd,
            "safety summary"
        );
    }
}

fn parse_symbols(raw: &str) -> Vec<String> {
    let mut symbols = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_uppercase())
        .collect::<Vec<_>>();
    symbols.sort();
    symbols.dedup();
    symbols
}

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
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
            .map_err(|error| anyhow::anyhow!("failed to parse env {key}={value}: {error}")),
        Err(_) => Ok(default),
    }
}

fn env_optional_parse<T>(key: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .parse::<T>()
            .map(Some)
            .map_err(|error| anyhow::anyhow!("failed to parse env {key}={value}: {error}")),
        _ => Ok(None),
    }
}
