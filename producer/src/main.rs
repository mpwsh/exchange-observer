use crate::ws::WsStream;
use anyhow::Result;
use exchange_observer::{util::Elapsed, AppConfig};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use rskafka::client::{Client, ClientBuilder};
use serde_json::Value;
pub use stats::*;
use std::{cell::RefCell, collections::HashMap, time::Instant};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::protocol::Message;
pub mod mq;
pub mod stats;
pub mod ws;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg: AppConfig = AppConfig::load()?;
    info!("Connecting to message queue at: {} ...", cfg.mq.ip);

    //setup redpanda
    let client = ClientBuilder::new(vec![format!("{}:{}", cfg.mq.ip, cfg.mq.port)])
        .build()
        .await?;
    mq::create_topics(&client, &cfg).await?;

    let mut errors = 0;
    loop {
        let ws_stream = ws::connect_and_subscribe(&cfg).await?;
        match run(&client, ws_stream, &cfg).await {
            Ok(k) => info!("Closing OK?: {:?}", k),
            Err(e) => error!("Connection closed due to {}. Trying to reconnect", e),
        }
        errors += 1;
        error!("Disconnect count: {errors}");
    }
}

async fn run(client: &Client, mut ws: WsStream, cfg: &AppConfig) -> Result<()> {
    let exchange = String::from("Okx");
    let inc = RefCell::new(0);
    let (tx, mut rx) = watch::channel(false);
    let cooldowns = Cooldowns::default();
    //Send ping on channel change (every 25 secs)
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            info!("Sending keep-alive ping");
            ws.write
                .send(Message::Ping(Vec::new()))
                .await
                .expect("Failed to send message");
        }
    });
    let partition_count = RefCell::new(HashMap::from([
        ("candle1m".to_string(), 0),
        ("tickers".to_string(), 0),
        ("trades".to_string(), 0),
    ]));

    //Send received websocket messages to corresponding queues
    let read_future = ws.read.for_each(|message| async {
        let start = Instant::now();
        let data = match message {
            Ok(m) => m.into_data(),
            Err(e) => {
                error!("Error while receiving message: {e}");
                Vec::new()
            }
        };

        if cooldowns.ping.borrow().elapsed().as_millis() >= 25000 {
            if let Err(e) = tx.send(true) {
                error!("{}", e);
            } else {
                cooldowns.ping.replace(Instant::now());
            }
        };

        if let Ok(res) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&data)) {
            ws::process_message(&exchange, &partition_count, client, &res)
                .await
                .unwrap();
        }

        update_partition_count(&cooldowns, &partition_count, cfg).await;
        log_stats(&cooldowns, &inc, start).await;

        inc.replace_with(|&mut old| old + 1);
    });
    read_future.await;
    Ok(())
}
