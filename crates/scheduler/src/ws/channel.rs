//! Websocket outbound: serialize scheduler state and closed reports and
//! push them to the console(s).
//!
//! Two variants on the channel:
//! - `State`: per-cycle account + portfolio snapshot. Existing wire shape
//!   preserved bit-exact — the account and portfolio JSON envelopes on the
//!   client haven't changed.
//! - `Report`: one closed position, emitted once when the scheduler saves
//!   the report to Scylla. New channel `report` on the console side.

use serde_json::json;
use tokio::sync::mpsc;

use super::server::WebSocket;
use crate::prelude::*;

/// Snapshot of the current cycle's account + portfolio.
pub struct StateSnapshot {
    pub balance: Balance,
    pub change: f32,
    pub earnings: f64,
    pub fee_spend: f64,
    pub tokens: Vec<Token>,
    pub ts: DateTime<Utc>,
}

/// One closed position, sent as it's saved to Scylla.
pub struct ReportEvent {
    pub report: Report,
    pub instid: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub ts: DateTime<Utc>,
}

/// Messages flowing from the main loop into the WS transmit task. The
/// existing `State` cadence (once per ~300ms tick) is unchanged; `Report`
/// events fire ad-hoc when positions close.
pub enum Data {
    State(StateSnapshot),
    Report(ReportEvent),
}

pub async fn transmit(server: WebSocket, mut receiver: mpsc::Receiver<Data>) -> Result<()> {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(300));
    // Only state messages get batched to the tick; report events go out
    // immediately (they're low-volume and the delay would obscure the
    // "trade just closed" feel on the console).
    let mut state_buffer: Vec<StateSnapshot> = Vec::new();

    loop {
        tokio::select! {
            Some(data) = receiver.recv() => match data {
                Data::State(snap) => state_buffer.push(snap),
                Data::Report(event) => emit_report(&server, event).await,
            },
            _ = interval.tick() => {
                for snap in state_buffer.drain(..) {
                    emit_state(&server, snap).await;
                }
            },
        }
    }
}

async fn emit_state(server: &WebSocket, data: StateSnapshot) {
    let balance = &data.balance;
    let tokens = &data.tokens;

    let token_balances: f64 = tokens.iter().map(|t| t.balance.available * t.price).sum();
    let ts = &data.ts;
    let mut open_order_value: f64 = 0.0;

    for t in tokens.iter() {
        if let Some(orders) = &t.orders {
            for order in orders {
                let (price, size) = match (order.px.parse::<f64>(), order.sz.parse::<f64>()) {
                    (Ok(price), Ok(size)) => (price, size),
                    _ => continue,
                };
                let usdt_taker_fee = calculate_fees(balance.spendable, 0.10);
                let order_amount = (size * price) + usdt_taker_fee;
                if order.state == OrderState::Live {
                    match order.side {
                        Side::Buy => open_order_value += order_amount,
                        Side::Sell => open_order_value += size * price,
                    }
                };
            }
        }
    }

    server
        .send(
            json!({
                "channel": "account",
                "data": json!({
                    "balance": balance,
                    "token_balance": token_balances,
                    "open_orders": open_order_value,
                    "earnings": &data.earnings,
                    "change": &data.change,
                    "fee_spend": &data.fee_spend,
                }).to_string(),
                "ts": ts,
            })
            .to_string(),
        )
        .await;

    server
        .send(
            json!({
                "channel": "portfolio",
                "data": serde_json::to_string(&tokens).unwrap(),
                "ts": ts,
            })
            .to_string(),
        )
        .await;
}

async fn emit_report(server: &WebSocket, event: ReportEvent) {
    // Flatten so the console can consume without knowing about the Report
    // struct's internal shape. All fields exposed here are already columns
    // in `okx.reports`, so semantics are stable.
    server
        .send(
            json!({
                "channel": "report",
                "data": json!({
                    "round_id":   event.report.round_id,
                    "instid":     event.instid,
                    "reason":     event.report.reason,
                    "earnings":   event.report.earnings,
                    "fees":       event.report.fees,
                    "change":     event.report.change,
                    "time_left":  event.report.time_left,
                    "highest":    event.report.highest,
                    "lowest":     event.report.lowest,
                    "buy_price":  event.buy_price,
                    "sell_price": event.sell_price,
                    "strategy":   event.report.strategy,
                }).to_string(),
                "ts": event.ts,
            })
            .to_string(),
        )
        .await;
}
