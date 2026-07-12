//! Data models for OKX market data.
//!
//! **Design note:** OKX sends every numeric field as a JSON string
//! (`"last": "43000.5"`), not a JSON number. We deserialize those directly
//! into typed Rust fields (`f64`, `i64`) using a helper module, so downstream
//! consumers get typed values without further parsing.
//!
//! For the Scylla insert path, these types expose `to_row()` methods that
//! return typed tuples ready to bind against prepared statements — no more
//! `INSERT ... JSON ?` server-side parsing.

use anyhow::Result;
use scylla::value::CqlTimestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// String-encoded-number helpers.
//
// OKX gives us numbers-as-strings. Serde by itself won't coerce
// `"43000.5"` → `f64`, so we plug custom deserializers into the fields that
// need it. Serialization still emits JSON strings for compatibility with
// anyone downstream who parses our messages.
// ---------------------------------------------------------------------------

mod str_num {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use std::fmt::Display;
    use std::str::FromStr;

    /// Deserialize a JSON string into any `T: FromStr`.
    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        s.parse::<T>().map_err(de::Error::custom)
    }

    /// Serialize any `T: Display` as a JSON string.
    ///
    /// Kept so re-serialized OKX payloads look identical to the originals.
    pub fn serialize<T, S>(val: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: Display,
        S: Serializer,
    {
        s.serialize_str(&val.to_string())
    }
}

// ---------------------------------------------------------------------------
// Channel enum
// ---------------------------------------------------------------------------

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Tickers,
    Candle1m,
    Trades,
    Books,
}

impl Display for Channel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Tickers => "tickers",
            Self::Candle1m => "candle1m",
            Self::Trades => "trades",
            Self::Books => "books",
        };
        write!(f, "{s}")
    }
}

impl FromStr for Channel {
    type Err = ();
    fn from_str(input: &str) -> Result<Channel, Self::Err> {
        match input.to_lowercase().as_str() {
            "tickers" => Ok(Channel::Tickers),
            "candle1m" => Ok(Channel::Candle1m),
            "trades" => Ok(Channel::Trades),
            "books" => Ok(Channel::Books),
            _ => Err(()),
        }
    }
}

impl Channel {
    /// Parse the raw record value (as JSON bytes) into a channel-specific
    /// struct wrapped in a `RowPayload` ready to bind against Scylla.
    pub fn parse(&self, data: &[u8], inst_id: &str) -> Result<RowPayload> {
        Ok(match self {
            Self::Tickers => RowPayload::Ticker {
                inst_id: inst_id.to_owned(),
                row: serde_json::from_slice::<Ticker>(data)?,
            },
            Self::Candle1m => RowPayload::Candle {
                inst_id: inst_id.to_owned(),
                row: serde_json::from_slice::<Candlestick>(data)?,
            },
            Self::Trades => RowPayload::Trade {
                inst_id: inst_id.to_owned(),
                row: serde_json::from_slice::<Trade>(data)?,
            },
            Self::Books => RowPayload::Book {
                inst_id: inst_id.to_owned(),
                row: serde_json::from_slice::<Book>(data)?,
            },
        })
    }
}

/// A parsed row ready to be inserted. Each variant owns its inst_id and the
/// typed struct; the consumer matches on this and calls the appropriate
/// prepared statement.
#[derive(Debug)]
pub enum RowPayload {
    Ticker { inst_id: String, row: Ticker },
    Candle { inst_id: String, row: Candlestick },
    Trade { inst_id: String, row: Trade },
    Book { inst_id: String, row: Book },
}

// ---------------------------------------------------------------------------
// Ticker
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticker {
    #[serde(with = "str_num")]
    pub ask_px: f64,
    #[serde(with = "str_num")]
    pub ask_sz: f64,
    #[serde(with = "str_num")]
    pub bid_px: f64,
    #[serde(with = "str_num")]
    pub bid_sz: f64,
    #[serde(with = "str_num")]
    pub high24h: f64,
    #[serde(with = "str_num")]
    pub last: f64,
    #[serde(with = "str_num")]
    pub last_sz: f64,
    #[serde(with = "str_num")]
    pub low24h: f64,
    #[serde(with = "str_num")]
    pub open24h: f64,
    #[serde(with = "str_num")]
    pub sod_utc0: f64,
    #[serde(with = "str_num")]
    pub sod_utc8: f64,
    #[serde(with = "str_num")]
    pub ts: i64,
    #[serde(with = "str_num")]
    pub vol24h: f64,
    #[serde(with = "str_num")]
    pub vol_ccy24h: f64,
}

/// Column-order tuple for the tickers table.
///
/// Matches:
/// ```cql
/// INSERT INTO okx.tickers
///   (instid, askpx, asksz, bidpx, bidsz, high24h, last, lastsz,
///    low24h, open24h, sodutc0, sodutc8, ts, vol24h, volccy24h)
/// VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
/// ```
pub type TickerRow<'a> = (
    &'a str,      // instid
    f64,          // askpx
    f64,          // asksz
    f64,          // bidpx
    f64,          // bidsz
    f64,          // high24h
    f64,          // last
    f64,          // lastsz
    f64,          // low24h
    f64,          // open24h
    f64,          // sodutc0
    f64,          // sodutc8
    CqlTimestamp, // ts
    f64,          // vol24h
    f64,          // volccy24h
);

impl Ticker {
    pub fn to_row<'a>(&self, inst_id: &'a str) -> TickerRow<'a> {
        (
            inst_id,
            self.ask_px,
            self.ask_sz,
            self.bid_px,
            self.bid_sz,
            self.high24h,
            self.last,
            self.last_sz,
            self.low24h,
            self.open24h,
            self.sod_utc0,
            self.sod_utc8,
            CqlTimestamp(self.ts),
            self.vol24h,
            self.vol_ccy24h,
        )
    }
}

// ---------------------------------------------------------------------------
// Trade
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trade {
    #[serde(with = "str_num")]
    pub px: f64,
    pub side: String,
    #[serde(with = "str_num")]
    pub sz: f64,
    #[serde(with = "str_num")]
    pub trade_id: i32,
    #[serde(with = "str_num")]
    pub ts: i64,
}

/// Matches `INSERT INTO okx.trades (instid, sz, tradeid, px, side, ts) VALUES (?,?,?,?,?,?)`.
pub type TradeRow<'a> = (&'a str, f64, i32, f64, &'a str, CqlTimestamp);

impl Trade {
    pub fn to_row<'a>(&'a self, inst_id: &'a str) -> TradeRow<'a> {
        (
            inst_id,
            self.sz,
            self.trade_id,
            self.px,
            &self.side,
            CqlTimestamp(self.ts),
        )
    }
}

// ---------------------------------------------------------------------------
// Book
//
// OKX books come with `asks`/`bids` as arrays of arrays of strings:
//   [ ["price", "quantity", "deprecated", "num_orders"], ... ]
// We turn each string into an `OrderEntry` UDT (price float, quantity int,
// deprecated int, num_orders int) at deserialization time.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, scylla::SerializeValue)]
pub struct OrderEntry {
    pub price: f32,
    pub quantity: i32,
    pub deprecated: i32,
    pub num_orders: i32,
}

impl OrderEntry {
    fn from_okx_row(row: &[String]) -> Option<Self> {
        if row.len() < 4 {
            return None;
        }
        Some(OrderEntry {
            price: row[0].parse().ok()?,
            quantity: row[1].parse().ok()?,
            deprecated: row[2].parse().ok()?,
            num_orders: row[3].parse().ok()?,
        })
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    #[serde(deserialize_with = "de_order_entries")]
    pub asks: Vec<OrderEntry>,
    #[serde(deserialize_with = "de_order_entries")]
    pub bids: Vec<OrderEntry>,
    pub checksum: Option<i64>,
    pub prev_seq_id: Option<i64>,
    pub seq_id: i64,
    #[serde(with = "str_num")]
    pub ts: i64,
}

fn de_order_entries<'de, D>(d: D) -> Result<Vec<OrderEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Vec<Vec<String>> = Vec::deserialize(d)?;
    Ok(raw
        .iter()
        .filter_map(|r| OrderEntry::from_okx_row(r))
        .collect())
}

/// Matches `INSERT INTO okx.books (instid, asks, bids, checksum, prev_seq_id, seq_id, ts) ...`.
pub type BookRow<'a> = (
    &'a str,
    &'a [OrderEntry],
    &'a [OrderEntry],
    Option<i32>,
    Option<i32>,
    i32,
    CqlTimestamp,
);

impl Book {
    pub fn to_row<'a>(&'a self, inst_id: &'a str) -> BookRow<'a> {
        (
            inst_id,
            &self.asks,
            &self.bids,
            self.checksum.map(|v| v as i32),
            self.prev_seq_id.map(|v| v as i32),
            self.seq_id as i32,
            CqlTimestamp(self.ts),
        )
    }
}

// ---------------------------------------------------------------------------
// Candlestick
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candlestick {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub change: f64,
    pub range: f64,
    pub volume: f64,
    pub ts: i64,
}

/// Matches `INSERT INTO okx.candle1m (instid, open, high, low, close, volume, change, range, ts) ...`.
///
/// Note: schema was originally `change float` / `range float` — the migration
/// bumps those columns to `double` so the Rust `f64` matches exactly. See
/// `scylla/migration_v2.cql`.
pub type CandleRow<'a> = (&'a str, f64, f64, f64, f64, f64, f64, f64, CqlTimestamp);

impl Candlestick {
    pub fn to_row<'a>(&self, inst_id: &'a str) -> CandleRow<'a> {
        (
            inst_id,
            self.open,
            self.high,
            self.low,
            self.close,
            self.volume,
            self.change,
            self.range,
            CqlTimestamp(self.ts),
        )
    }

    /// Build a candle from an OKX WS `candle1m` message.
    ///
    /// Rewritten to avoid the panicking `.unwrap()` chain that was in the
    /// previous version — a missing or malformed field now yields a default
    /// candle instead of crashing the producer loop.
    pub fn from_candle(msg: &Value) -> Candlestick {
        let Some(row) = msg.pointer("/data/0") else {
            return Candlestick::default();
        };
        let f = |i: usize| -> f64 {
            row.get(i)
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0)
        };
        let ts = row
            .get(0)
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Candlestick {
            ts,
            open: f(1),
            high: f(2),
            low: f(3),
            close: f(4),
            volume: f(6),
            change: 0.0,
            range: 0.0,
        }
    }

    pub fn get_range(mut self) -> Self {
        if self.low != 0.0 {
            let range = self.high - self.low;
            self.range = (range / self.low * 100.0 * 100.0).round() / 100.0;
        }
        self
    }

    pub fn get_change(mut self) -> Self {
        if self.open != 0.0 {
            let change = self.close - self.open;
            self.change = (change / self.open * 100.0 * 100.0).round() / 100.0;
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Websocket subscribe messages (unchanged from before)
// ---------------------------------------------------------------------------

#[derive(Serialize, Debug, Deserialize)]
pub struct SubscribeMsg {
    pub op: String,
    pub args: Vec<SubscribeArg>,
}

#[derive(Serialize, Deserialize)]
pub struct WsResponse {
    pub event: String,
    pub arg: SubscribeArg,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeArg {
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inst_type: Option<String>,
    pub inst_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ticker_deserializes_okx_string_numbers() {
        let raw = r#"{
            "askPx":"7.096","askSz":"107.9","bidPx":"7.095","bidSz":"5.7",
            "high24h":"7.15","last":"7.096","lastSz":"0.2","low24h":"7.024",
            "open24h":"7.069","sodUtc0":"7.066","sodUtc8":"7.088",
            "ts":"1783755266261","vol24h":"61376.0","volCcy24h":"435309.2"
        }"#;
        let t: Ticker = serde_json::from_str(raw).unwrap();
        assert!((t.last - 7.096).abs() < 1e-9);
        assert_eq!(t.ts, 1_783_755_266_261);
    }

    #[test]
    fn ticker_to_row_matches_schema_order() {
        let t = Ticker {
            last: 43_000.5,
            ts: 1_783_755_266_261,
            ..Default::default()
        };
        let row = t.to_row("BTC-USDT");
        assert_eq!(row.0, "BTC-USDT");
        assert_eq!(row.12.0, 1_783_755_266_261);
    }

    #[test]
    fn candlestick_from_candle_survives_malformed_input() {
        let empty = json!({ "data": null });
        assert_eq!(Candlestick::from_candle(&empty), Candlestick::default());

        let missing_fields = json!({ "data": [["1783755000000"]] });
        let c = Candlestick::from_candle(&missing_fields);
        assert_eq!(c.ts, 1_783_755_000_000);
        assert_eq!(c.open, 0.0);
    }

    #[test]
    fn order_entry_skips_short_rows() {
        assert!(OrderEntry::from_okx_row(&["1.0".to_owned()]).is_none());
        let ok = OrderEntry::from_okx_row(&[
            "1.5".to_owned(),
            "10".to_owned(),
            "0".to_owned(),
            "3".to_owned(),
        ])
        .unwrap();
        assert_eq!(ok.quantity, 10);
    }
}
