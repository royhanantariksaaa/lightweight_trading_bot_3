use anyhow::{Context, Result, anyhow, bail};
use polymarket_client_sdk_v2::auth::{LocalSigner, Normal, Signer as _};
use tracing::{error, warn};
use polymarket_client_sdk_v2::clob::types::request::{BalanceAllowanceRequest, OrdersRequest};
use polymarket_client_sdk_v2::clob::types::{AssetType, OrderType, Side, SignatureType};
use polymarket_client_sdk_v2::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk_v2::types::{Address, Decimal, U256};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::config::Settings;
use crate::polymarket::MarketSnapshot;
use crate::snipe::SnipeSignal;

#[derive(Clone, Debug, Default, Serialize)]
pub struct WalletSnapshot {
    pub address: Option<String>,
    pub cash: Option<f64>,
    pub allowance: Option<f64>,
    pub position_value: Option<f64>,
    pub portfolio_value: Option<f64>,
    pub positions_count: usize,
    pub open_orders: Vec<OpenOrderSnapshot>,
    pub updated_at: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenOrderSnapshot {
    pub id: String,
    pub market: String,
    pub outcome: String,
    pub side: String,
    pub price: f64,
    pub original_size: f64,
    pub size_matched: f64,
    pub created_at: String,
}

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
    Sell,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LiveOrderResponse {
    pub success: bool,
    pub order_id: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct CancelLiveOrderResponse {
    pub canceled: bool,
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

pub fn buy_request_from_market(
    settings: &Settings,
    market: &MarketSnapshot,
    outcome_name: &str,
    amount_usd: f64,
) -> Result<LiveOrderRequest> {
    let outcome = market
        .outcomes
        .iter()
        .find(|outcome| outcome.name.eq_ignore_ascii_case(outcome_name))
        .ok_or_else(|| anyhow!("outcome {outcome_name} not found for {}", market.slug))?;
    let token_id = outcome
        .token_id
        .clone()
        .ok_or_else(|| anyhow!("cannot place order without clob token_id"))?;
    let price = outcome
        .best_ask
        .or(outcome.best_bid)
        .unwrap_or(outcome.price);
    guarded_request(
        settings,
        token_id,
        market.slug.clone(),
        outcome.name.clone(),
        LiveSide::Buy,
        price,
        (amount_usd / price).max(0.0),
        Some(market.seconds_to_expiry),
    )
}

pub fn sell_request_from_position(
    settings: &Settings,
    market: &MarketSnapshot,
    outcome_name: &str,
    shares: f64,
) -> Result<LiveOrderRequest> {
    let outcome = market
        .outcomes
        .iter()
        .find(|outcome| outcome.name.eq_ignore_ascii_case(outcome_name))
        .ok_or_else(|| anyhow!("outcome {outcome_name} not found for {}", market.slug))?;
    let token_id = outcome
        .token_id
        .clone()
        .ok_or_else(|| anyhow!("cannot place order without clob token_id"))?;
    let price = outcome
        .best_bid
        .or(outcome.best_ask)
        .unwrap_or(outcome.price);
    guarded_request(
        settings,
        token_id,
        market.slug.clone(),
        outcome.name.clone(),
        LiveSide::Sell,
        price,
        shares,
        None,
    )
}

pub async fn post_live_order(
    settings: &Settings,
    request: &LiveOrderRequest,
) -> Result<LiveOrderResponse> {
    if settings.dry_run {
        bail!("blocked live order: DRY_RUN=true");
    }

    let private_key = settings
        .polymarket_private_key
        .as_deref()
        .context("POLYMARKET_PRIVATE_KEY is required for live trading")?;
    let signer = LocalSigner::from_str(private_key.trim())
        .context("failed to parse POLYMARKET_PRIVATE_KEY")?
        .with_chain_id(Some(settings.polymarket_chain_id));

    let client = authenticated_clob_client(settings, &signer).await?;

    let token_id = U256::from_str(&request.token_id)
        .with_context(|| format!("invalid CLOB token_id={}", request.token_id))?;
    let price = decimal_from_f64(request.price, "price", 2)?;
    let order_type = sdk_order_type(&request.order_type)?;

    let size = decimal_from_f64(request.size, "size", 2)?;

    let response = client
        .limit_order()
        .token_id(token_id)
        .side(sdk_side(&request.side))
        .price(price)
        .size(size)
        .order_type(order_type)
        .build_sign_and_post(&signer)
        .await
        .map_err(|e| {
            error!(
                raw_error = ?e,
                token_id = %request.token_id,
                price = %request.price,
                size = %request.size,
                order_type = %request.order_type,
                "CLOB SDK build_sign_and_post failed — raw error above"
            );
            e
        })
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

pub async fn cancel_live_order(
    settings: &Settings,
    order_id: &str,
) -> Result<CancelLiveOrderResponse> {
    if settings.dry_run {
        bail!("blocked live cancel: DRY_RUN=true");
    }

    let private_key = settings
        .polymarket_private_key
        .as_deref()
        .context("POLYMARKET_PRIVATE_KEY is required for live cancel")?;
    let signer = LocalSigner::from_str(private_key.trim())
        .context("failed to parse POLYMARKET_PRIVATE_KEY")?
        .with_chain_id(Some(settings.polymarket_chain_id));
    let client = authenticated_clob_client(settings, &signer).await?;
    let response = client
        .cancel_order(order_id)
        .await
        .with_context(|| format!("failed to cancel Polymarket order {order_id}"))?;
    let canceled = response.canceled.iter().any(|id| id == order_id);
    let raw = serde_json::json!({
        "canceled": response.canceled,
        "not_canceled": response.not_canceled,
    });
    Ok(CancelLiveOrderResponse { canceled, raw })
}

pub async fn fetch_wallet_snapshot(settings: &Settings) -> WalletSnapshot {
    let updated_at = chrono::Utc::now().to_rfc3339();
    match try_fetch_wallet_snapshot(settings, updated_at.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => WalletSnapshot {
            updated_at,
            error: Some(error.to_string()),
            ..WalletSnapshot::default()
        },
    }
}

async fn try_fetch_wallet_snapshot(
    settings: &Settings,
    updated_at: String,
) -> Result<WalletSnapshot> {
    let private_key = match settings.polymarket_private_key.as_deref() {
        Some(value) if !value.trim().is_empty() => value.to_string(),
        _ => {
            return Ok(WalletSnapshot {
                updated_at,
                error: Some("Polymarket private key not configured".to_string()),
                ..WalletSnapshot::default()
            });
        }
    };

    let signer = LocalSigner::from_str(private_key.trim())
        .context("failed to parse POLYMARKET_PRIVATE_KEY")?
        .with_chain_id(Some(settings.polymarket_chain_id));
    let client = authenticated_clob_client(settings, &signer).await?;
    let address = wallet_address_for_profile(settings, client.address().to_string());
    let signature_type =
        sdk_signature_type(settings.polymarket_signature_type)?.unwrap_or(SignatureType::Eoa);
    let balance = client
        .balance_allowance(
            BalanceAllowanceRequest::builder()
                .asset_type(AssetType::Collateral)
                .signature_type(signature_type)
                .build(),
        )
        .await
        .context("failed to fetch Polymarket wallet balance")?;
    let cash = decimal_to_f64(&balance.balance);
    let allowance = balance
        .allowances
        .values()
        .filter_map(|value| value.parse::<f64>().ok())
        .max_by(|left, right| left.total_cmp(right));
    let positions = fetch_position_value(&address).await.unwrap_or_default();
    let open_orders = fetch_open_orders(&client).await.unwrap_or_default();

    Ok(WalletSnapshot {
        address: Some(address),
        cash: Some(cash),
        allowance,
        position_value: Some(positions.position_value),
        portfolio_value: Some(cash + positions.position_value),
        positions_count: positions.positions_count,
        open_orders,
        updated_at,
        error: None,
    })
}

async fn authenticated_clob_client<S: polymarket_client_sdk_v2::auth::Signer>(
    settings: &Settings,
    signer: &S,
) -> Result<
    polymarket_client_sdk_v2::clob::Client<
        polymarket_client_sdk_v2::auth::state::Authenticated<Normal>,
    >,
> {
    let mut auth = ClobClient::new(&settings.polymarket_clob_host, ClobConfig::default())
        .context("failed to create Polymarket CLOB SDK client")?
        .authentication_builder(signer);

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

    auth.authenticate()
        .await
        .context("failed to authenticate Polymarket CLOB SDK client")
}

fn wallet_address_for_profile(settings: &Settings, signer_address: String) -> String {
    let funder = settings.polymarket_funder_address.trim();
    if funder.is_empty() {
        signer_address
    } else {
        funder.to_string()
    }
}

#[derive(Default)]
struct PositionTotals {
    position_value: f64,
    positions_count: usize,
}

async fn fetch_position_value(address: &str) -> Result<PositionTotals> {
    let positions = reqwest::Client::new()
        .get("https://data-api.polymarket.com/positions")
        .query(&[("user", address), ("limit", "500"), ("sizeThreshold", "0")])
        .send()
        .await
        .context("failed to fetch Polymarket positions")?
        .error_for_status()
        .context("Polymarket positions returned error status")?
        .json::<Vec<serde_json::Value>>()
        .await
        .context("failed to decode Polymarket positions")?;

    Ok(PositionTotals {
        position_value: positions
            .iter()
            .filter_map(|position| {
                position
                    .get("currentValue")
                    .and_then(serde_json::Value::as_f64)
            })
            .sum(),
        positions_count: positions.len(),
    })
}

async fn fetch_open_orders<S: polymarket_client_sdk_v2::auth::Kind>(
    client: &polymarket_client_sdk_v2::clob::Client<
        polymarket_client_sdk_v2::auth::state::Authenticated<S>,
    >,
) -> Result<Vec<OpenOrderSnapshot>> {
    let page = client
        .orders(&OrdersRequest::builder().build(), None)
        .await
        .context("failed to fetch open orders")?;
    Ok(page
        .data
        .into_iter()
        .take(20)
        .map(|order| OpenOrderSnapshot {
            id: order.id,
            market: order.market.to_string(),
            outcome: order.outcome,
            side: format!("{:?}", order.side),
            price: decimal_to_f64(&order.price),
            original_size: decimal_to_f64(&order.original_size),
            size_matched: decimal_to_f64(&order.size_matched),
            created_at: order.created_at.to_rfc3339(),
        })
        .collect())
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
        LiveSide::Sell => Side::Sell,
    }
}

pub async fn redeem_winnings(settings: &Settings) -> Result<()> {
    if settings.dry_run || !settings.auto_redeem {
        return Ok(());
    }

    let _private_key = settings
        .polymarket_private_key
        .as_deref()
        .context("POLYMARKET_PRIVATE_KEY required for redeem")?;
    
    // TODO: The 0.6.0-canary SDK seems to have moved the redeem method or uses a different builder.
    // client.redeem().await.context("failed to redeem winnings")?;
    warn!("Auto-redeem is not yet fully implemented for this SDK version");
    
    Ok(())
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

fn decimal_from_f64(value: f64, label: &str, decimals: usize) -> Result<Decimal> {
    if !value.is_finite() {
        bail!("invalid live order {label} {value}");
    }
    // We format to the max decimals but then trim the string if it's something like "0.740" 
    // to avoid strict tick size validation errors on some markets.
    let s = format!("{value:.decimals$}");
    let trimmed = if s.contains('.') {
        let t = s.trim_end_matches('0').trim_end_matches('.');
        if t.is_empty() { "0" } else { t }
    } else {
        &s
    };
    
    Decimal::from_str(trimmed)
        .with_context(|| format!("failed to convert live order {label}={value} to Decimal"))
}

fn decimal_to_f64(value: &Decimal) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}

pub fn hide_stale_display_orders(settings: &Settings, wallet: &mut WalletSnapshot) {
    wallet
        .open_orders
        .retain(|order| !is_stale_display_order(order, settings.maker_order_ttl_ms));
}

fn is_stale_display_order(order: &OpenOrderSnapshot, ttl_ms: i64) -> bool {
    let cutoff = chrono::Utc::now() - chrono::Duration::milliseconds(ttl_ms.max(5_000) * 2);
    chrono::DateTime::parse_from_rfc3339(&order.created_at)
        .map(|created_at| created_at.with_timezone(&chrono::Utc) <= cutoff)
        .unwrap_or(false)
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

    let mut size = size;
    let mut amount_usd = price * size;

    let is_fak = settings.live_order_type.trim().eq_ignore_ascii_case("FAK") ||
        settings.live_order_type.trim().eq_ignore_ascii_case("FOK");

    // For FAK/FOK buys, check LIVE_MAX against the original intended amount BEFORE
    // decimal compliance adjustment. The compliance bump is a mandatory API requirement
    // and should not block orders that were within the user's intended limit.
    if is_fak && matches!(side, LiveSide::Buy) && amount_usd > settings.live_max_order_usd {
        bail!(
            "blocked live order: ${:.2} exceeds LIVE_MAX_ORDER_USD={:.2}",
            amount_usd,
            settings.live_max_order_usd
        );
    }
    if is_fak && matches!(side, LiveSide::Buy) {
        // Polymarket CLOB API rules for marketable (FAK/FOK) BUY orders:
        //   - maker_amount (USDC = size * price) must have at most 2 decimal places
        //   - maker_amount must be >= $1.00 minimum
        // Using integer cent arithmetic to avoid float rounding issues.
        //
        // Phase 1: walk DOWN from raw size to find largest size where (size*price) % $0.01 == 0
        // Phase 2: if Phase 1 produced maker_usdc < $1, walk UP to next clean size >= $1
        let price_cents = (price * 100.0).round() as i64;
        if price_cents <= 0 {
            bail!("blocked live order: price rounds to zero ({price})");
        }
        let mut size_cents = (size * 100.0).floor() as i64;
        if size_cents <= 0 {
            bail!("blocked live order: size rounds to zero ({size})");
        }

        // Phase 1: walk down until product is 2-decimal-clean
        while (size_cents * price_cents) % 100 != 0 {
            size_cents -= 1;
            if size_cents <= 0 {
                bail!("blocked live order: no 2-decimal-clean product possible for price={price}");
            }
        }

        // Phase 2: if maker_usdc < $1 minimum, bump up to next valid clean size
        // $1.00 in micro-cents = 10_000; size_cents * price_cents gives micro-cents
        if size_cents * price_cents < 10_000 {
            // Smallest size_cents that brings maker_usdc to >= $1.00
            size_cents = (10_000i64 + price_cents - 1) / price_cents;
            // Walk up until the product is also 2-decimal-clean
            while (size_cents * price_cents) % 100 != 0 {
                size_cents += 1;
            }
        }

        size = size_cents as f64 / 100.0;
        amount_usd = (size_cents * price_cents) as f64 / 10_000.0;
    }
    // Keep the legacy 5-share bump for resting order types; FAK is allowed to submit smaller size.
    if !is_fak && size < 5.0 {
        let bumped_amount = price * 5.0;
        if bumped_amount <= settings.live_max_order_usd {
            size = 5.0;
            amount_usd = bumped_amount;
        } else {
            bail!(
                "blocked live order: size {:.2} is below minimum 5.0 for {} and bumping to ${:.2} would exceed LIVE_MAX_ORDER_USD={:.2}",
                size,
                settings.live_order_type,
                bumped_amount,
                settings.live_max_order_usd
            );
        }
    }

    // For non-FAK orders, check the final amount (already checked pre-compliance for FAK above).
    if !is_fak && amount_usd > settings.live_max_order_usd {
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
