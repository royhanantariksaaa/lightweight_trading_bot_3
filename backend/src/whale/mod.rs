mod book;
mod config;
mod model;
mod parser;
mod runtime;
mod signal;
mod state;
mod tracker;
mod util;

const DEPTH_STREAM_SUFFIX: &str = "depth20@100ms";
const PRE_WHALE_LOOKBACK_MS: i64 = 5_000;
const PRICE_HISTORY_KEEP_MS: i64 = 30_000;
const PROGRESS_PRINT_STEP: f64 = 0.10;

pub use runtime::run_whale_detector;
