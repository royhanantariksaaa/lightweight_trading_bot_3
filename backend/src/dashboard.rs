use axum::{
    Json, Router,
    extract::{Path, State},
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, fs, net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use crate::config::{RuntimeSettingsUpdate, Settings};
use crate::live::{
    WalletSnapshot, buy_request_from_market, fetch_wallet_snapshot, hide_stale_display_orders,
    post_live_order,
};
use crate::polymarket::MarketSnapshot;
use crate::snipe::SnipeSignal;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ActivityLog {
    pub id: String,
    pub timestamp_ms: u64,
    pub level: String, // "info", "warn", "success", "whale"
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DashboardState {
    pub last_scan_at: Option<String>,
    pub scanned_markets: usize,
    pub candidates: Vec<SnipeSignal>,
    pub watched_markets: Vec<MarketSnapshot>,
    pub latest_whale_signal: Option<WhaleSignal>,
    pub whale_signals: Vec<WhaleSignal>,
    pub binance_books: std::collections::HashMap<String, BinanceBookInfo>,
    pub global_activity_score: f64,
    pub last_snipe: Option<SnipeSignal>,
    pub last_error: Option<String>,
    pub dry_run: bool,
    pub allow_live_buys: bool,
    pub allow_live_sells: bool,
    pub live_max_order_usd: f64,
    pub live_order_type: String,
    pub snipe_max_position_usd: f64,
    pub wallet_configured: bool,
    pub funder_address: String,
    pub signature_type: Option<u8>,
    pub enable_llm_market_reports: bool,
    pub llm_api_base: String,
    pub llm_api_key_configured: bool,
    pub llm_model: String,
    pub llm_report_dir: String,
    pub llm_code_patch_mode: String,
    pub enable_balanced_recovery: bool,
    pub enable_same_side_avg_down: bool,
    pub enable_opposite_side_hedge: bool,
    pub enable_resolution_locked_hedge: bool,
    pub max_market_recovery_cost_usd: f64,
    pub recovery_max_adds_per_market: usize,
    pub recovery_min_price_improvement_pct: f64,
    pub phase1_min_hold_ms: i64,
    pub exit_confirmation_ticks: usize,
    pub exit_block_if_book_support: bool,
    pub disable_phase1_price_cap: bool,
    pub enable_recovery_unwind: bool,
    pub recovery_unwind_profit_pct: f64,
    pub recovery_trailing_drawdown_pct: f64,
    pub recovery_partial_sell_pct: f64,
    pub wallet: WalletSnapshot,
    pub active_symbols: Vec<String>,
    pub activities: VecDeque<ActivityLog>,
}

impl DashboardState {
    pub fn push_activity(&mut self, level: &str, message: &str, detail: Option<&str>) {
        let log = ActivityLog {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            level: level.to_string(),
            message: message.to_string(),
            detail: detail.map(|s| s.to_string()),
        };
        self.activities.push_front(log);
        if self.activities.len() > 50 {
            self.activities.pop_back();
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WhaleWallInfo {
    pub price: f64,
    pub notional_usd: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WhaleSignal {
    pub timestamp: String,
    pub market: String,
    pub symbol: String,
    pub side: String,
    pub tier: String,
    pub trade_price: f64,
    pub quantity: f64,
    pub notional_usd: f64,
    pub target_price: f64,
    pub required_notional: f64,
    pub signal: String,
    pub imbalance_pct: f64,
    pub bid_wall: Option<WhaleWallInfo>,
    pub ask_wall: Option<WhaleWallInfo>,
    pub need_up_10: f64,
    pub need_down_10: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BinanceBookInfo {
    pub symbol: String,
    pub imbalance_pct: f64,
    pub bid_wall: Option<WhaleWallInfo>,
    pub ask_wall: Option<WhaleWallInfo>,
    pub need_up_10: f64,
    pub need_down_10: f64,
    #[serde(default)]
    pub imbalance_history: Vec<ImbalanceSample>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImbalanceSample {
    pub timestamp_ms: i64,
    pub imbalance_pct: f64,
    pub need_up_10: f64,
    pub need_down_10: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManualOrderRequest {
    pub market_slug: String,
    pub outcome: String,
    pub amount_usd: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ManualOrderResponse {
    pub accepted: bool,
    pub live: bool,
    pub message: String,
    pub order_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LlmReportListItem {
    pub id: String,
    pub generated_at: Option<String>,
    pub market_slug: Option<String>,
    pub question: Option<String>,
    pub has_response: bool,
    pub has_code_patch: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LlmReportDetail {
    pub id: String,
    pub report: serde_json::Value,
    pub llm_response: Option<serde_json::Value>,
    pub llm_response_raw: Option<String>,
    pub code_patch_unified_diff: Option<String>,
}

#[derive(Clone)]
struct DashboardContext {
    settings: Arc<RwLock<Settings>>,
    shared: SharedDashboard,
}

pub type SharedDashboard = Arc<RwLock<DashboardState>>;

pub async fn serve_dashboard(
    settings: Arc<RwLock<Settings>>,
    shared: SharedDashboard,
) -> anyhow::Result<()> {
    let context = DashboardContext {
        settings: settings.clone(),
        shared,
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/settings", post(update_settings))
        .route("/api/manual-order", post(manual_order))
        .route("/api/llm-reports", get(list_llm_reports))
        .route("/api/llm-reports/:id", get(get_llm_report))
        .route("/api/hermes-reports", get(list_llm_reports))
        .route("/api/hermes-reports/:id", get(get_llm_report))
        .layer(CorsLayer::permissive())
        .with_state(context);

    let bind_settings = settings.read().await.clone();
    let addr: SocketAddr = format!(
        "{}:{}",
        bind_settings.dashboard_host, bind_settings.dashboard_port
    )
    .parse()?;
    tracing::info!(%addr, "dashboard listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn status(State(context): State<DashboardContext>) -> Json<DashboardState> {
    Json(context.shared.read().await.clone())
}

async fn list_llm_reports(State(context): State<DashboardContext>) -> Json<Vec<LlmReportListItem>> {
    let settings = context.settings.read().await.clone();
    let mut items = fs::read_dir(&settings.hermes_report_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !file_name.ends_with("-report.json") {
                return None;
            }
            let id = file_name.trim_end_matches("-report.json").to_string();
            let report = fs::read_to_string(entry.path())
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
            let response_path = settings
                .hermes_report_dir
                .join(format!("{id}-llm-response.json"));
            let response_raw = fs::read_to_string(&response_path).ok();
            Some(LlmReportListItem {
                id,
                generated_at: report
                    .as_ref()
                    .and_then(|value| value.get("generated_at"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
                market_slug: report
                    .as_ref()
                    .and_then(|value| value.pointer("/observed_market/slug"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
                question: report
                    .as_ref()
                    .and_then(|value| value.pointer("/observed_market/question"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
                has_response: response_raw.is_some(),
                has_code_patch: response_raw
                    .as_deref()
                    .and_then(extract_code_patch)
                    .map(|patch| !patch.trim().is_empty())
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.id.cmp(&a.id));
    Json(items)
}

async fn get_llm_report(
    State(context): State<DashboardContext>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    if !is_safe_report_id(&id) {
        return Json(serde_json::json!({ "ok": false, "error": "invalid report id" }));
    }
    let settings = context.settings.read().await.clone();
    let report_path = settings.hermes_report_dir.join(format!("{id}-report.json"));
    let report = match fs::read_to_string(&report_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
    {
        Some(report) => report,
        None => return Json(serde_json::json!({ "ok": false, "error": "report not found" })),
    };
    let response_raw = fs::read_to_string(
        settings
            .hermes_report_dir
            .join(format!("{id}-llm-response.json")),
    )
    .ok();
    let response = response_raw
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let detail = LlmReportDetail {
        id,
        report,
        llm_response: response,
        llm_response_raw: response_raw.clone(),
        code_patch_unified_diff: response_raw.as_deref().and_then(extract_code_patch),
    };
    Json(serde_json::json!({ "ok": true, "report": detail }))
}

async fn manual_order(
    State(context): State<DashboardContext>,
    Json(request): Json<ManualOrderRequest>,
) -> Json<ManualOrderResponse> {
    let settings = context.settings.read().await.clone();
    let market = {
        let dashboard = context.shared.read().await;
        dashboard
            .watched_markets
            .iter()
            .find(|market| market.slug == request.market_slug)
            .cloned()
    };
    let Some(market) = market else {
        return Json(ManualOrderResponse {
            accepted: false,
            live: false,
            message: "Market is no longer active in the scanner.".to_string(),
            order_id: None,
        });
    };

    let order =
        match buy_request_from_market(&settings, &market, &request.outcome, request.amount_usd) {
            Ok(order) => order,
            Err(error) => {
                return Json(ManualOrderResponse {
                    accepted: false,
                    live: false,
                    message: format!("{:#}", error),
                    order_id: None,
                });
            }
        };

    if settings.dry_run || !settings.allow_live_buys {
        return Json(ManualOrderResponse {
            accepted: true,
            live: false,
            message: format!(
                "Paper order prepared: {} {} for ${:.2} at {:.3}.",
                order.market_slug, order.outcome, order.amount_usd, order.price
            ),
            order_id: None,
        });
    }

    match post_live_order(&settings, &order).await {
        Ok(response) if response.success => Json(ManualOrderResponse {
            accepted: true,
            live: true,
            message: "Live order accepted by Polymarket.".to_string(),
            order_id: response.order_id,
        }),
        Ok(response) => Json(ManualOrderResponse {
            accepted: false,
            live: true,
            message: response.raw.to_string(),
            order_id: response.order_id,
        }),
        Err(error) => Json(ManualOrderResponse {
            accepted: false,
            live: true,
            message: format!("{:#}", error),
            order_id: None,
        }),
    }
}

fn is_safe_report_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn extract_code_patch(raw: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("code_patch_unified_diff")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
}

async fn update_settings(
    State(context): State<DashboardContext>,
    Json(update): Json<RuntimeSettingsUpdate>,
) -> Json<serde_json::Value> {
    let mut settings = context.settings.write().await;
    match settings.apply_runtime_update(update) {
        Ok(()) => {
            let mut wallet = fetch_wallet_snapshot(&settings).await;
            hide_stale_display_orders(&settings, &mut wallet);
            let mut dashboard = context.shared.write().await;
            dashboard.dry_run = settings.dry_run;
            dashboard.allow_live_buys = settings.allow_live_buys;
            dashboard.allow_live_sells = settings.allow_live_sells;
            dashboard.live_max_order_usd = settings.live_max_order_usd;
            dashboard.live_order_type = settings.live_order_type.clone();
            dashboard.snipe_max_position_usd = settings.snipe_max_position_usd;
            dashboard.wallet_configured = settings.polymarket_private_key.is_some();
            dashboard.funder_address = settings.polymarket_funder_address.clone();
            dashboard.signature_type = settings.polymarket_signature_type;
            dashboard.enable_llm_market_reports = settings.enable_llm_market_reports;
            dashboard.llm_api_base = settings.llm_api_base.clone();
            dashboard.llm_api_key_configured = settings.llm_api_key.is_some();
            dashboard.llm_model = settings.llm_model.clone();
            dashboard.llm_report_dir = settings.hermes_report_dir.display().to_string();
            dashboard.llm_code_patch_mode = settings.llm_code_patch_mode.clone();
            dashboard.enable_balanced_recovery = settings.enable_balanced_recovery;
            dashboard.enable_same_side_avg_down = settings.enable_same_side_avg_down;
            dashboard.enable_opposite_side_hedge = settings.enable_opposite_side_hedge;
            dashboard.enable_resolution_locked_hedge = settings.enable_resolution_locked_hedge;
            dashboard.max_market_recovery_cost_usd = settings.max_market_recovery_cost_usd;
            dashboard.recovery_max_adds_per_market = settings.recovery_max_adds_per_market;
            dashboard.recovery_min_price_improvement_pct =
                settings.recovery_min_price_improvement_pct;
            dashboard.phase1_min_hold_ms = settings.phase1_min_hold_ms;
            dashboard.exit_confirmation_ticks = settings.exit_confirmation_ticks;
            dashboard.exit_block_if_book_support = settings.exit_block_if_book_support;
            dashboard.disable_phase1_price_cap = settings.disable_phase1_price_cap;
            dashboard.enable_recovery_unwind = settings.enable_recovery_unwind;
            dashboard.recovery_unwind_profit_pct = settings.recovery_unwind_profit_pct;
            dashboard.recovery_trailing_drawdown_pct = settings.recovery_trailing_drawdown_pct;
            dashboard.recovery_partial_sell_pct = settings.recovery_partial_sell_pct;
            dashboard.wallet = wallet;
            Json(serde_json::json!({ "ok": true }))
        }
        Err(error) => Json(serde_json::json!({
            "ok": false,
            "error": error.to_string()
        })),
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Polymarket 5m Snipe Dashboard</title>
  <style>
    body { font-family: system-ui, -apple-system, Segoe UI, sans-serif; margin: 0; background: #0b1020; color: #edf2ff; }
    header { padding: 24px; border-bottom: 1px solid #26324d; background: #111936; }
    main { padding: 24px; display: grid; gap: 16px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 16px; }
    .card { background: #141d3a; border: 1px solid #26324d; border-radius: 14px; padding: 16px; box-shadow: 0 10px 25px rgba(0,0,0,.18); }
    .muted { color: #94a3b8; font-size: 13px; }
    .big { font-size: 28px; font-weight: 800; margin-top: 6px; }
    .metric { display: grid; gap: 4px; }
    .metric .label { color: #94a3b8; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }
    .metric .value { font-size: 18px; font-weight: 750; }
    .signal-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; margin-top: 12px; }
    .signal-panel { border: 1px solid #26324d; background: #101831; border-radius: 8px; padding: 14px; }
    .table-wrap { overflow-x: auto; }
    table { width: 100%; border-collapse: collapse; overflow: hidden; border-radius: 14px; }
    .wide-table { min-width: 1120px; }
    th, td { text-align: left; padding: 10px; border-bottom: 1px solid #26324d; vertical-align: top; }
    th { color: #94a3b8; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }
    .badge { display: inline-block; padding: 4px 8px; border-radius: 999px; background: #1e293b; font-size: 12px; }
    .hot { background: #713f12; color: #fde68a; }
    .safe { background: #14532d; color: #bbf7d0; }
    .controls { display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 12px; align-items: end; }
    label { display: grid; gap: 6px; color: #94a3b8; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }
    input, select, button { border-radius: 10px; border: 1px solid #334155; background: #0f172a; color: #edf2ff; padding: 10px; }
    button { cursor: pointer; font-weight: 800; background: #1d4ed8; }
    button.secondary { background: #334155; }
    a { color: #93c5fd; }
  </style>
</head>
<body>
  <header>
    <h1>Polymarket Last-1-Minute 5m Snipe</h1>
    <div class="muted">Dry-run first. Live buys only when explicitly enabled in env.</div>
  </header>
  <main>
    <section class="grid">
      <div class="card"><div class="muted">Last scan</div><div class="big" id="lastScan">-</div></div>
      <div class="card"><div class="muted">Markets scanned</div><div class="big" id="scanned">0</div></div>
      <div class="card"><div class="muted">Snipe candidates</div><div class="big" id="candidates">0</div></div>
      <div class="card"><div class="muted">Mode</div><div class="big" id="mode">DRY</div></div>
      <div class="card"><div class="muted">Recovery</div><div class="big" id="recoveryMode">-</div><div class="muted" id="recoveryDetail">-</div></div>
    </section>

    <section class="card">
      <h2>Recovery / Exit Controls</h2>
      <div class="controls">
        <label>Recovery master
          <select id="ctlRecovery"><option value="true">ON</option><option value="false">OFF</option></select>
        </label>
        <label>Same-side avg-down
          <select id="ctlSameSideAvgDown"><option value="true">ON</option><option value="false">OFF</option></select>
        </label>
        <label>Opposite hedge
          <select id="ctlOppositeHedge"><option value="false">OFF</option><option value="true">ON</option></select>
        </label>
        <label>Resolution lock hedge
          <select id="ctlResolutionLock"><option value="false">OFF</option><option value="true">ON</option></select>
        </label>
        <label>Recovery cap $
          <input id="ctlRecoveryCap" type="number" step="0.01" min="1" max="5" />
        </label>
        <label>Max adds
          <input id="ctlRecoveryAdds" type="number" step="1" min="0" max="5" />
        </label>
        <label>Min avg-down drop
          <input id="ctlRecoveryDrop" type="number" step="0.01" min="0" max="0.5" />
        </label>
        <label>Min hold ms
          <input id="ctlHoldMs" type="number" step="1000" min="0" />
        </label>
        <label>Exit ticks
          <input id="ctlExitTicks" type="number" step="1" min="1" max="10" />
        </label>
        <label>Block exit if book supports
          <select id="ctlExitBlock"><option value="true">ON</option><option value="false">OFF</option></select>
        </label>
        <label>Disable Phase1 price cap
          <select id="ctlDisablePriceCap"><option value="true">ON</option><option value="false">OFF</option></select>
        </label>
        <label>Recovery unwind
          <select id="ctlRecoveryUnwind"><option value="true">ON</option><option value="false">OFF</option></select>
        </label>
        <label>Unwind profit pct
          <input id="ctlUnwindProfit" type="number" step="0.01" min="0" max="1" />
        </label>
        <label>Trail drawdown pct
          <input id="ctlTrailDrawdown" type="number" step="0.01" min="0" max="1" />
        </label>
        <label>Partial sell pct
          <input id="ctlPartialSell" type="number" step="0.05" min="0.1" max="1" />
        </label>
        <button onclick="saveRecoverySettings()">Save Controls</button>
        <button class="secondary" onclick="refresh()">Reload</button>
      </div>
      <div id="settingsMsg" class="muted" style="margin-top:10px"></div>
    </section>

    <section class="card">
      <h2>Current candidates</h2>
      <table>
        <thead><tr><th>Market</th><th>Outcome</th><th>Price</th><th>Edge proxy</th><th>TTE</th><th>Stake</th><th>Mode</th></tr></thead>
        <tbody id="rows"><tr><td colspan="7" class="muted">Loading...</td></tr></tbody>
      </table>
    </section>

    <section class="card">
      <h2>Whale signals</h2>
      <div id="latestWhale" class="muted">Loading...</div>
      <div class="table-wrap">
        <table class="wide-table">
          <thead><tr><th>Time</th><th>Market</th><th>Flow</th><th>Trade</th><th>Target</th><th>Recovery / Pullback</th><th>Book pressure</th><th>Walls</th><th>Move liquidity</th></tr></thead>
          <tbody id="whaleRows"><tr><td colspan="9" class="muted">Loading...</td></tr></tbody>
        </table>
      </div>
    </section>

    <section class="card">
      <h2>Last error</h2>
      <div id="error" class="muted">None</div>
    </section>
  </main>
<script>
async function refresh() {
  const res = await fetch('/api/status');
  const data = await res.json();
  document.getElementById('lastScan').textContent = data.last_scan_at || '-';
  document.getElementById('scanned').textContent = data.scanned_markets;
  document.getElementById('candidates').textContent = data.candidates.length;
  document.getElementById('mode').textContent = data.dry_run || !data.allow_live_buys ? 'DRY' : 'LIVE';
  document.getElementById('recoveryMode').textContent = data.enable_balanced_recovery ? 'ON' : 'OFF';
  document.getElementById('recoveryDetail').textContent = `same-side ${data.enable_same_side_avg_down ? 'ON' : 'OFF'} | opposite hedge ${data.enable_opposite_side_hedge ? 'ON' : 'OFF'} | resolution lock ${data.enable_resolution_locked_hedge ? 'ON' : 'OFF'} | cap $${Number(data.max_market_recovery_cost_usd || 0).toFixed(2)} | adds ${data.recovery_max_adds_per_market || 0} | hold ${Math.round((data.phase1_min_hold_ms || 0)/1000)}s | exit ticks ${data.exit_confirmation_ticks || 1} | unwind ${data.enable_recovery_unwind ? 'ON' : 'OFF'}`;
  hydrateControls(data);
  window.latestStatus = data;
  document.getElementById('error').textContent = data.last_error || 'None';

  const rows = document.getElementById('rows');
  if (!data.candidates.length) {
    rows.innerHTML = '<tr><td colspan="7" class="muted">No last-minute 5m snipe candidates right now.</td></tr>';
  } else {
    rows.innerHTML = data.candidates.map(c => `
      <tr>
        <td><strong>${escapeHtml(c.market_slug)}</strong><div class="muted">${escapeHtml(c.question)}</div></td>
        <td>${escapeHtml(c.outcome)}</td>
        <td>${c.price.toFixed(3)}</td>
        <td>${c.expected_edge.toFixed(3)}</td>
        <td>${c.seconds_to_expiry}s</td>
        <td>$${c.stake_usd.toFixed(2)}</td>
        <td><span class="badge ${c.dry_run ? 'safe' : 'hot'}">${c.dry_run ? 'DRY RUN' : 'LIVE BUY ENABLED'}</span></td>
      </tr>`).join('');
  }

  const whaleRows = document.getElementById('whaleRows');
  const latestWhale = document.getElementById('latestWhale');
  latestWhale.innerHTML = renderLatestWhale(data.latest_whale_signal);

  if (!data.whale_signals.length) {
    whaleRows.innerHTML = '<tr><td colspan="9" class="muted">No whale signals yet.</td></tr>';
  } else {
    whaleRows.innerHTML = data.whale_signals.map(w => `
      <tr>
        <td>${escapeHtml(w.timestamp)}</td>
        <td><strong>${escapeHtml(w.symbol)}</strong><div class="muted">${escapeHtml(w.market)}</div></td>
        <td><span class="badge ${w.side === 'BUY' ? 'safe' : 'hot'}">${escapeHtml(w.side)}</span><div class="muted">${escapeHtml(w.tier)}</div></td>
        <td>$${money(w.notional_usd)}<div class="muted">qty ${num(w.quantity, 6)} @ ${price(w.trade_price)}</div></td>
        <td>${price(w.target_price)}<div class="muted">distance ${price(w.target_price - w.trade_price)}</div></td>
        <td><span class="badge hot">${escapeHtml(w.signal)}</span><div class="muted">required $${money(w.required_notional)}</div></td>
        <td>${num(w.imbalance_pct, 1)}% imbalance</td>
        <td>${wallText('Bid', w.bid_wall)}<div class="muted">${wallText('Ask', w.ask_wall)}</div></td>
        <td>up10 $${money(w.need_up_10)}<div class="muted">down10 $${money(w.need_down_10)}</div></td>
      </tr>`).join('');
  }
}

function renderLatestWhale(w) {
  if (!w) return '<div class="muted">No latest whale signal yet.</div>';
  return `
    <div class="signal-panel">
      <div class="muted">${escapeHtml(w.timestamp)} · ${escapeHtml(w.market)}</div>
      <div class="signal-grid">
        ${metric('Symbol', escapeHtml(w.symbol))}
        ${metric('Side / Tier', `<span class="badge ${w.side === 'BUY' ? 'safe' : 'hot'}">${escapeHtml(w.side)}</span> ${escapeHtml(w.tier)}`)}
        ${metric('Signal', `<span class="badge hot">${escapeHtml(w.signal)}</span>`)}
        ${metric('Trade Price', price(w.trade_price))}
        ${metric('Quantity', num(w.quantity, 6))}
        ${metric('Notional', '$' + money(w.notional_usd))}
        ${metric('Target Price', price(w.target_price))}
        ${metric('Required Flow', '$' + money(w.required_notional))}
        ${metric('Book Imbalance', num(w.imbalance_pct, 1) + '%')}
        ${metric('Bid Wall', wallValue(w.bid_wall))}
        ${metric('Ask Wall', wallValue(w.ask_wall))}
        ${metric('Move Liquidity', `up10 $${money(w.need_up_10)} / down10 $${money(w.need_down_10)}`)}
      </div>
    </div>`;
}


function hydrateControls(data) {
  setValue('ctlRecovery', String(!!data.enable_balanced_recovery));
  setValue('ctlSameSideAvgDown', String(!!data.enable_same_side_avg_down));
  setValue('ctlOppositeHedge', String(!!data.enable_opposite_side_hedge));
  setValue('ctlResolutionLock', String(!!data.enable_resolution_locked_hedge));
  setValue('ctlRecoveryCap', Number(data.max_market_recovery_cost_usd || 0).toFixed(2));
  setValue('ctlRecoveryAdds', data.recovery_max_adds_per_market || 0);
  setValue('ctlRecoveryDrop', data.recovery_min_price_improvement_pct || 0.12);
  setValue('ctlHoldMs', data.phase1_min_hold_ms || 0);
  setValue('ctlExitTicks', data.exit_confirmation_ticks || 1);
  setValue('ctlExitBlock', String(!!data.exit_block_if_book_support));
  setValue('ctlDisablePriceCap', String(!!data.disable_phase1_price_cap));
  setValue('ctlRecoveryUnwind', String(!!data.enable_recovery_unwind));
  setValue('ctlUnwindProfit', data.recovery_unwind_profit_pct || 0.04);
  setValue('ctlTrailDrawdown', data.recovery_trailing_drawdown_pct || 0.03);
  setValue('ctlPartialSell', data.recovery_partial_sell_pct || 0.50);
}
function setValue(id, value) {
  const el = document.getElementById(id);
  if (el && document.activeElement !== el) el.value = value;
}
async function saveRecoverySettings() {
  const data = window.latestStatus || await (await fetch('/api/status')).json();
  const payload = {
    dry_run: !!data.dry_run,
    allow_live_buys: !!data.allow_live_buys,
    allow_live_sells: !!data.allow_live_sells,
    live_max_order_usd: Number(data.live_max_order_usd || 1),
    live_order_type: data.live_order_type || 'FAK',
    snipe_max_position_usd: Number(data.snipe_max_position_usd || 1),
    active_symbols: (data.active_symbols || []).join(','),
    funder_address: data.funder_address || '',
    signature_type: data.signature_type ?? null,
    private_key: null,
    enable_llm_market_reports: !!data.enable_llm_market_reports,
    llm_api_base: data.llm_api_base || '',
    llm_api_key: null,
    llm_model: data.llm_model || '',
    llm_report_dir: data.llm_report_dir || '',
    llm_code_patch_mode: data.llm_code_patch_mode || 'proposal_only',
    enable_balanced_recovery: document.getElementById('ctlRecovery').value === 'true',
    enable_same_side_avg_down: document.getElementById('ctlSameSideAvgDown').value === 'true',
    enable_opposite_side_hedge: document.getElementById('ctlOppositeHedge').value === 'true',
    enable_resolution_locked_hedge: document.getElementById('ctlResolutionLock').value === 'true',
    max_market_recovery_cost_usd: Number(document.getElementById('ctlRecoveryCap').value || 2),
    recovery_max_adds_per_market: Number(document.getElementById('ctlRecoveryAdds').value || 1),
    recovery_min_price_improvement_pct: Number(document.getElementById('ctlRecoveryDrop').value || 0.12),
    phase1_min_hold_ms: Number(document.getElementById('ctlHoldMs').value || 15000),
    exit_confirmation_ticks: Number(document.getElementById('ctlExitTicks').value || 3),
    exit_block_if_book_support: document.getElementById('ctlExitBlock').value === 'true',
    disable_phase1_price_cap: document.getElementById('ctlDisablePriceCap').value === 'true',
    enable_recovery_unwind: document.getElementById('ctlRecoveryUnwind').value === 'true',
    recovery_unwind_profit_pct: Number(document.getElementById('ctlUnwindProfit').value || 0.04),
    recovery_trailing_drawdown_pct: Number(document.getElementById('ctlTrailDrawdown').value || 0.03),
    recovery_partial_sell_pct: Number(document.getElementById('ctlPartialSell').value || 0.50)
  };
  const res = await fetch('/api/settings', {method:'POST', headers:{'content-type':'application/json'}, body: JSON.stringify(payload)});
  const out = await res.json();
  document.getElementById('settingsMsg').textContent = out.ok ? 'Saved. Next scan uses updated settings.' : ('Save failed: ' + out.error);
  await refresh();
}

function metric(label, value) {
  return `<div class="metric"><div class="label">${label}</div><div class="value">${value}</div></div>`;
}

function wallValue(wall) {
  return wall ? `$${money(wall.notional_usd)} @ ${price(wall.price)}` : 'none';
}

function wallText(label, wall) {
  return `${label}: ${wallValue(wall)}`;
}

function money(value) {
  return Number(value || 0).toLocaleString(undefined, { maximumFractionDigits: 0 });
}

function price(value) {
  return Number(value || 0).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function num(value, digits) {
  return Number(value || 0).toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}
function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;',"'":'&#39;','"':'&quot;'}[c]));
}
refresh();
setInterval(refresh, 3000);
</script>
</body>
</html>"#;
