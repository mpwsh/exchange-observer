use crate::app::{Account, Candlestick};
use eframe::egui::{plot, Color32, Stroke};

pub struct CandlestickBoxPlot {
    pub boxes: Vec<plot::BoxElem>,
}

impl CandlestickBoxPlot {
    pub fn new(candlesticks: &[Candlestick]) -> Self {
        // Sort candlesticks by timestamp
        let mut sorted_candlesticks = candlesticks.to_vec();
        sorted_candlesticks.sort_by(|a, b| a.ts.cmp(&b.ts));

        // Convert sorted candlesticks to box plot data

        let boxes: Vec<plot::BoxElem> = sorted_candlesticks
            .iter()
            .enumerate()
            .map(|(i, candlestick)| {
                let ts_secs = candlestick.ts.num_seconds() as f64;
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

                plot::BoxElem::new(
                    (i as f64) / 3.5,
                    plot::BoxSpread::new(
                        candlestick.low,
                        lower_quartile,
                        median,
                        upper_quartile,
                        candlestick.high,
                    ),
                )
                .name(format!(
                    "Time: {:.2}\nVol: {}\nChange: {}\nOpen: {}\nClose: {}",
                    ts_secs, candlestick.vol, candlestick.change, open, close
                ))
                .fill(color)
                .stroke(Stroke::new(1.0, color)) // Set color based on change
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

        let available_balance_line: Vec<[f64; 2]> = account_history
            .iter()
            .zip(timestamps.iter())
            .map(|(account, &ts)| [ts as f64, account.balance.available])
            .collect();

        Self {
            lines_values: vec![current_balance_line, available_balance_line],
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

        Self {
            lines_values: vec![earnings_line],
        }
    }
}
