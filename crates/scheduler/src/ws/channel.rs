use serde_json::json;
use tokio::sync::mpsc;

use super::server::WebSocket;
use crate::prelude::*;

pub struct Data {
    pub balance: Balance,
    pub change: f32,
    pub earnings: f64,
    pub fee_spend: f64,
    pub tokens: Vec<Token>,
    pub ts: DateTime<Utc>,
}

pub async fn transmit(server: WebSocket, mut receiver: mpsc::Receiver<Data>) -> Result<()> {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(300));
    let mut buffer = Vec::new();
    loop {
        tokio::select! {
                Some(data) = receiver.recv() => {
                    buffer.push(data);
                },
                _ = interval.tick() => {
                    for data in buffer.drain(..) {
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
                        _ => {
                            continue;
                        }
                    };

                let usdt_taker_fee = calculate_fees(balance.spendable, 0.10);
                let order_amount = (size * price) + usdt_taker_fee;
                if order.state == OrderState::Live { match order.side {
                            Side::Buy => {
                                open_order_value += order_amount;
                            }
                            Side::Sell => {
                                open_order_value += size * price;
                            }
                        }
                };
            }}};
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
                    "ts": ts
                    })
                    .to_string(),
                )
                .await;

                server
                    .send(
                        json!({
                        "channel": "portfolio",
                        "data": serde_json::to_string(&tokens).unwrap(),
                        "ts": ts
                        })
                        .to_string(),
                    )
                    .await;
        }}}
        if false {
            break;
        }
    }
    Ok(())
}
