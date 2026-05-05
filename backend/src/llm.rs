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

const QUANT_REVIEW_SYSTEM_PROMPT: &str = r#"You are the bot's Quantitative Trader / Market Microstructure Analyst / Software Engineer.

Your job is to turn each closed-market or trade-execution report into practical strategy intelligence for a 5-minute Polymarket crypto bot.

Required behavior:
1. Act like a quant trader first: evaluate edge quality, entry timing, book/whale support, odds paid, expected value, and whether the trade should have been taken, skipped, exited earlier, or sized differently.
2. Act like an analyst second: separate evidence from speculation. Use only the provided JSON. If required data is missing, say exactly what is missing and how to instrument it.
3. Act like a software engineer third: propose minimal, safe parameter/code changes that improve the strategy without broad rewrites.
4. Preserve safety guardrails: never suggest order size above $1, never suggest disabling wallet/order safety checks, never request private keys/secrets, never suggest restarting the bot from inside an agent session.
5. Respect the current architecture: two-phase strategy, RTDS price feed, Binance book/whale context, GTC/FAK routing, background wallet fetch, order timeouts, adaptive TP.
6. Prefer measurable rules over vague advice. Every recommendation must include the signal/threshold/metric it changes and the reason.
7. If no change is justified, explicitly say HOLD/NO CHANGE and explain why.
8. Code patches are review-only proposals. Keep patches minimal and include them only when the evidence strongly supports a change.

Return JSON only. The frontend depends on these keys:
- review_summary: 1-3 sentences describing what you think should happen next.
- intended_action: one of "NO_CHANGE", "PARAMETER_TUNE", "CODE_PATCH", "INVESTIGATE", "DISABLE_SIGNAL", "ENABLE_SIGNAL".
- expected_impact: concise expected effect on win rate, false positives, fills, exits, or risk.
- evidence: array of concrete facts from the report.
- strategy_recommendations: array of specific trading/strategy recommendations.
- engineering_recommendations: array of implementation or instrumentation recommendations.
- parameter_suggestions: array of { name, current, suggested, reason, confidence }.
- risk_notes: array of risks or reasons not to change.
- code_patch_unified_diff: optional string; empty string if no patch.
"#;

fn quant_review_schema() -> serde_json::Value {
    json!({
        "review_summary": "1-3 sentence summary of what should happen next",
        "intended_action": "NO_CHANGE | PARAMETER_TUNE | CODE_PATCH | INVESTIGATE | DISABLE_SIGNAL | ENABLE_SIGNAL",
        "expected_impact": "concise expected impact",
        "evidence": ["specific facts from report"],
        "strategy_recommendations": ["specific quant/trading recommendation"],
        "engineering_recommendations": ["specific software/instrumentation recommendation"],
        "parameter_suggestions": [{
            "name": "ENV_OR_FIELD",
            "current": "if known",
            "suggested": "value",
            "reason": "why this exact change helps",
            "confidence": "low | medium | high"
        }],
        "risk_notes": ["risk or reason not to change"],
        "code_patch_unified_diff": "optional review-only git diff, or empty string"
    })
}

#[derive(Clone)]
pub struct LlmReporter {
    http: Client,
}

#[derive(Debug, Serialize)]
pub struct ClosedMarketReport {
    pub generated_at: String,
    pub observed_market: MarketSnapshot,
    pub final_market: ClosedMarketSnapshot,
    pub bot_positions: Vec<BotPosition>,
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

#[derive(Debug, Serialize, Clone)]
pub struct TradeExecutionReport {
    pub generated_at: String,
    pub event_type: String,
    pub market_slug: String,
    pub outcome: String,
    pub side: String,
    pub amount_usd: Option<f64>,
    pub price: Option<f64>,
    pub shares: Option<f64>,
    pub success: bool,
    pub reason: String,
    pub exchange_response: Option<serde_json::Value>,
    pub error: Option<String>,
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
    content: Option<serde_json::Value>,
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
            bail!(
                "LLM reporting enabled but LLM_API_KEY/OPENAI_API_KEY/DEEPSEEK_API_KEY is missing"
            );
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

        let bot_positions = state
            .bot_positions
            .values()
            .filter(|position| position.market_slug == observed_market.slug)
            .cloned()
            .collect::<Vec<_>>();
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
            bot_positions,
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
                requested_output_schema: quant_review_schema(),
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
                        "content": QUANT_REVIEW_SYSTEM_PROMPT
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
            .and_then(|content| extract_message_content(&content))
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

    pub async fn report_trade_execution(
        &self,
        settings: &Settings,
        execution: TradeExecutionReport,
    ) -> Result<bool> {
        if !settings.enable_llm_market_reports {
            return Ok(false);
        }
        let Some(api_key) = settings.llm_api_key.as_deref() else {
            bail!(
                "LLM reporting enabled but LLM_API_KEY/OPENAI_API_KEY/DEEPSEEK_API_KEY is missing"
            );
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

        let slug = sanitize_filename(&execution.market_slug);
        let report_path = settings.llm_report_dir.join(format!(
            "{}-{}-trade-report.json",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            slug
        ));
        let report_json = serde_json::to_string_pretty(&execution)?;
        fs::write(&report_path, &report_json)
            .with_context(|| format!("failed to write {}", report_path.display()))?;

        let prompt = format!(
            "A trade execution event occurred in a Polymarket bot.\n\
             Review this event as a quantitative trader, market analyst, and software engineer.\n\
             Decide whether the execution reveals a strategy edge, false positive, fill-quality issue, risk-control issue, or instrumentation gap.\n\
             Return JSON only matching this schema:\n{}\n\n\
             EVENT JSON:\n{}",
            serde_json::to_string_pretty(&quant_review_schema())?,
            report_json
        );

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
                        "content": QUANT_REVIEW_SYSTEM_PROMPT
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
            .and_then(|content| extract_message_content(&content))
            .unwrap_or_else(|| "{\"error\":\"empty model response\"}".to_string());
        let response_path = report_path.with_file_name(
            report_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("trade-report.json")
                .replace("-trade-report.json", "-trade-llm-response.json"),
        );
        fs::write(&response_path, content)
            .with_context(|| format!("failed to write {}", response_path.display()))?;
        Ok(true)
    }
}

fn extract_message_content(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| part.as_str().map(str::to_string))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

fn build_prompt(report_json: &str) -> String {
    format!(
        "A 5-minute Polymarket crypto market has closed. Analyze it for strategy improvement.\n\nContext rules:\n- Treat observed_market as what the bot saw while trading.\n- Treat final_market as the post-close/resolution snapshot.\n- bot_positions and bot_orders are local bot-owned state, not the whole wallet.\n- recent_whale_signals are supporting context, not ground truth.\n- Evaluate the event as a quantitative trader: edge, odds paid, phase quality, whale/book confirmation, false-positive risk, and exit quality.\n- Evaluate the event as a software engineer: exact thresholds/instrumentation/code paths to improve.\n- Return JSON only matching the requested schema in strategy_context.requested_output_schema.\n- If you propose code, return a unified diff only in code_patch_unified_diff and keep it minimal.\n- Do not request secrets, private keys, live wallet actions, or order sizes above $1.\n\nREPORT JSON:\n{report_json}"
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
