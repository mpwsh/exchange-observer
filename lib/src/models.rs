use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

use anyhow::Result;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Tickers,
    Candle1m,
    Trades,
}

impl ToString for Channel {
    fn to_string(&self) -> String {
        match self {
            Self::Tickers => "tickers".to_string(),
            Self::Candle1m => "candle1m".to_string(),
            Self::Trades => "trades".to_string(),
        }
    }
}

impl FromStr for Channel {
    type Err = ();
    fn from_str(input: &str) -> Result<Channel, Self::Err> {
        let lower = input.to_lowercase();
        match lower.as_ref() {
            "tickers" => Ok(Channel::Tickers),
            "candle1m" => Ok(Channel::Candle1m),
            "trades" => Ok(Channel::Trades),
            _ => Err(()),
        }
    }
}

impl Channel {
    pub fn parse(&self, data: &[u8], inst_id: &str) -> Result<String> {
        // Collect offers into a vec
        let json = match self {
            Self::Tickers => serde_json::from_slice::<Ticker>(data)?.build_query(inst_id),
            Self::Candle1m => serde_json::from_slice::<Candlestick>(data)?.build_query(inst_id),

            Self::Trades => serde_json::from_slice::<Trade>(data)?.build_query(inst_id),
        };
        Ok(json.to_string())
    }
}
#[derive(Default, Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticker {
    pub ask_px: String,
    pub ask_sz: String,
    pub bid_px: String,
    pub bid_sz: String,
    pub high24h: String,
    //pub inst_type: String,
    pub last: String,
    pub last_sz: String,
    pub low24h: String,
    pub open24h: String,
    pub sod_utc0: String,
    pub sod_utc8: String,
    pub ts: String,
    pub vol24h: String,
    pub vol_ccy24h: String,
}
impl Ticker {
    pub fn build_query(self, inst_id: &str) -> Value {
        json!({
        "instid": inst_id,
        "askpx": self.ask_px,
        "asksz": self.ask_sz,
        "bidpx": self.bid_px,
        "bidsz": self.bid_sz,
        "high24h": self.high24h,
        //"insttype": self.inst_type,
        "last": self.last,
        "lastsz": self.last_sz,
        "low24h": self.low24h,
        "open24h": self.open24h,
        "sodutc0": self.sod_utc0,
        "sodutc8": self.sod_utc8,
        "ts": self.ts,
        "vol24h": self.vol24h,
        "volccy24h": self.vol_ccy24h,
        })
    }
}

#[derive(Default, Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trade {
    pub px: String,
    pub side: String,
    pub sz: String,
    pub trade_id: String,
    pub ts: String,
}

impl Trade {
    pub fn build_query(self, inst_id: &str) -> Value {
        json!({
        "instid": inst_id,
        "px": self.px,
        "side": self.side,
        "sz": self.sz,
        "tradeid": self.trade_id,
        "ts": self.ts,
        })
    }
}

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
impl Candlestick {
    pub fn build_query(self, inst_id: &str) -> Value {
        json!({
        "instid": inst_id,
        "open": self.open,
        "high": self.high,
        "low": self.low,
        "close": self.close,
        "range": self.range,
        "change": self.change,
        "volume": self.volume,
        "ts": self.ts,
        })
    }

    pub fn new() -> Candlestick {
        Candlestick {
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            range: 0.0,
            change: 0.0,
            ts: 0,
            volume: 0.0,
        }
    }
    pub fn from_candle(msg: &Value) -> Candlestick {
        if msg["data"] != json!(null) {
            let x = &msg["data"][0];
            Candlestick {
                open: x[1].as_str().unwrap().parse::<f64>().unwrap_or(0.0),
                high: x[2].as_str().unwrap().parse::<f64>().unwrap_or(0.0),
                low: x[3].as_str().unwrap().parse::<f64>().unwrap_or(0.0),
                close: x[4].as_str().unwrap().parse::<f64>().unwrap_or(0.0),
                //vol in USD
                volume: x[6].as_str().unwrap().parse::<f64>().unwrap_or(0.0),
                ts: x[0].as_str().unwrap().parse::<i64>().unwrap_or(0),
                range: 0.0,
                change: 0.0,
            }
        } else {
            Candlestick::new()
        }
    }

    pub fn get_range(mut self) -> Self {
        let range = self.high - self.low;
        self.range = format!("{:.2}", (range / self.low) * 101.00)
            .parse()
            .unwrap();
        self
    }
    pub fn get_change(mut self) -> Self {
        let change = self.close - self.open;
        self.change = format!("{:.2}", (change / self.open) * 101.00)
            .parse()
            .unwrap();
        self
    }
}

#[derive(Serialize, Deserialize)]
pub struct SubscribeMsg {
    pub op: String,
    pub args: Vec<SubArg>,
}

#[derive(Serialize, Deserialize)]
pub struct WsResponse {
    pub event: String,
    pub arg: SubArg,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubArg {
    pub channel: String,
    pub inst_type: String,
    pub inst_id: Option<String>,
}
