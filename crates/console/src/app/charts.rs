//! Chart builders that turn account/token data into `egui_plot` primitives.
//!
//! Each `*Chart` type holds pre-computed `[x, y]` series lists. The UI layer
//! wraps them in `Line`/`BoxPlot` and hands them to `Plot::show`.

use chrono::Duration;
use egui::{Color32, Stroke};
use egui_plot::{BoxElem, BoxSpread};

use super::models::{Account, Candlestick};

// ---------------------------------------------------------------------------
// Candlestick box plot
// ---------------------------------------------------------------------------

pub struct CandlestickBoxPlot {
    pub boxes: Vec<BoxElem>,
}

impl CandlestickBoxPlot {
    /// Build a box plot from a series of candlesticks, prepending a marker
    /// box at `buy_ts`/`buy_price` so the user can see where they entered.
    ///
    /// The buy marker is coloured yellow to distinguish it from the green/red
    /// price candles.
    pub fn new(candlesticks: &[Candlestick], buy_ts: Duration, buy_price: f64) -> Self {
        // Build a merged, sorted list without allocating a full Vec<Candlestick>
        // clone — we just carry a small BuyMarker variant alongside the borrowed
        // candle references.
        enum Item<'a> {
            Buy,
            Candle(&'a Candlestick),
        }

        let mut items: Vec<(Duration, Item<'_>)> = candlesticks
            .iter()
            .map(|c| (c.ts, Item::Candle(c)))
            .chain(std::iter::once((buy_ts, Item::Buy)))
            .collect();
        items.sort_by_key(|(ts, _)| *ts);

        let boxes = items
            .into_iter()
            .map(|(ts, item)| match item {
                Item::Buy => build_buy_marker(ts, buy_price),
                Item::Candle(c) => build_candle_box(c),
            })
            .collect();

        Self { boxes }
    }
}

const BOX_WIDTH: f64 = 30.0;
const WHISKER_WIDTH: f64 = 0.1;

fn build_candle_box(c: &Candlestick) -> BoxElem {
    let ts_secs = c.ts.num_seconds() as f64;
    let (lower, upper) = if c.open < c.close {
        (c.open, c.close)
    } else {
        (c.close, c.open)
    };
    let median = (c.open + c.close) / 2.0;
    let colour = if c.open < c.close {
        Color32::DARK_GREEN
    } else {
        Color32::DARK_RED
    };

    let legend = format!(
        "Time: {}\nVol: {}\nChange: {}\nOpen: {}\nClose: {}",
        c.ts.num_seconds(),
        c.vol,
        c.change,
        c.open,
        c.close,
    );

    BoxElem::new(ts_secs, BoxSpread::new(c.low, lower, median, upper, c.high))
        .name(legend)
        .fill(colour)
        .box_width(BOX_WIDTH)
        .whisker_width(WHISKER_WIDTH)
        .stroke(Stroke::new(1.0, colour))
}

fn build_buy_marker(ts: Duration, price: f64) -> BoxElem {
    let ts_secs = ts.num_seconds() as f64;
    let legend = format!("Time: {}\nBuy price: {price}", ts.num_seconds());
    BoxElem::new(ts_secs, BoxSpread::new(price, price, price, price, price))
        .name(legend)
        .fill(Color32::YELLOW)
        .box_width(BOX_WIDTH)
        .whisker_width(WHISKER_WIDTH)
        .stroke(Stroke::new(1.0, Color32::YELLOW))
}

// ---------------------------------------------------------------------------
// Balance / change / earnings line charts
// ---------------------------------------------------------------------------

/// Build a `[ts, value]` series from paired history + timestamps by picking
/// the value with a projection.
fn series_from(
    history: &[Account],
    timestamps: &[i64],
    project: impl Fn(&Account) -> f64,
) -> Vec<[f64; 2]> {
    history
        .iter()
        .zip(timestamps.iter())
        .map(|(a, &ts)| [ts as f64, project(a)])
        .collect()
}

pub struct BalanceChart {
    pub current: Vec<[f64; 2]>,
    pub tokens: Vec<[f64; 2]>,
    pub open_orders: Vec<[f64; 2]>,
    pub available: Vec<[f64; 2]>,
}

impl BalanceChart {
    pub fn new(history: &[Account], timestamps: &[i64]) -> Self {
        Self {
            current: series_from(history, timestamps, |a| a.balance.current),
            tokens: series_from(history, timestamps, |a| a.token_balance),
            open_orders: series_from(history, timestamps, |a| a.open_orders),
            available: series_from(history, timestamps, |a| a.balance.available),
        }
    }
}

pub struct ChangeChart {
    pub change: Vec<[f64; 2]>,
}

impl ChangeChart {
    pub fn new(history: &[Account], timestamps: &[i64]) -> Self {
        Self {
            change: series_from(history, timestamps, |a| a.change),
        }
    }
}

pub struct EarningsChart {
    pub earnings: Vec<[f64; 2]>,
    pub fees: Vec<[f64; 2]>,
}

impl EarningsChart {
    pub fn new(history: &[Account], timestamps: &[i64]) -> Self {
        Self {
            earnings: series_from(history, timestamps, |a| a.earnings),
            fees: series_from(history, timestamps, |a| a.fee_spend),
        }
    }
}
