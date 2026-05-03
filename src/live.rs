use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Settings;
use crate::snipe::SnipeSignal;

#[derive(Clone, Debug, Serialize)]
pub struct LiveOrderRequest {
    pub token_id: String,
    pub market_slug: String,
    pub outcome: String,
    pub side: LiveSide,
    pub price: f64,
    pub size: f64,
    pub amount_usd: f64,
    pub order_type: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LiveSide {
    Buy,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LiveOrderResponse {
    pub success: bool,
    pub order_id: Option<String>,
    pub raw: serde_json::Value,
}

pub fn buy_request_from_snipe(
    settings: &Settings,
    signal: &SnipeSignal,
) -> Result<LiveOrderRequest> {
    let token_id = signal
        .token_id
        .clone()
        .ok_or_else(|| anyhow!("cannot place live order without clob token_id"))?;
    guarded_request(
        settings,
        token_id,
        signal.market_slug.clone(),
        signal.outcome.clone(),
        LiveSide::Buy,
        signal.price,
        (signal.stake_usd / signal.price).max(0.0),
        Some(signal.seconds_to_expiry),
    )
}

pub async fn post_live_order(
    settings: &Settings,
    request: &LiveOrderRequest,
) -> Result<LiveOrderResponse> {
    if settings.dry_run {
        bail!("blocked live order: DRY_RUN=true");
    }
    if settings.live_executor_command.trim().is_empty() {
        bail!("blocked live order: LIVE_EXECUTOR_COMMAND is empty");
    }

    let mut parts = settings.live_executor_command.split_whitespace();
    let program = parts.next().context("LIVE_EXECUTOR_COMMAND is empty")?;
    let args = parts.collect::<Vec<_>>();
    let payload = serde_json::to_vec(request).context("failed to encode live order request")?;

    let mut child = Command::new(program)
        .args(args)
        .env("POLYMARKET_CLOB_HOST", &settings.polymarket_clob_host)
        .env(
            "POLYMARKET_CHAIN_ID",
            settings.polymarket_chain_id.to_string(),
        )
        .env("LIVE_ORDER_TYPE", &request.order_type)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn live executor `{}`",
                settings.live_executor_command
            )
        })?;

    let mut stdin = child
        .stdin
        .take()
        .context("failed to open live executor stdin")?;
    stdin
        .write_all(&payload)
        .await
        .context("failed to write live order request")?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .context("live executor failed to exit cleanly")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "live executor returned {}: {}",
            output.status,
            stderr.trim()
        );
    }

    serde_json::from_slice::<LiveOrderResponse>(&output.stdout).with_context(|| {
        format!(
            "failed to decode live executor response: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn guarded_request(
    settings: &Settings,
    token_id: String,
    market_slug: String,
    outcome: String,
    side: LiveSide,
    price: f64,
    size: f64,
    seconds_to_expiry: Option<i64>,
) -> Result<LiveOrderRequest> {
    if token_id.trim().is_empty() {
        bail!("cannot place live order without clob token_id");
    }
    if !(0.0..=1.0).contains(&price) || price <= 0.0 {
        bail!("invalid live order price {price}");
    }
    if size <= 0.0 {
        bail!("invalid live order size {size}");
    }
    if let Some(seconds) = seconds_to_expiry {
        if seconds < settings.live_min_seconds_to_expiry {
            bail!(
                "blocked live order: {}s to expiry is below LIVE_MIN_SECONDS_TO_EXPIRY={}",
                seconds,
                settings.live_min_seconds_to_expiry
            );
        }
    }

    let amount_usd = price * size;
    if amount_usd > settings.live_max_order_usd {
        bail!(
            "blocked live order: ${:.2} exceeds LIVE_MAX_ORDER_USD={:.2}",
            amount_usd,
            settings.live_max_order_usd
        );
    }

    Ok(LiveOrderRequest {
        token_id,
        market_slug,
        outcome,
        side,
        price,
        size,
        amount_usd,
        order_type: settings.live_order_type.clone(),
    })
}
