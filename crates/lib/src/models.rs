//! Data models for OKX market data.
//!
//! **Design note:** OKX sends every numeric field as a JSON string
//! (`"last": "43000.5"`), not a JSON number. We deserialize those directly
//! into typed Rust fields (`f64`, `i64`) using a helper module, so downstream
//! consumers get typed values without further parsing.
//!
//! For the Scylla insert path, these types expose `to_row()` methods that
//! return typed tuples ready to bind against prepared statements.
//!
//! **Numeric widths:** exchange identifiers and sequence numbers are `i64`, not
//! `i32`. OKX `seqId` and `tradeId` both exceed 2^31 in practice, and the
//! previous `as i32` casts wrapped them silently. See `scylla/migration_v3.cql`
//! for the column changes this requires.

use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use anyhow::Result;
use scylla::value::CqlTimestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// String-encoded-number helpers.
// ---------------------------------------------------------------------------

mod str_num {
    use std::{fmt::Display, str::FromStr};

    use serde::{de, Deserialize, Deserializer, Serializer};

    /// Deserialize a JSON string into any `T: FromStr`.
    ///
    /// **An empty string yields `T::default()`, not an error.**
    ///
    /// OKX sends `"askPx": ""` (and `""` for the other side, `lastSz`, and some
    /// 24h fields) whenever a book side is empty — which happens constantly on
    /// thin spot instruments. `"".parse::<f64>()` errors, and that error used to
    /// propagate all the way up: `send_message` returned `Err`, `process_message`
    /// logged
    ///
    /// ```text
    /// WARN producer] process_message failed: cannot parse float from empty string
    /// ```
    ///
    /// ...and **threw the whole message away**. Not just the missing field — the
    /// entire ticker, including `last`. Those warnings have been scrolling past
    /// since the producer was written; they became load-bearing the moment orders
    /// started pricing off `askPx`/`bidPx`.
    ///
    /// `0.0` is the honest reading of "there is nothing resting on that side", and
    /// downstream `Token::top_of_book()` treats a zero ask or bid as *no book* and
    /// refuses to enter. Which is correct: an instrument with an empty book side
    /// is not one you can buy.
    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: FromStr + Default,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            return Ok(T::default());
        }
        s.parse::<T>().map_err(de::Error::custom)
    }

    /// Serialize any `T: Display` as a JSON string.
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

/// A parsed row ready to be inserted.
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

    /// Quoted spread in basis points of the mid, or `None` when either side of
    /// the book is missing.
    ///
    /// Exposed because it is the number nobody in this system was looking at: a
    /// round trip costs `2 x taker_fee` *plus* the spread, twice, and on a 1%
    /// take-profit that is not a rounding error.
    #[must_use]
    pub fn spread_bps(&self) -> Option<f64> {
        if self.bid_px <= 0.0 || self.ask_px <= 0.0 || self.ask_px < self.bid_px {
            return None;
        }
        let mid = (self.ask_px + self.bid_px) / 2.0;
        (mid > 0.0).then(|| (self.ask_px - self.bid_px) / mid * 10_000.0)
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
    /// OKX `tradeId`.
    ///
    /// Was `i32`. OKX trade ids are monotonically increasing per instrument and
    /// pass 2^31 on active instruments; parsing one that does into an `i32`
    /// fails outright (`str_num` returns a serde error), so the whole trade
    /// message is dropped. Widened to `i64` — the `trades.tradeid` column has to
    /// become `bigint` to match. See `scylla/migration_v3.cql`.
    #[serde(with = "str_num")]
    pub trade_id: i64,
    #[serde(with = "str_num")]
    pub ts: i64,
}

/// Matches `INSERT INTO okx.trades (instid, sz, tradeid, px, side, ts) VALUES (?,?,?,?,?,?)`.
pub type TradeRow<'a> = (&'a str, f64, i64, f64, &'a str, CqlTimestamp);

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
//   [ "price", "quantity", "deprecated", "num_orders" ]
// ---------------------------------------------------------------------------

/// One level of the order book.
///
/// **This type is why `okx.books` is empty.** `quantity` was `i32`, and OKX
/// sends sizes as decimal strings — `"8.5"`, `"0.34"`. `"8.5".parse::<i32>()`
/// fails, `from_okx_row` returned `None`, and `de_order_entries` silently
/// `filter_map`ped the level away. Every fractional level — which is nearly all
/// of them on spot — was discarded on ingest, without a log line. `price` was
/// `f32`, which additionally throws away precision on sub-cent instruments,
/// where the tick is smaller than an `f32` can resolve at that magnitude.
///
/// Both are now `f64`. This changes the `order_entry` UDT and requires a
/// migration — Scylla cannot `ALTER TYPE` a field's type, so the UDT and the
/// `books` table have to be recreated. That is cheap here precisely because the
/// existing data is worthless.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, scylla::SerializeValue)]
pub struct OrderEntry {
    pub price: f64,
    pub quantity: f64,
    /// Always "0" on spot (deprecated by OKX). Kept for wire compatibility.
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
    /// OKX checksum. Genuinely a signed 32-bit value, so `i32` is correct here.
    pub checksum: Option<i32>,
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
///
/// `seq_id` / `prev_seq_id` were being written as `i32` via `as` casts. OKX
/// sequence ids are well past 2^31, and `as i32` on an out-of-range `i64` wraps
/// silently — so the sequence numbers in the table were not just wrong, they were
/// wrong in a way that still looked like plausible integers. Both columns need to
/// be `bigint`.
pub type BookRow<'a> = (
    &'a str,
    &'a [OrderEntry],
    &'a [OrderEntry],
    Option<i32>,
    Option<i64>,
    i64,
    CqlTimestamp,
);

impl Book {
    pub fn to_row<'a>(&'a self, inst_id: &'a str) -> BookRow<'a> {
        (
            inst_id,
            &self.asks,
            &self.bids,
            self.checksum,
            self.prev_seq_id,
            self.seq_id,
            CqlTimestamp(self.ts),
        )
    }

    /// Best bid / best ask, if the book has both sides.
    #[must_use]
    pub fn touch(&self) -> Option<(f64, f64)> {
        let bid = self.bids.first()?.price;
        let ask = self.asks.first()?.price;
        (bid > 0.0 && ask >= bid).then_some((bid, ask))
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
    /// Quote-currency volume for the bar (OKX `volCcy`, element 6).
    ///
    /// Quote, not base — this is what `strategy.min_vol` is denominated in.
    pub volume: f64,
    pub ts: i64,
    /// OKX `confirm` (element 8): `false` while the bar is still being built.
    ///
    /// New. OKX pushes the in-progress minute repeatedly with `confirm = "0"`
    /// and then once more with `"1"` when it closes. The old parser ignored the
    /// flag, so partial bars were written to `candle1m` indistinguishable from
    /// closed ones — and downstream, `min_change_last_candle` and the reversion
    /// bounce window were being evaluated against bars that were seconds old.
    ///
    /// `#[serde(default)]` so messages produced before this field existed still
    /// deserialize (as unconfirmed, which is the safe reading).
    #[serde(default)]
    pub confirm: bool,
}

/// Bind tuple for the candle1m insert.
///
/// ```cql
/// INSERT INTO okx.candle1m
///   (instid, open, high, low, close, volume, change, range, ts, confirm)
/// VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) USING TTL <ttl> AND TIMESTAMP ?
/// ```
///
/// The trailing `i64` is **not data** — it binds the `USING TIMESTAMP` marker,
/// in microseconds. It is here because a bind is a flat tuple and there is
/// nowhere else to put it.
///
/// Why it exists: OKX pushes the in-progress minute repeatedly, and every push
/// upserts the same `(instid, ts)` row. The consumer inserts each record in its
/// own `tokio::spawn` with 512 in flight, so Kafka's ordering is discarded — and
/// Scylla resolves same-cell conflicts by the write timestamp the *coordinator*
/// assigns when the write lands. If the `confirm = true` push happens to land
/// before an earlier partial push for the same bar, the partial one wins, and the
/// bar is permanently truncated.
///
/// Binding the timestamp from the Kafka record makes the later-produced message
/// win regardless of execution order, and makes `confirm` mean what it says: a
/// past bar with `confirm = false` is a genuine producer disconnect, not a lost
/// race.
pub type CandleRow<'a> = (
    &'a str,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    CqlTimestamp,
    bool,
    i64,
);

impl Candlestick {
    /// `write_ts_micros` binds `USING TIMESTAMP` — see [`CandleRow`]. Pass the
    /// Kafka record's own timestamp (`record.timestamp.timestamp_micros()`), not
    /// the candle's `ts`: every push for a given bar carries the *same* candle
    /// `ts`, so it cannot break the tie between them.
    pub fn to_row<'a>(&self, inst_id: &'a str, write_ts_micros: i64) -> CandleRow<'a> {
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
            self.confirm,
            write_ts_micros,
        )
    }

    /// Build a candle from an OKX WS `candle1m` message.
    ///
    /// Element order for SPOT:
    /// `[ts, open, high, low, close, vol(base), volCcy(quote), volCcyQuote, confirm]`
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
        // OKX sends "1" / "0". Anything we can't read is treated as unconfirmed,
        // which is the conservative reading: a consumer that filters on this
        // should drop the bar rather than trade on it.
        let confirm = row.get(8).and_then(Value::as_str).is_some_and(|s| s == "1");

        Candlestick {
            ts,
            open: f(1),
            high: f(2),
            low: f(3),
            close: f(4),
            volume: f(6),
            change: 0.0,
            range: 0.0,
            confirm,
        }
    }

    /// High-to-low range as a percentage of the low.
    ///
    /// The `.round()` that used to quantize this to two decimals is gone. It
    /// meant every range under 0.005% was stored as exactly `0.0`, and every
    /// other one was snapped to the nearest hundredth of a percent — on 1-minute
    /// bars, where the signal being measured is itself a few hundredths of a
    /// percent, that is most of the information. (`scheduler/src/utils.rs` has a
    /// test celebrating that it stopped doing this. That was the *consumer*. The
    /// producer kept doing it.)
    ///
    /// The columns are already `double`; nothing downstream needs to change.
    #[must_use]
    pub fn get_range(mut self) -> Self {
        if self.low != 0.0 && self.low.is_finite() && self.high.is_finite() {
            self.range = (self.high - self.low) / self.low * 100.0;
        }
        self
    }

    /// Close-to-open change as a percentage of the open. See [`Self::get_range`]
    /// for why the rounding is gone.
    #[must_use]
    pub fn get_change(mut self) -> Self {
        if self.open != 0.0 && self.open.is_finite() && self.close.is_finite() {
            self.change = (self.close - self.open) / self.open * 100.0;
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Websocket subscribe messages
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
    use serde_json::json;

    use super::*;

    /// A spot book with fractional sizes and a sequence id above `i32::MAX` —
    /// i.e. an entirely ordinary one.
    fn book_json() -> &'static str {
        r#"{
            "asks": [["100.05", "8.5", "0", "3"], ["100.06", "1.0", "0", "1"]],
            "bids": [["99.95", "12.25", "0", "2"]],
            "checksum": -1500093,
            "seqId": 12345678901,
            "prevSeqId": 12345678900,
            "ts": "1783755266261"
        }"#
    }

    #[test]
    fn ticker_deserializes_okx_string_numbers() {
        let raw = r#"{
            "askPx":"7.096","askSz":"107.9","bidPx":"7.095","bidSz":"5.7",
            "high24h":"7.15","last":"7.096","lastSz":"0.2","low24h":"7.024",
            "open24h":"7.069","sodUtc0":"7.066","sodUtc8":"7.088",
            "ts":"1783755266261","vol24h":"61376.0","volCcy24h":"435309.2"
        }"#;
        let t: Ticker = serde_json::from_str(raw).expect("valid ticker");
        assert!((t.last - 7.096).abs() < 1e-9);
        assert_eq!(t.ts, 1_783_755_266_261);
    }

    #[test]
    fn ticker_with_an_empty_book_side_should_deserialize_not_explode() {
        // OKX sends "" for a side with nothing resting on it. This used to abort
        // the parse and discard the entire message — `last` included.
        let raw = r#"{
            "askPx":"","askSz":"","bidPx":"7.095","bidSz":"5.7",
            "high24h":"7.15","last":"7.096","lastSz":"0.2","low24h":"7.024",
            "open24h":"7.069","sodUtc0":"7.066","sodUtc8":"7.088",
            "ts":"1783755266261","vol24h":"61376.0","volCcy24h":"435309.2"
        }"#;
        let t: Ticker = serde_json::from_str(raw).expect("empty side parses");
        assert_eq!(t.ask_px, 0.0);
        // The point of the exercise: we still get the price.
        assert!((t.last - 7.096).abs() < 1e-9);
    }

    #[test]
    fn ticker_with_an_empty_book_side_reports_no_spread() {
        let t = Ticker {
            ask_px: 0.0,
            bid_px: 7.095,
            ..Default::default()
        };
        assert_eq!(t.spread_bps(), None);
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
        assert_eq!(row.12 .0, 1_783_755_266_261);
    }

    #[test]
    fn ticker_spread_bps_reports_the_quoted_spread() {
        let t = Ticker {
            bid_px: 99.95,
            ask_px: 100.05,
            ..Default::default()
        };
        let spread = t.spread_bps().expect("book present");
        assert!((spread - 10.0).abs() < 0.01);
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
    fn candlestick_from_candle_reads_the_confirm_flag() {
        let closed = json!({ "data": [[
            "1783755000000", "1.0", "1.1", "0.9", "1.05",
            "100", "105", "105", "1"
        ]]});
        assert!(Candlestick::from_candle(&closed).confirm);
    }

    #[test]
    fn candlestick_from_candle_treats_an_in_progress_bar_as_unconfirmed() {
        let live = json!({ "data": [[
            "1783755000000", "1.0", "1.1", "0.9", "1.05",
            "100", "105", "105", "0"
        ]]});
        assert!(!Candlestick::from_candle(&live).confirm);
    }

    #[test]
    fn candlestick_change_keeps_sub_basis_point_moves() {
        // 0.003% used to round to exactly 0.0 and vanish.
        let c = Candlestick {
            open: 100.0,
            close: 100.003,
            ..Default::default()
        }
        .get_change();
        assert!((c.change - 0.003).abs() < 1e-9);
    }

    #[test]
    fn candlestick_range_keeps_full_precision() {
        let c = Candlestick {
            low: 100.0,
            high: 100.007,
            ..Default::default()
        }
        .get_range();
        assert!((c.range - 0.007).abs() < 1e-9);
    }

    #[test]
    fn order_entry_skips_short_rows() {
        assert!(OrderEntry::from_okx_row(&["1.0".to_owned()]).is_none());
    }

    #[test]
    fn order_entry_accepts_fractional_quantities() {
        // The whole reason `okx.books` is empty: "8.5" is not an i32.
        let entry = OrderEntry::from_okx_row(&[
            "1.5".to_owned(),
            "8.5".to_owned(),
            "0".to_owned(),
            "3".to_owned(),
        ])
        .expect("fractional size parses");
        assert!((entry.quantity - 8.5).abs() < 1e-9);
    }

    /// The old model dropped every level whose size wasn't an integer, which on
    /// spot is nearly all of them — silently, via `filter_map`.
    #[test]
    fn book_deserializes_fractional_levels() {
        let b: Book = serde_json::from_str(book_json()).expect("valid book");
        assert_eq!(b.asks.len(), 2);
        assert_eq!(b.bids.len(), 1);
    }

    #[test]
    fn book_seq_id_survives_values_above_i32_max() {
        let b: Book = serde_json::from_str(book_json()).expect("valid book");
        // `as i32` used to wrap this into something that still looked like a
        // plausible sequence number.
        assert_eq!(b.to_row("BTC-USDT").5, 12_345_678_901);
    }

    #[test]
    fn book_touch_returns_best_bid_and_ask() {
        let b: Book = serde_json::from_str(book_json()).expect("valid book");
        let (bid, ask) = b.touch().expect("both sides present");
        assert!((bid - 99.95).abs() < 1e-9 && (ask - 100.05).abs() < 1e-9);
    }

    #[test]
    fn trade_id_survives_values_above_i32_max() {
        let raw =
            r#"{"px":"1.0","side":"buy","sz":"1.0","tradeId":"3000000000","ts":"1783755266261"}"#;
        let t: Trade = serde_json::from_str(raw).expect("valid trade");
        assert_eq!(t.trade_id, 3_000_000_000);
    }
}
