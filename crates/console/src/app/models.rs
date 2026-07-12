//! Wire format models for the scheduler → console websocket protocol.
//!
//! Each websocket message is a `TextMsg` envelope (see app.rs) whose `data`
//! field is a JSON-encoded payload of one of these types, keyed by the
//! sibling `channel` field.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Account {
    pub balance: Balance,
    pub token_balance: f64,
    pub open_orders: f64,
    pub fee_spend: f64,
    pub earnings: f64,
    pub change: f64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Balance {
    pub start: f64,
    pub current: f64,
    pub available: f64,
    pub spendable: f64,
}

#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Token {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub buy_ts: Duration,
    #[serde(rename = "px")]
    pub price: f64,
    pub change: f32,
    pub std_deviation: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
    pub balance: Balance,
    pub earnings: f64,
    pub fees_deducted: bool,
    pub vol: f64,
    pub vol24h: f64,
    pub change24h: f32,
    pub range: f32,
    pub range24h: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub cooldown: Duration,
    pub candlesticks: Vec<Candlestick>,
    pub status: String,
    pub config: Config,
    pub exit_reason: Option<String>,
}

#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub sell_floor: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    /// Candle timestamp. The scheduler serializes this as an ISO 8601
    /// string (chrono's default), not as milliseconds — parsing it as a
    /// `Duration` was silently killing every `portfolio` message, which
    /// is why open positions were "waiting" forever.
    pub ts: DateTime<Utc>,
    pub change: f32,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f32,
    pub vol: f64,
}

/// One closed position — the wire shape of a report event from the
/// scheduler's `report` channel. Fields mirror the `okx.reports` columns
/// the scheduler flattens onto the wire.
#[derive(Deserialize, Debug, Clone)]
pub struct Report {
    pub round_id: u64,
    pub instid: String,
    pub reason: String,
    pub earnings: f64,
    pub change: f32,
    pub time_left: i64,
    pub highest: f32,
    pub lowest: f32,
    pub buy_price: f64,
    pub sell_price: f64,
    pub strategy: String,
}
