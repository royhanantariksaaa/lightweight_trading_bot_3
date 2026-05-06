use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};
use tracing::warn;

use crate::config::Settings;
use crate::dashboard::{BinanceBookInfo, SharedDashboard, WhaleSignal};
use crate::live::WalletSnapshot;
use crate::polymarket::MarketSnapshot;
use crate::snipe::SnipeSignal;

#[derive(Debug, Serialize)]
struct MarketRecorderFrame {
    timestamp: String,
    timestamp_ms: i64,
    last_scan_at: Option<String>,
    scanned_markets: usize,
    active_symbols: Vec<String>,
    watched_markets: Vec<MarketSnapshot>,
    candidates: Vec<SnipeSignal>,
    latest_whale_signal: Option<WhaleSignal>,
    whale_signals: Vec<WhaleSignal>,
    binance_books: std::collections::HashMap<String, BinanceBookInfo>,
    wallet: WalletSnapshot,
}

pub async fn run_market_recorder(
    runtime_settings: Arc<RwLock<Settings>>,
    dashboard_state: SharedDashboard,
) {
    let mut ticker = interval(Duration::from_secs(1));
    let mut last_cleanup_day = String::new();

    loop {
        ticker.tick().await;

        let settings = runtime_settings.read().await.clone();
        if !settings.enable_market_recorder {
            continue;
        }

        if let Err(error) = record_once(&settings, &dashboard_state, &mut last_cleanup_day).await {
            warn!(%error, "market recorder write failed");
        }
    }
}

async fn record_once(
    settings: &Settings,
    dashboard_state: &SharedDashboard,
    last_cleanup_day: &mut String,
) -> Result<()> {
    let now = Utc::now();
    let day = now.format("%Y%m%d").to_string();
    if *last_cleanup_day != day {
        cleanup_old_files(
            &settings.market_recorder_dir,
            settings.market_recorder_retention_hours,
        )
        .await?;
        *last_cleanup_day = day.clone();
    }

    fs::create_dir_all(&settings.market_recorder_dir).await?;
    let path = settings
        .market_recorder_dir
        .join(format!("market-recorder-{day}.jsonl"));

    let dash = dashboard_state.read().await;
    let frame = MarketRecorderFrame {
        timestamp: now.to_rfc3339(),
        timestamp_ms: now.timestamp_millis(),
        last_scan_at: dash.last_scan_at.clone(),
        scanned_markets: dash.scanned_markets,
        active_symbols: dash.active_symbols.clone(),
        watched_markets: dash.watched_markets.clone(),
        candidates: dash.candidates.clone(),
        latest_whale_signal: dash.latest_whale_signal.clone(),
        whale_signals: dash.whale_signals.clone(),
        binance_books: dash.binance_books.clone(),
        wallet: dash.wallet.clone(),
    };
    drop(dash);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let mut line = serde_json::to_vec(&frame)?;
    line.push(b'\n');
    file.write_all(&line).await?;
    Ok(())
}

async fn cleanup_old_files(dir: &Path, retention_hours: i64) -> Result<()> {
    if retention_hours <= 0 || !dir.exists() {
        return Ok(());
    }

    let cutoff = Utc::now() - ChronoDuration::hours(retention_hours);
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path: PathBuf = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(day) = name
            .strip_prefix("market-recorder-")
            .and_then(|name| name.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let Ok(file_date) = chrono::NaiveDate::parse_from_str(day, "%Y%m%d") else {
            continue;
        };
        let Some(file_start) = file_date.and_hms_opt(0, 0, 0) else {
            continue;
        };
        let file_start = chrono::DateTime::<Utc>::from_naive_utc_and_offset(file_start, Utc);
        if file_start < cutoff {
            let _ = fs::remove_file(path).await;
        }
    }
    Ok(())
}
