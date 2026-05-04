use anyhow::{Result, bail};
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

    pub enable_whale_detector: bool,
    pub whale_mini_usd: f64,
    pub whale_usd: f64,
    pub whale_super_usd: f64,
    pub whale_wall_min_usd: f64,
    pub whale_tracking_window_ms: i64,
    pub whale_symbols: Vec<String>,

    pub live_max_order_usd: f64,
    pub live_min_seconds_to_expiry: i64,
    pub live_order_cooldown_ms: i64,
    pub live_order_type: String,
    pub polymarket_clob_host: String,
    pub polymarket_chain_id: u64,
    pub polymarket_signature_type: Option<u8>,
    pub polymarket_funder_address: String,
    pub polymarket_private_key: Option<String>,

    pub enable_llm_market_reports: bool,
    pub llm_api_base: String,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    pub llm_report_dir: PathBuf,
    pub llm_code_patch_mode: String,
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

            enable_whale_detector: env_bool("ENABLE_WHALE_DETECTOR", true),
            whale_mini_usd: env_parse("WHALE_MINI_USD", 5_000.0)?,
            whale_usd: env_parse("WHALE_USD", 10_000.0)?,
            whale_super_usd: env_parse("WHALE_SUPER_USD", 25_000.0)?,
            whale_wall_min_usd: env_parse("WHALE_WALL_MIN_USD", 25_000.0)?,
            whale_tracking_window_ms: env_parse("WHALE_TRACKING_WINDOW_MS", 300_000)?,
            whale_symbols: parse_symbols(&env_string("WHALE_SYMBOLS", "")),

            live_max_order_usd: env_parse("LIVE_MAX_ORDER_USD", 5.0)?,
            live_min_seconds_to_expiry: env_parse("LIVE_MIN_SECONDS_TO_EXPIRY", 3)?,
            live_order_cooldown_ms: env_parse("LIVE_ORDER_COOLDOWN_MS", 20_000)?,
            live_order_type: env_string("LIVE_ORDER_TYPE", "FAK"),
            polymarket_clob_host: env_string("POLYMARKET_CLOB_HOST", "https://clob.polymarket.com"),
            polymarket_chain_id: env_parse("POLYMARKET_CHAIN_ID", 137)?,
            polymarket_signature_type: env_optional_parse("SIGNATURE_TYPE")?,
            polymarket_funder_address: env_string("FUNDER_ADDRESS", ""),
            polymarket_private_key: env_optional_string("POLYMARKET_PRIVATE_KEY"),

            enable_llm_market_reports: env_bool("ENABLE_LLM_MARKET_REPORTS", false),
            llm_api_base: env_string("LLM_API_BASE", "https://api.openai.com/v1"),
            llm_api_key: env_optional_string("LLM_API_KEY")
                .or_else(|| env_optional_string("OPENAI_API_KEY"))
                .or_else(|| env_optional_string("DEEPSEEK_API_KEY")),
            llm_model: env_string("LLM_MODEL", ""),
            llm_report_dir: PathBuf::from(env_string(
                "LLM_REPORT_DIR",
                "./data/llm-reports",
            )),
            llm_code_patch_mode: env_string("LLM_CODE_PATCH_MODE", "proposal_only"),
        })
    }

    pub fn apply_runtime_update(&mut self, update: RuntimeSettingsUpdate) -> Result<()> {
        let mut next = self.clone();
        next.dry_run = update.dry_run;
        next.allow_live_buys = update.allow_live_buys;
        next.allow_live_sells = update.allow_live_sells;
        next.live_max_order_usd = update.live_max_order_usd;
        next.snipe_max_position_usd = update.snipe_max_position_usd;
        next.polymarket_signature_type = update.signature_type;
        next.polymarket_funder_address = update.funder_address.trim().to_string();
        next.enable_llm_market_reports = update.enable_llm_market_reports;
        next.llm_api_base = update.llm_api_base.trim().to_string();
        next.llm_model = update.llm_model.trim().to_string();
        next.llm_report_dir = PathBuf::from(update.llm_report_dir.trim());
        next.llm_code_patch_mode = update.llm_code_patch_mode.trim().to_string();

        let mut env_updates = vec![
            ("DRY_RUN", update.dry_run.to_string()),
            ("ALLOW_LIVE_BUYS", update.allow_live_buys.to_string()),
            ("ALLOW_LIVE_SELLS", update.allow_live_sells.to_string()),
            ("LIVE_MAX_ORDER_USD", update.live_max_order_usd.to_string()),
            ("SNIPE_MAX_POSITION_USD", update.snipe_max_position_usd.to_string()),
            ("FUNDER_ADDRESS", update.funder_address.trim().to_string()),
            (
                "ENABLE_LLM_MARKET_REPORTS",
                update.enable_llm_market_reports.to_string(),
            ),
            ("LLM_API_BASE", update.llm_api_base.trim().to_string()),
            ("LLM_MODEL", update.llm_model.trim().to_string()),
            ("LLM_REPORT_DIR", update.llm_report_dir.trim().to_string()),
            (
                "LLM_CODE_PATCH_MODE",
                update.llm_code_patch_mode.trim().to_string(),
            ),
        ];

        if let Some(st) = update.signature_type {
            env_updates.push(("SIGNATURE_TYPE", st.to_string()));
        } else {
            env_updates.push(("SIGNATURE_TYPE", "".to_string()));
        }

        if let Some(private_key) = update.private_key {
            let trimmed = private_key.trim();
            next.polymarket_private_key = if trimmed.is_empty() {
                next.polymarket_private_key
            } else {
                env_updates.push(("POLYMARKET_PRIVATE_KEY", trimmed.to_string()));
                Some(trimmed.to_string())
            };
        }

        if let Some(api_key) = update.llm_api_key {
            let trimmed = api_key.trim();
            if !trimmed.is_empty() {
                env_updates.push(("LLM_API_KEY", trimmed.to_string()));
                next.llm_api_key = Some(trimmed.to_string());
            }
        }

        next.validate_runtime_wallet()?;
        next.validate_runtime_llm()?;
        persist_env(&env_updates)?;

        *self = next;
        Ok(())
    }

    pub fn validate_runtime_wallet(&self) -> Result<()> {
        if !self.dry_run && self.allow_live_buys && self.polymarket_private_key.is_none() {
            bail!("live buys require a Polymarket private key");
        }
        if !self.polymarket_funder_address.trim().is_empty()
            && self.polymarket_signature_type.is_none()
        {
            bail!("funder address requires signature type");
        }
        Ok(())
    }

    pub fn validate_runtime_llm(&self) -> Result<()> {
        if !self.enable_llm_market_reports {
            return Ok(());
        }
        if self.llm_api_base.trim().is_empty() {
            bail!("LLM reporting requires an API base URL");
        }
        if self.llm_model.trim().is_empty() {
            bail!("LLM reporting requires a model");
        }
        if self.llm_api_key.is_none() {
            bail!("LLM reporting requires an API key");
        }
        if self.llm_report_dir.as_os_str().is_empty() {
            bail!("LLM reporting requires a report directory");
        }
        Ok(())
    }

    pub fn log_safety_summary(&self) {
        warn!(
            dry_run = self.dry_run,
            symbols = ?self.symbols,
            max_markets = self.max_markets,
            dashboard = %format!("{}:{}", self.dashboard_host, self.dashboard_port),
            allow_live_buys = self.allow_live_buys,
            allow_live_sells = self.allow_live_sells,
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
            enable_whale_detector = self.enable_whale_detector,
            whale_symbols = ?self.effective_whale_symbols(),
            enable_llm_market_reports = self.enable_llm_market_reports,
            llm_model_configured = !self.llm_model.trim().is_empty(),
            llm_api_key_configured = self.llm_api_key.is_some(),
            llm_code_patch_mode = %self.llm_code_patch_mode,
            "safety summary"
        );
    }

    pub fn effective_whale_symbols(&self) -> Vec<String> {
        if self.whale_symbols.is_empty() {
            self.symbols.clone()
        } else {
            self.whale_symbols.clone()
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct RuntimeSettingsUpdate {
    pub dry_run: bool,
    pub allow_live_buys: bool,
    pub allow_live_sells: bool,
    pub live_max_order_usd: f64,
    pub snipe_max_position_usd: f64,
    pub funder_address: String,
    pub signature_type: Option<u8>,
    pub private_key: Option<String>,
    pub enable_llm_market_reports: bool,
    pub llm_api_base: String,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    pub llm_report_dir: String,
    pub llm_code_patch_mode: String,
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

fn env_optional_string(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
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

fn persist_env(updates: &[(&str, String)]) -> Result<()> {
    let env_path = std::env::current_dir()?.join(".env");
    let content = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut lines: Vec<String> = content.lines().map(ToString::to_string).collect();

    for (key, value) in updates {
        let prefix = format!("{key}=");
        lines.retain(|line| !line.starts_with(&prefix));
        lines.push(format!("{key}={value}"));
    }

    // Ensure the file ends with a newline
    let mut new_content = lines.join("\n");
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    std::fs::write(&env_path, new_content)?;
    Ok(())
}
