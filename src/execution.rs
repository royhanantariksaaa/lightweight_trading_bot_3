use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Settings;
use crate::snipe::SnipeSignal;

const DEFAULT_CLOB_HOST: &str = "https://clob.polymarket.com";
type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct LiveExecutor {
    http: Client,
    settings: Settings,
}

#[derive(Clone, Debug, Serialize)]
pub struct OrderIntent {
    pub token_id: String,
    pub side: String,
    pub price: f64,
    pub size: f64,
    pub order_type: String,
    pub market_slug: String,
    pub outcome: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignedOrderEnvelope {
    pub order: Value,
    #[serde(default = "default_order_type")]
    pub order_type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OrderSubmitResponse {
    #[serde(flatten)]
    pub raw: Value,
}

impl LiveExecutor {
    pub fn new(settings: Settings) -> Self {
        Self { http: Client::new(), settings }
    }

    pub async fn submit_snipe_buy(&self, signal: &SnipeSignal) -> Result<OrderSubmitResponse> {
        self.validate_live_ready(signal)?;

        let token_id = signal
            .token_id
            .clone()
            .ok_or_else(|| anyhow!("live order blocked: signal has no CLOB token_id"))?;

        let price = round_price(signal.price.min(self.settings.snipe_max_price));
        let size = round_size((signal.stake_usd / price).max(0.0));

        if signal.stake_usd > self.settings.live_max_order_usd {
            bail!(
                "live order blocked: stake ${:.2} exceeds LIVE_MAX_ORDER_USD ${:.2}",
                signal.stake_usd,
                self.settings.live_max_order_usd
            );
        }

        if price <= 0.0 || price >= 1.0 || size <= 0.0 {
            bail!("live order blocked: invalid price/size price={price} size={size}");
        }

        let intent = OrderIntent {
            token_id,
            side: "BUY".to_string(),
            price,
            size,
            order_type: self.settings.live_order_type.clone(),
            market_slug: signal.market_slug.clone(),
            outcome: signal.outcome.clone(),
            reason: signal.reason.clone(),
        };

        let signed = self.sign_order(&intent).await?;
        self.post_order(&signed).await
    }

    fn validate_live_ready(&self, signal: &SnipeSignal) -> Result<()> {
        if self.settings.dry_run {
            bail!("live order blocked: DRY_RUN=true");
        }
        if !self.settings.allow_live_buys {
            bail!("live order blocked: ALLOW_LIVE_BUYS=false");
        }
        if !self.settings.live_order_confirm {
            bail!("live order blocked: LIVE_ORDER_CONFIRM=false");
        }
        if signal.dry_run {
            bail!("live order blocked: signal is marked dry_run");
        }
        if signal.token_id.is_none() {
            bail!("live order blocked: no token_id on signal");
        }
        require_env(&self.settings.polymarket_api_key, "POLYMARKET_API_KEY")?;
        require_env(&self.settings.polymarket_api_secret, "POLYMARKET_API_SECRET")?;
        require_env(&self.settings.polymarket_api_passphrase, "POLYMARKET_API_PASSPHRASE")?;
        require_env(&self.settings.polymarket_address, "POLYMARKET_ADDRESS")?;
        require_env(&self.settings.order_signer_command, "ORDER_SIGNER_COMMAND")?;
        Ok(())
    }

    async fn sign_order(&self, intent: &OrderIntent) -> Result<SignedOrderEnvelope> {
        let signer = self.settings.order_signer_command.trim();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(signer)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn ORDER_SIGNER_COMMAND: {signer}"))?;

        let stdin = child.stdin.as_mut().context("failed to open signer stdin")?;
        stdin.write_all(serde_json::to_string(intent)?.as_bytes()).await?;
        drop(child.stdin.take());

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            bail!(
                "ORDER_SIGNER_COMMAND failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let signed: SignedOrderEnvelope = serde_json::from_slice(&output.stdout)
            .context("ORDER_SIGNER_COMMAND must return JSON: {\"order\": {...}, \"order_type\": \"GTC\"}")?;
        Ok(signed)
    }

    async fn post_order(&self, signed: &SignedOrderEnvelope) -> Result<OrderSubmitResponse> {
        let host = self.settings.clob_host.trim_end_matches('/');
        let path = "/order";
        let body = serde_json::json!({
            "order": signed.order,
            "orderType": signed.order_type,
        });
        let body_raw = serde_json::to_string(&body)?;
        let timestamp = Utc::now().timestamp().to_string();
        let signature = clob_l2_signature(
            &self.settings.polymarket_api_secret,
            &timestamp,
            "POST",
            path,
            &body_raw,
        )?;

        let response = self
            .http
            .post(format!("{host}{path}"))
            .header("POLY_ADDRESS", self.settings.polymarket_address.trim())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp)
            .header("POLY_API_KEY", self.settings.polymarket_api_key.trim())
            .header("POLY_PASSPHRASE", self.settings.polymarket_api_passphrase.trim())
            .header("Content-Type", "application/json")
            .body(body_raw)
            .send()
            .await
            .context("failed to submit live CLOB order")?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("CLOB order rejected with status {status}: {text}");
        }

        let raw = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| serde_json::json!({ "raw": text }));
        Ok(OrderSubmitResponse { raw })
    }
}

fn require_env(value: &str, key: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("live order blocked: {key} is empty");
    }
    Ok(())
}

fn clob_l2_signature(secret: &str, timestamp: &str, method: &str, path: &str, body: &str) -> Result<String> {
    let decoded_secret = general_purpose::STANDARD
        .decode(secret.trim())
        .unwrap_or_else(|_| secret.as_bytes().to_vec());
    let payload = format!("{timestamp}{method}{path}{body}");
    let mut mac = HmacSha256::new_from_slice(&decoded_secret).context("invalid CLOB API secret")?;
    mac.update(payload.as_bytes());
    Ok(general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

fn round_price(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round_size(value: f64) -> f64 {
    (value * 1_000_000.0).floor() / 1_000_000.0
}

fn default_order_type() -> String {
    "GTC".to_string()
}

impl Default for LiveExecutor {
    fn default() -> Self {
        Self { http: Client::new(), settings: Settings::default_for_tests() }
    }
}
