use axum::{extract::State, response::Html, routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

use crate::config::Settings;
use crate::polymarket::MarketSnapshot;
use crate::snipe::SnipeSignal;

#[derive(Clone, Debug, Default, Serialize)]
pub struct DashboardState {
    pub last_scan_at: Option<String>,
    pub scanned_markets: usize,
    pub candidates: Vec<SnipeSignal>,
    pub watched_markets: Vec<MarketSnapshot>,
    pub last_error: Option<String>,
    pub dry_run: bool,
    pub allow_live_buys: bool,
}

pub type SharedDashboard = Arc<RwLock<DashboardState>>;

pub async fn serve_dashboard(settings: Settings, shared: SharedDashboard) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .layer(CorsLayer::permissive())
        .with_state(shared);

    let addr: SocketAddr = format!("{}:{}", settings.dashboard_host, settings.dashboard_port).parse()?;
    tracing::info!(%addr, "dashboard listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn status(State(shared): State<SharedDashboard>) -> Json<DashboardState> {
    Json(shared.read().await.clone())
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
    table { width: 100%; border-collapse: collapse; overflow: hidden; border-radius: 14px; }
    th, td { text-align: left; padding: 10px; border-bottom: 1px solid #26324d; vertical-align: top; }
    th { color: #94a3b8; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }
    .badge { display: inline-block; padding: 4px 8px; border-radius: 999px; background: #1e293b; font-size: 12px; }
    .hot { background: #713f12; color: #fde68a; }
    .safe { background: #14532d; color: #bbf7d0; }
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
    </section>

    <section class="card">
      <h2>Current candidates</h2>
      <table>
        <thead><tr><th>Market</th><th>Outcome</th><th>Price</th><th>Edge proxy</th><th>TTE</th><th>Stake</th><th>Mode</th></tr></thead>
        <tbody id="rows"><tr><td colspan="7" class="muted">Loading...</td></tr></tbody>
      </table>
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
  document.getElementById('error').textContent = data.last_error || 'None';

  const rows = document.getElementById('rows');
  if (!data.candidates.length) {
    rows.innerHTML = '<tr><td colspan="7" class="muted">No last-minute 5m snipe candidates right now.</td></tr>';
    return;
  }
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
function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;',"'":'&#39;','"':'&quot;'}[c]));
}
refresh();
setInterval(refresh, 3000);
</script>
</body>
</html>"#;
