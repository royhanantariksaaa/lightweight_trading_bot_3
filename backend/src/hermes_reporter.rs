use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use tracing::{info, warn};

use crate::config::Settings;
use crate::dashboard::WhaleSignal;
use crate::polymarket::{ClosedMarketSnapshot, MarketSnapshot};
use crate::state::{BotOrder, BotPosition, BotState};
use crate::llm::{
    ClosedMarketReport, StrategyLearningContext, TradeExecutionReport, sanitize_filename,
};

#[derive(Clone)]
pub struct HermesReporter {
    pub binary_path: String,
    pub timeout_seconds: u64,
}

impl HermesReporter {
    pub fn new(settings: &Settings) -> Self {
        Self {
            binary_path: settings.hermes_binary_path.clone(),
            timeout_seconds: settings.hermes_timeout_seconds,
        }
    }

    /// Called whenever a market closes (whether the bot traded or not).
    /// Writes the report JSON to file, then invokes Hermes Agent to analyze it.
    pub async fn report_closed_market(
        &self,
        settings: &Settings,
        observed_market: MarketSnapshot,
        final_market: ClosedMarketSnapshot,
        state: &BotState,
        recent_whale_signals: Vec<WhaleSignal>,
    ) -> Result<bool> {
        let bot_positions: Vec<_> = state
            .bot_positions.values()
            .filter(|p| p.market_slug == observed_market.slug)
            .cloned().collect();
        let bot_orders: Vec<_> = state
            .bot_orders.values()
            .filter(|o| o.market_slug == observed_market.slug)
            .cloned().collect();
        self.report_closed_market_from_parts(
            settings,
            observed_market,
            final_market,
            &bot_positions,
            &bot_orders,
            &recent_whale_signals,
        ).await
    }

    /// Same as report_closed_market but takes pre-extracted positions/orders
    /// to avoid borrowing the full BotState (for use in spawned tasks).
    pub async fn report_closed_market_from_parts(
        &self,
        settings: &Settings,
        observed_market: MarketSnapshot,
        final_market: ClosedMarketSnapshot,
        bot_positions: &[BotPosition],
        bot_orders: &[BotOrder],
        recent_whale_signals: &[WhaleSignal],
    ) -> Result<bool> {
        if !settings.enable_hermes_market_reports {
            return Ok(false);
        }

        fs::create_dir_all(&settings.hermes_report_dir).with_context(|| {
            format!(
                "failed to create Hermes report dir {}",
                settings.hermes_report_dir.display()
            )
        })?;

        let report = ClosedMarketReport {
            generated_at: Utc::now().to_rfc3339(),
            observed_market,
            final_market,
            bot_positions: bot_positions.to_vec(),
            bot_orders: bot_orders.to_vec(),
            recent_whale_signals: recent_whale_signals.to_vec(),
            strategy_context: StrategyLearningContext {
                objective: "Improve a safety-first Polymarket 5-minute crypto trading strategy from resolved market evidence without increasing live-trading risk.".to_string(),
                current_guardrails: vec![
                    "Never sell manual positions; only bot-owned positions may be considered.".to_string(),
                    "Keep live order size, cooldown, and wallet safety checks intact.".to_string(),
                    "Prefer parameter or scoring changes over broad rewrites.".to_string(),
                    "Any code patch must be reviewable and must not include secrets.".to_string(),
                ],
                code_change_policy: settings.llm_code_patch_mode.clone(),
                requested_output_schema: serde_json::Value::Null,
            },
        };

        let slug = sanitize_filename(&report.observed_market.slug);
        let report_path = settings.hermes_report_dir.join(format!(
            "{}-{}-report.json",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            slug
        ));
        let report_json = serde_json::to_string_pretty(&report)?;
        fs::write(&report_path, &report_json)
            .with_context(|| format!("failed to write {}", report_path.display()))?;

        let prompt = build_closed_market_prompt(&report_path, &report.observed_market.slug);
        self.invoke_hermes(&prompt).await?;

        info!(slug = %report.observed_market.slug, "closed market reported to Hermes Agent");
        Ok(true)
    }

    /// Called after each trade execution (buy or sell).
    pub async fn report_trade_execution(
        &self,
        settings: &Settings,
        execution: &TradeExecutionReport,
    ) -> Result<bool> {
        if !settings.enable_hermes_market_reports {
            return Ok(false);
        }

        fs::create_dir_all(&settings.hermes_report_dir).with_context(|| {
            format!(
                "failed to create Hermes report dir {}",
                settings.hermes_report_dir.display()
            )
        })?;

        let slug = sanitize_filename(&execution.market_slug);
        let report_path = settings.hermes_report_dir.join(format!(
            "{}-{}-trade-report.json",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            slug
        ));

        let full_report = serde_json::to_string_pretty(&execution)?;
        fs::write(&report_path, &full_report)
            .with_context(|| format!("failed to write {}", report_path.display()))?;

        let prompt = build_trade_execution_prompt(&report_path, &execution.market_slug);
        self.invoke_hermes(&prompt).await?;

        info!(slug = %execution.market_slug, "trade execution reported to Hermes Agent");
        Ok(true)
    }

    /// Invoke `hermes chat -q` to send the prompt to Hermes Agent for analysis.
    async fn invoke_hermes(&self, prompt: &str) -> Result<()> {
        let timeout = std::time::Duration::from_secs(self.timeout_seconds);

        let output = tokio::time::timeout(timeout, async {
            tokio::process::Command::new(&self.binary_path)
                .args([
                    "chat",
                    "-q",
                    prompt,
                    "-Q",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
        })
        .await
        .context("hermes command timed out")?
        .context("failed to spawn hermes process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            warn!(
                %stderr,
                %stdout,
                exit_code = ?output.status.code(),
                "hermes process exited with error"
            );
            bail!(
                "hermes exited with code {:?}: {}",
                output.status.code(),
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            info!(response = %stdout.trim(), "hermes agent response");
        }

        Ok(())
    }
}

fn build_closed_market_prompt(report_path: &PathBuf, market_slug: &str) -> String {
    format!(
        "A 5-minute Polymarket crypto market has closed: {market_slug}\n\
         \n\
         Read the closed market report at: {report_path}\n\
         \n\
         Your task:\n\
         1. Read the report file using read_file\n\
         2. Analyze what happened: the bot's decisions, the market outcome, strategy performance\n\
         3. Decide if changes are needed. Then act based on the change type:\n\
         \n\
         AUTONOMOUS (apply immediately, no waiting):\n\
         - Runtime settings: POST to http://127.0.0.1:8787/api/settings for instant hot-reload\n\
           Fields: dry_run, allow_live_buys, allow_live_sells, live_max_order_usd, snipe_max_position_usd\n\
         - Strategy params in /opt/trading-bot/.env: edit with patch tool ONLY.\n\
           Do NOT restart the bot — the .env changes will take effect on the next natural restart.\n\
           Fields: SNIPE_MIN_EDGE, SNIPE_WINDOW_SECONDS, SNIPE_MAX_PRICE, SNIPE_MIN_VOLUME_USD,\n\
           SNIPE_MIN_LIQUIDITY_USD, LIVE_ORDER_COOLDOWN_MS, POLL_INTERVAL_MS, etc.\n\
         \n\
         REVIEW (save as proposal, do NOT modify source code):\n\
         - Any changes to backend/src/*.rs source code logic\n\
         - Save review as JSON to /var/lib/trading-bot/hermes-reviews/<timestamp>-<slug>-review.json\n\
           Format: {{\"market_slug\":\"...\", \"generated_at\":\"...\", \"summary\":\"...\",\n\
           \"changes\":[{{\"file\":\"path\", \"description\":\"what to change\", \"reason\":\"why\"}}],\n\
           \"parameter_suggestions\":[{{\"key\":\"ENV_VAR\", \"current\":\"val\", \"suggested\":\"val\"}}]}}\n\
         \n\
         Prefer parameter changes over code changes. They're safer and can be applied immediately.\n\
         \n\
         Context:\n\
         - observed_market = what the bot saw while trading\n\
         - final_market = the post-close/resolution snapshot\n\
         - bot_positions/bot_orders = bot-owned state, not the whole wallet\n\
         - recent_whale_signals = supporting context, not ground truth\n\
         \n\
         Safety rules (NEVER violate these):\n\
         - Never disable safety checks or guardrails\n\
         - Never remove dry-run protections\n\
         - Never sell manual positions; only bot-owned positions\n\
         - Never expose secrets, private keys, or wallet addresses\n\
         - Prefer parameter adjustments (.env) over large code rewrites\n\
         - Parameter changes go in backend/.env, code changes in backend/src/\n\
         \n\
         The codebase is at /root/lightweight_trading_bot_3 (Rust backend)\n\
         The running bot dashboard is at http://127.0.0.1:8787\n\
         \n\
         You have full file-system access. Read the report, analyze it, and modify the bot if warranted.",
        market_slug = market_slug,
        report_path = report_path.display()
    )
}

fn build_trade_execution_prompt(report_path: &PathBuf, market_slug: &str) -> String {
    format!(
        "A trade execution event occurred in the Polymarket bot for market: {market_slug}\n\
         \n\
         Read the trade execution report at: {report_path}\n\
         \n\
         Your task:\n\
         1. Read the report file using read_file\n\
         2. Review the trade execution for strategy/safety signals\n\
         3. Check if the execution was successful, if it made sense, and if any parameters should be tuned\n\
         4. If changes are warranted, modify code or .env parameters\n\
         \n\
         Safety rules (NEVER violate these):\n\
         - Never disable safety checks or guardrails\n\
         - Never remove dry-run protections\n\
         - Never expose secrets, private keys, or wallet addresses\n\
         - Prefer parameter adjustments (.env) over large code rewrites\n\
         \n\
         The codebase is at /root/lightweight_trading_bot_3 (Rust backend)\n\
         The running bot dashboard is at http://127.0.0.1:8787\n\
         \n\
         Take action if warranted — you have full access to analyze and modify the bot.",
        market_slug = market_slug,
        report_path = report_path.display()
    )
}
