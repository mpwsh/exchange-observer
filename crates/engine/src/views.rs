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
    /// Highest percent change this position has reached during the hold.
    ///
    /// Replaces `still_listed`, which was whether the token currently passes the
    /// **entry** filter and holds a top-N rank — and which `Thresholds::exit_decision`
    /// used to gate the `sell_floor` exit on.
    ///
    /// That coupling was a live bug. It meant your exit depended on your *entry*
    /// filter rejecting the token you were already holding. Loosen `min_dip` and a
    /// rallying position keeps re-qualifying as a fresh entry, `still_listed` stays
    /// true, and the floor **silently stops firing**. Observed: dropping `min_dip`
    /// from 0.4 to 0.17 took `floor_reached` from 42% of exits to 14%, and a
    /// position that peaked at +0.81% rode all the way back to -0.41% because
    /// nothing was allowed to close it.
    ///
    /// A peak is what the floor was always about. Now it says so.
    pub highest: f64,
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
