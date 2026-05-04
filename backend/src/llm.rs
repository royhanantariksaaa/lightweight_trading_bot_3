use anyhow::{Context, Result, bail};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;

use crate::config::Settings;
use crate::dashboard::WhaleSignal;
use crate::polymarket::{ClosedMarketSnapshot, MarketSnapshot};
use crate::state::{BotOrder, BotPosition, BotState};

#[derive(Clone)]
pub struct LlmReporter {
    http: Client,
}

#[derive(Debug, Serialize)]
pub struct ClosedMarketReport {
    pub generated_at: String,
    pub observed_market: MarketSnapshot,
    pub final_market: ClosedMarketSnapshot,
    pub bot_position: Option<BotPosition>,
    pub bot_orders: Vec<BotOrder>,
    pub recent_whale_signals: Vec<WhaleSignal>,
    pub strategy_context: StrategyLearningContext,
}

#[derive(Debug, Serialize)]
pub struct StrategyLearningContext {
    pub objective: String,
    pub current_guardrails: Vec<String>,
    pub code_change_policy: String,
    pub requested_output_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

impl LlmReporter {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
        }
    }

    pub async fn report_closed_market(
        &self,
        settings: &Settings,
        observed_market: MarketSnapshot,
        final_market: ClosedMarketSnapshot,
        state: &BotState,
        recent_whale_signals: Vec<WhaleSignal>,
    ) -> Result<bool> {
        if !settings.enable_llm_market_reports {
            return Ok(false);
        }
        let Some(api_key) = settings.llm_api_key.as_deref() else {
            bail!("LLM reporting enabled but LLM_API_KEY/OPENAI_API_KEY/DEEPSEEK_API_KEY is missing");
        };
        if settings.llm_model.trim().is_empty() {
            bail!("LLM reporting enabled but LLM_MODEL is empty");
        }

        fs::create_dir_all(&settings.llm_report_dir).with_context(|| {
            format!(
                "failed to create LLM report dir {}",
                settings.llm_report_dir.display()
            )
        })?;

        let key_prefix = format!("{}::", observed_market.slug);
        let bot_position = state
            .bot_positions
            .iter()
            .find(|(key, _)| key.starts_with(&key_prefix))
            .map(|(_, position)| position.clone());
        let bot_orders = state
            .bot_orders
            .values()
            .filter(|order| order.market_slug == observed_market.slug)
            .cloned()
            .collect::<Vec<_>>();
        let report = ClosedMarketReport {
            generated_at: Utc::now().to_rfc3339(),
            observed_market,
            final_market,
            bot_position,
            bot_orders,
            recent_whale_signals,
            strategy_context: StrategyLearningContext {
                objective: "Improve a safety-first Polymarket 5-minute crypto trading strategy from resolved market evidence without increasing live-trading risk.".to_string(),
                current_guardrails: vec![
                    "Never sell manual positions; only bot-owned positions may be considered.".to_string(),
                    "Keep live order size, cooldown, and wallet safety checks intact.".to_string(),
                    "Prefer parameter or scoring changes over broad rewrites.".to_string(),
                    "Any code patch must be reviewable and must not include secrets.".to_string(),
                ],
                code_change_policy: settings.llm_code_patch_mode.clone(),
                requested_output_schema: json!({
                    "market_summary": "short factual summary",
                    "bot_decision_quality": "what the bot did well/poorly",
                    "strategy_lessons": ["specific lesson"],
                    "parameter_suggestions": [{"name": "ENV_OR_FIELD", "current": "if known", "suggested": "value", "reason": "why"}],
                    "code_patch_unified_diff": "optional review-only git diff, or empty string",
                    "risk_notes": ["risk or reason not to change"]
                }),
            },
        };

        let slug = sanitize_filename(&report.observed_market.slug);
        let report_path = settings.llm_report_dir.join(format!(
            "{}-{}-report.json",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            slug
        ));
        let report_json = serde_json::to_string_pretty(&report)?;
        fs::write(&report_path, &report_json)
            .with_context(|| format!("failed to write {}", report_path.display()))?;

        let prompt = build_prompt(&report_json);
        let response = self
            .http
            .post(format!(
                "{}/chat/completions",
                settings.llm_api_base.trim_end_matches('/')
            ))
            .bearer_auth(api_key)
            .json(&json!({
                "model": settings.llm_model,
                "messages": [
                    {
                        "role": "system",
                        "content": "You are a cautious trading-strategy review agent. Use only the report data. Do not claim certainty when the data is missing. Return concise JSON only. Code patches are review-only proposals and must preserve all safety guardrails."
                    },
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "temperature": 0.2
            }))
            .send()
            .await
            .context("failed to call LLM chat completions endpoint")?
            .error_for_status()
            .context("LLM chat completions endpoint returned an error")?
            .json::<ChatCompletionResponse>()
            .await
            .context("failed to decode LLM chat completions response")?;

        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .unwrap_or_else(|| "{\"error\":\"empty model response\"}".to_string());
        let response_path = report_path.with_file_name(
            report_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("market-report.json")
                .replace("-report.json", "-llm-response.json"),
        );
        fs::write(&response_path, content)
            .with_context(|| format!("failed to write {}", response_path.display()))?;
        Ok(true)
    }
}

fn build_prompt(report_json: &str) -> String {
    format!(
        "A 5-minute Polymarket crypto market has closed. Analyze it for strategy improvement.\n\nContext rules:\n- Treat observed_market as what the bot saw while trading.\n- Treat final_market as the post-close/resolution snapshot.\n- bot_position and bot_orders are local bot-owned state, not the whole wallet.\n- recent_whale_signals are supporting context, not ground truth.\n- If you propose code, return a unified diff only in code_patch_unified_diff and keep it minimal.\n- Do not request secrets, private keys, or live wallet actions.\n\nREPORT JSON:\n{report_json}"
    )
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
