use anyhow::{Context, Result, anyhow, bail};
use polymarket_client_sdk_v2::PRIVATE_KEY_VAR;
use polymarket_client_sdk_v2::auth::{LocalSigner, Signer as _};
use polymarket_client_sdk_v2::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk_v2::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk_v2::types::{Address, Decimal, U256};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

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

    let private_key = std::env::var(PRIVATE_KEY_VAR)
        .with_context(|| format!("{PRIVATE_KEY_VAR} is required for live trading"))?;
    let signer = LocalSigner::from_str(private_key.trim())
        .context("failed to parse POLYMARKET_PRIVATE_KEY")?
        .with_chain_id(Some(settings.polymarket_chain_id));

    let mut auth = ClobClient::new(&settings.polymarket_clob_host, ClobConfig::default())
        .context("failed to create Polymarket CLOB SDK client")?
        .authentication_builder(&signer);

    if let Some(signature_type) = sdk_signature_type(settings.polymarket_signature_type)? {
        auth = auth.signature_type(signature_type);
    }

    let funder = settings.polymarket_funder_address.trim();
    if !funder.is_empty() {
        if settings.polymarket_signature_type.is_none() {
            bail!("FUNDER_ADDRESS requires SIGNATURE_TYPE=1, 2, or 3");
        }
        auth = auth.funder(
            Address::from_str(funder)
                .with_context(|| format!("invalid FUNDER_ADDRESS={funder}"))?,
        );
    }

    let client = auth
        .authenticate()
        .await
        .context("failed to authenticate Polymarket CLOB SDK client")?;

    let token_id = U256::from_str(&request.token_id)
        .with_context(|| format!("invalid CLOB token_id={}", request.token_id))?;
    let price = decimal_from_f64(request.price, "price")?;
    let size = decimal_from_f64(request.size, "size")?;
    let order_type = sdk_order_type(&request.order_type)?;

    let response = client
        .limit_order()
        .token_id(token_id)
        .side(sdk_side(&request.side))
        .price(price)
        .size(size)
        .order_type(order_type)
        .build_sign_and_post(&signer)
        .await
        .context("failed to build, sign, and post Polymarket CLOB V2 order")?;

    let raw = serde_json::json!({
        "success": response.success,
        "order_id": response.order_id,
        "status": format!("{:?}", response.status),
        "error_msg": response.error_msg,
        "making_amount": response.making_amount.to_string(),
        "taking_amount": response.taking_amount.to_string(),
        "transaction_hashes": response.transaction_hashes.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "trade_ids": response.trade_ids,
    });
    Ok(LiveOrderResponse {
        success: response.success,
        order_id: Some(response.order_id),
        raw,
    })
}

fn sdk_order_type(raw: &str) -> Result<OrderType> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "GTC" => Ok(OrderType::GTC),
        "FOK" => Ok(OrderType::FOK),
        "GTD" => Ok(OrderType::GTD),
        "FAK" => Ok(OrderType::FAK),
        other => bail!("unsupported LIVE_ORDER_TYPE={other}; expected GTC, FOK, GTD, or FAK"),
    }
}

fn sdk_side(side: &LiveSide) -> Side {
    match side {
        LiveSide::Buy => Side::Buy,
    }
}

fn sdk_signature_type(raw: Option<u8>) -> Result<Option<SignatureType>> {
    match raw {
        None => Ok(None),
        Some(0) => Ok(Some(SignatureType::Eoa)),
        Some(1) => Ok(Some(SignatureType::Proxy)),
        Some(2) => Ok(Some(SignatureType::GnosisSafe)),
        Some(3) => Ok(Some(SignatureType::Poly1271)),
        Some(other) => bail!("unsupported SIGNATURE_TYPE={other}; expected 0, 1, 2, or 3"),
    }
}

fn decimal_from_f64(value: f64, label: &str) -> Result<Decimal> {
    if !value.is_finite() {
        bail!("invalid live order {label} {value}");
    }
    Decimal::from_str(&format!("{value:.6}"))
        .with_context(|| format!("failed to convert live order {label}={value} to Decimal"))
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
