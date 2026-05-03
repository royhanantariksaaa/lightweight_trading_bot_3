use crate::config::Settings;

use super::DEPTH_STREAM_SUFFIX;

#[derive(Clone, Debug)]
pub struct MarketConfig {
    pub market: String,
    pub symbol: String,
    pub url: String,
    pub uses_usdt_conversion: bool,
}

pub fn build_markets(settings: &Settings) -> Vec<MarketConfig> {
    settings
        .effective_whale_symbols()
        .into_iter()
        .map(|base| {
            let symbol = if base.ends_with("USDT") {
                base
            } else {
                format!("{base}USDT")
            };
            let stream_symbol = symbol.to_ascii_lowercase();
            let url = format!(
                "wss://stream.binance.com:9443/stream?streams=usdtusd@bookTicker/{stream_symbol}@bookTicker/{stream_symbol}@aggTrade/{stream_symbol}@{}",
                DEPTH_STREAM_SUFFIX
            );

            MarketConfig {
                market: format!("{symbol} SPOT"),
                symbol,
                url,
                uses_usdt_conversion: true,
            }
        })
        .collect()
}
