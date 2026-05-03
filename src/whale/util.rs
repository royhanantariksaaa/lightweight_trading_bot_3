use chrono::{DateTime, Utc};

pub fn side_from_m_flag(buyer_is_market_maker: bool) -> &'static str {
    if buyer_is_market_maker { "SELL" } else { "BUY" }
}

pub fn symbol_from_stream(stream: &str) -> &str {
    stream.split('@').next().unwrap_or("")
}

pub fn format_timestamp(ms: Option<i64>) -> String {
    match ms.and_then(DateTime::<Utc>::from_timestamp_millis) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    }
}
