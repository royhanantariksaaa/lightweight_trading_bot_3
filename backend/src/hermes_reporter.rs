use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use tracing::{info, warn};

use crate::config::Settings;
use crate::dashboard::WhaleSignal;
use crate::llm::{
    ClosedMarketReport, QUANT_REVIEW_SYSTEM_PROMPT, StrategyLearningContext, TradeExecutionReport,
    quant_review_schema, sanitize_filename,
};
use crate::polymarket::{ClosedMarketSnapshot, MarketSnapshot};
use crate::state::{BotOrder, BotPosition, BotState};

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
            .bot_positions
            .values()
            .filter(|p| p.market_slug == observed_market.slug)
            .cloned()
            .collect();
        let bot_orders: Vec<_> = state
            .bot_orders
            .values()
            .filter(|o| o.market_slug == observed_market.slug)
            .cloned()
            .collect();
        self.report_closed_market_from_parts(
            settings,
            observed_market,
            final_market,
            &bot_positions,
            &bot_orders,
            &recent_whale_signals,
        )
        .await
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
                requested_output_schema: quant_review_schema(),
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
        let reporter = self.clone();
        let slug_for_task = report.observed_market.slug.clone();
        tokio::spawn(async move {
            match reporter.invoke_hermes(&prompt).await {
                Ok(()) => info!(slug = %slug_for_task, "closed market reported to Hermes Agent"),
                Err(error) => {
                    warn!(%error, slug = %slug_for_task, "closed market Hermes report failed")
                }
            }
        });

        info!(slug = %report.observed_market.slug, "closed market queued for Hermes Agent");
        Ok(true)
    }

    /// Called after each trade execution (buy or sell).
    pub async fn report_trade_execution(
        &self,
        settings: &Settings,
        execution: TradeExecutionReport,
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
        let reporter = self.clone();
        let slug_for_task = execution.market_slug.clone();
        tokio::spawn(async move {
            match reporter.invoke_hermes(&prompt).await {
                Ok(()) => info!(slug = %slug_for_task, "trade execution reported to Hermes Agent"),
                Err(error) => {
                    warn!(%error, slug = %slug_for_task, "trade execution Hermes report failed")
                }
            }
        });

        info!(slug = %execution.market_slug, "trade execution queued for Hermes Agent");
        Ok(true)
    }

    /// Invoke `hermes chat -q` to send the prompt to Hermes Agent for analysis.
    async fn invoke_hermes(&self, prompt: &str) -> Result<()> {
        let timeout = std::time::Duration::from_secs(self.timeout_seconds);

        let output = tokio::time::timeout(timeout, async {
            tokio::process::Command::new(&self.binary_path)
                .args(["chat", "-q", prompt, "-Q"])
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
    let response_path = paired_response_path(report_path, "-report.json", "-llm-response.json");
    format!(
        "{role}\n\nA 5-minute Polymarket crypto market has closed: {market_slug}\n\nRead the closed-market report with read_file: {report_path}\n\nAct as the bot's Quantitative Trader / Market Microstructure Analyst / Software Engineer. Your output is for the dashboard LLM Reviews panel, so write a concise JSON review to: {response_path}\n\nRequired workflow:\n1. Read the report file.\n2. Evaluate edge quality, entry timing, odds paid, Binance book/whale support, false-positive risk, fill/exit quality, and whether the bot should have traded, skipped, exited earlier, or held.\n3. Separate evidence from speculation. If data is missing, state the missing data and recommend instrumentation.\n4. Recommend measurable strategy changes only when evidence supports them. Prefer HOLD/NO_CHANGE when evidence is weak.\n5. Do not restart the bot. Do not change live permissions. Do not raise any order or position size above $1.\n6. If you actually change .env parameters, use patch only, keep $1 caps intact, and still save the JSON review. Source-code changes are proposal-only in code_patch_unified_diff.\n\nReturn/save JSON only with this schema:\n{schema}\n\nContext:\n- observed_market = what the bot saw while trading\n- final_market = post-close/resolution snapshot\n- bot_positions/bot_orders = bot-owned state, not whole wallet\n- recent_whale_signals = supporting context, not ground truth\n- codebase = /root/lightweight_trading_bot_3\n- dashboard = http://127.0.0.1:8787\n",
        role = QUANT_REVIEW_SYSTEM_PROMPT,
        market_slug = market_slug,
        report_path = report_path.display(),
        response_path = response_path.display(),
        schema = serde_json::to_string_pretty(&quant_review_schema())
            .unwrap_or_else(|_| "{}".to_string())
    )
}

fn build_trade_execution_prompt(report_path: &PathBuf, market_slug: &str) -> String {
    let response_path = paired_response_path(
        report_path,
        "-trade-report.json",
        "-trade-llm-response.json",
    );
    format!(
        "{role}\n\nA trade execution event occurred in the Polymarket bot for market: {market_slug}\n\nRead the trade execution report with read_file: {report_path}\n\nAct as the bot's Quantitative Trader / Market Microstructure Analyst / Software Engineer. Your output is for the dashboard LLM Reviews panel, so write a concise JSON review to: {response_path}\n\nRequired workflow:\n1. Read the report file.\n2. Evaluate execution quality, order reason, fill/rejection/timeout details, odds paid, phase quality, whale/book support, expected edge, and risk-control behavior.\n3. Decide if this points to a strategy edge, false positive, routing/fill-quality issue, exit issue, or instrumentation gap.\n4. Recommend measurable parameter/code changes only when evidence supports them. Prefer HOLD/NO_CHANGE when evidence is weak.\n5. Do not restart the bot. Do not change live permissions. Do not raise any order or position size above $1.\n6. If you actually change .env parameters, use patch only, keep $1 caps intact, and still save the JSON review. Source-code changes are proposal-only in code_patch_unified_diff.\n\nReturn/save JSON only with this schema:\n{schema}\n\nContext:\n- codebase = /root/lightweight_trading_bot_3\n- dashboard = http://127.0.0.1:8787\n",
        role = QUANT_REVIEW_SYSTEM_PROMPT,
        market_slug = market_slug,
        report_path = report_path.display(),
        response_path = response_path.display(),
        schema = serde_json::to_string_pretty(&quant_review_schema())
            .unwrap_or_else(|_| "{}".to_string())
    )
}

fn paired_response_path(report_path: &PathBuf, suffix: &str, response_suffix: &str) -> PathBuf {
    report_path.with_file_name(
        report_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("hermes-report.json")
            .replace(suffix, response_suffix),
    )
}
