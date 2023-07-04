use crate::{prelude::*, server::WebSocket};
use serde_json::json;
use tokio::sync::mpsc;

pub struct WsData {
    pub account: Account,
    pub tokens: Vec<Token>,
    pub ts: DateTime<Utc>,
}

pub async fn transmit(server: WebSocket, mut receiver: mpsc::Receiver<WsData>) -> Result<()> {
    while let Some(data) = receiver.recv().await {
        let account = &data.account;
        let tokens = &data.tokens;
        let ts = &data.ts;
        server
            .send(
                json!({
                "channel": "account",
                "data": json!({
                    "balance": account.balance,
                    "earnings": account.earnings,
                    "change": account.change,
                }).to_string(),
                "ts": ts
                })
                .to_string(),
            )
            .await;

        if !account.portfolio.is_empty() {
            let data = serde_json::to_string(&account.portfolio).unwrap();
            server
                .send(
                    json!({
                    "channel": "portfolio",
                    "data": data,
                    "ts": ts
                    })
                    .to_string(),
                )
                .await;
        }
        if !tokens.is_empty() {
            let data = serde_json::to_string(&tokens).unwrap();
            server
                .send(
                    json!({
                    "channel": "tokens",
                    "data": data,
                    "ts": ts
                    })
                    .to_string(),
                )
                .await;
        }
    }
    Ok(())
}
