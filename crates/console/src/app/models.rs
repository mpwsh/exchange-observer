use chrono::Duration;
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
    //pub orders: Option<String>,
    pub exit_reason: Option<String>,
}

#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub sell_floor: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
}
#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub ts: Duration,
    pub change: f32,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f32,
    pub vol: f64,
}
