//! Read-only view types handed to strategies.
//!
//! Strategies never see the scheduler's internal `Token`/`Account` structs —
//! they see these trimmed snapshots. All types serialize, which becomes the
//! wire format when strategy runners split out over WebSocket later.
//!
//! Numeric convention: floats are `f64` at this boundary. The scheduler's
//! internal `f32` fields are widened with `as f64` (exact, so threshold
//! comparisons behave bit-identically to the pre-refactor `f32` compares).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A single OHLCV candle as seen by strategies.
///
/// Mirrors the scheduler's `Candlestick` minus the instrument id (carried by
/// the enclosing view) and minus any storage concerns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    /// Candle timestamp (minute resolution upstream).
    pub ts: DateTime<Utc>,
    /// Opening price.
    pub open: f64,
    /// Highest price.
    pub high: f64,
    /// Lowest price.
    pub low: f64,
    /// Closing price.
    pub close: f64,
    /// Percent change over the candle.
    pub change: f64,
    /// Percent range (high vs low) over the candle.
    pub range: f64,
    /// Traded volume in quote currency.
    pub vol: f64,
}

/// Snapshot of a candidate token being considered for entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenView {
    /// Instrument id, e.g. `BTC-USDT`.
    pub instid: String,
    /// Last known price.
    pub price: f64,
    /// Sum of candle percent-changes over the timeframe.
    pub change: f64,
    /// Standard deviation of candle changes over the timeframe.
    pub std_deviation: f64,
    /// Total volume over the timeframe, in quote currency.
    pub vol: f64,
    /// Whether the token is currently on the deny list.
    pub denied: bool,
    /// Candles covering the configured timeframe, oldest first.
    pub candles: Vec<Candle>,
}

/// Snapshot of an open position being considered for exit.
#[serde_with::serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionView {
    /// Instrument id, e.g. `BTC-USDT`.
    pub instid: String,
    /// Percent change versus the buy price.
    pub change: f64,
    /// Time remaining before the position times out (may be negative).
    /// Kept as a `Duration` so sub-second comparisons match the scheduler's.
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub timeout: Duration,
    /// Whether the token still appears in the scheduler's valid-token list.
    pub still_listed: bool,
    /// Candles covering the configured timeframe, oldest first.
    pub candles: Vec<Candle>,
}

/// Snapshot of account-level state relevant to strategies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioView {
    /// Quote currency currently available to spend.
    pub available: f64,
    /// Quote currency allocated per position.
    pub spendable: f64,
    /// Number of open positions.
    pub positions: usize,
}

