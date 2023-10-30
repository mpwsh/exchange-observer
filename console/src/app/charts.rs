use eframe::egui::{
    plot::{BoxElem, BoxSpread},
    Color32, Stroke,
};

use super::Duration;
use crate::app::{Account, Candlestick};

pub struct CandlestickBoxPlot {
    pub boxes: Vec<BoxElem>,
}

impl CandlestickBoxPlot {
    pub fn new(candlesticks: &[Candlestick], buy_ts: Duration, buy_price: f64) -> Self {
        // buy ts // buy price candlestick
        let buy_candle = Candlestick {
            instid: candlesticks.last().unwrap().instid.clone(),
            open: buy_price,
            high: buy_price,
            low: buy_price,
            close: buy_price,
            change: 0.0,
            range: 0.0,
            vol: 123.0,
            ts: buy_ts,
        };

        // Sort candlesticks by timestamp
        let mut sorted_candlesticks = candlesticks.to_vec();
        sorted_candlesticks.push(buy_candle);
        sorted_candlesticks.sort_by(|a, b| a.ts.cmp(&b.ts));

        let boxes: Vec<BoxElem> = sorted_candlesticks
            .iter()
            .map(|candlestick| {
                let ts_secs = candlestick.ts.num_seconds();
                let open = candlestick.open;
                let close = candlestick.close;
                let lower_quartile = if open < close { open } else { close };
                let upper_quartile = if close > open { close } else { open };
                let median = (open + close) / 2.0;

                let color = if open < close {
                    Color32::DARK_GREEN // Positive change, use green color
                } else {
                    Color32::DARK_RED // Negative change, use red color
                };
                let (legend, color) = if candlestick.vol == 123.0 {
                    (
                        format!("Time: {}\nBuy price: {}", buy_ts.num_seconds(), buy_price),
                        Color32::YELLOW,
                    )
                } else {
                    (
                        format!(
                            "Time: {}\nVol: {}\nChange: {}\nOpen: {}\nClose: {}",
                            ts_secs, candlestick.vol, candlestick.change, open, close
                        ),
                        color,
                    )
                };

                BoxElem::new(
                    ts_secs as f64,
                    BoxSpread::new(
                        candlestick.low,
                        lower_quartile,
                        median,
                        upper_quartile,
                        candlestick.high,
                    ),
                )
                .name(legend)
                .fill(color)
                .box_width(30.0)
                .whisker_width(0.1)
                .stroke(Stroke::new(1.0, color))
            })
            .collect();
        Self { boxes }
    }
}

pub struct BalanceChart {
    pub lines_values: Vec<Vec<[f64; 2]>>,
}

impl BalanceChart {
    pub fn new(account_history: &[Account], timestamps: &[i64]) -> Self {
        let current_balance_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.balance.current])
            .collect();

        let token_balance_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.token_balance])
            .collect();

        let open_orders_balance_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.open_orders])
            .collect();

        let available_balance_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.balance.available])
            .collect();

        Self {
            lines_values: vec![
                current_balance_line,
                token_balance_line,
                open_orders_balance_line,
                available_balance_line,
            ],
        }
    }
}

pub struct ChangeChart {
    pub lines_values: Vec<Vec<[f64; 2]>>,
}

impl ChangeChart {
    pub fn new(account_history: &[Account], timestamps: &[i64]) -> Self {
        let change_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.change])
            .collect();

        Self {
            lines_values: vec![change_line],
        }
    }
}

pub struct EarningsChart {
    pub lines_values: Vec<Vec<[f64; 2]>>,
}

impl EarningsChart {
    pub fn new(account_history: &[Account], timestamps: &[i64]) -> Self {
        let earnings_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.earnings])
            .collect();

        let fees_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.fee_spend])
            .collect();

        Self {
            lines_values: vec![earnings_line, fees_line],
        }
    }
}
