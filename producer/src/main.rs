use std::{collections::HashMap, sync::Arc, time::Instant};

use anyhow::Result;
use exchange_observer::{models::Channel, util::Elapsed, AppConfig, ChannelSettings};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use rskafka::client::{Client, ClientBuilder};
use serde_json::Value;
pub use stats::*;
use tokio::sync::{watch, Mutex};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::ws::WsStream;
pub mod mq;
pub mod stats;
pub mod ws;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg: AppConfig = AppConfig::load()?;
    info!("Connecting to message queue at: {} ...", cfg.mq.ip);

    // setup redpanda

    let client = Arc::new(
        ClientBuilder::new(vec![format!("{}:{}", cfg.mq.ip, cfg.mq.port)])
            .build()
            .await?,
    );

    mq::create_topics(&client, &cfg).await?;

    let channels = cfg.exchange.as_ref().unwrap().channels.clone();

    let (disconnect_tx, mut disconnect_rx) = tokio::sync::mpsc::channel(channels.len());

    for channel in &channels {
        let disconnect_tx = disconnect_tx.clone();
        let channel = channel.clone();
        let cfg = cfg.clone();

        tokio::spawn(handle_connection(
            client.clone(),
            channel,
            cfg,
            disconnect_tx,
        ));
    }

    while let Some(disconnected_channel) = disconnect_rx.recv().await {
        error!(
            "Channel {:?} disconnected. Trying to reconnect...",
            disconnected_channel
        );

        let disconnect_tx = disconnect_tx.clone();
        let cfg = cfg.clone();

        tokio::spawn(handle_connection(
            client.clone(),
            disconnected_channel,
            cfg,
            disconnect_tx,
        ));
    }

    Ok(())
}

async fn handle_connection(
    client: Arc<Client>,
    channel: ChannelSettings,
    cfg: AppConfig,
    disconnect_tx: tokio::sync::mpsc::Sender<ChannelSettings>,
) -> Result<()> {
    loop {
        match ws::connect_and_subscribe(channel.clone()).await {
            Ok(ws_stream) => {
                if run(client.clone(), ws_stream, &cfg).await.is_err() {
                    break;
                }
            },
            Err(e) => {
                error!("Failed to connect to channel {:?}: {:?}", channel, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            },
        }
    }
    disconnect_tx.send(channel).await?;

    Ok(())
}

async fn run(client: Arc<Client>, mut ws: WsStream, cfg: &AppConfig) -> Result<()> {
    let exchange = String::from("Okx");
    let inc = Arc::new(Mutex::new(0));
    let (tx, mut rx) = watch::channel(false);
    let cooldowns = Cooldowns::default();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            info!("Sending keep-alive ping");
            ws.write
                .send(Message::Ping(Vec::new()))
                .await
                .expect("Failed to send message");
        }
    });
    let partition_count = Arc::new(Mutex::new(HashMap::from([
        (Channel::Candle1m.to_string(), 0),
        (Channel::Tickers.to_string(), 0),
        (Channel::Books.to_string(), 0),
        (Channel::Trades.to_string(), 0),
    ])));

    //Send received websocket messages to corresponding queues
    let read_future = ws.read.for_each(|message| async {
        let start = Instant::now();
        let data = match message {
            Ok(m) => m.into_data(),
            Err(e) => {
                error!("Error while receiving message: {e}");
                Vec::new()
            },
        };

        if cooldowns.ping.lock().await.elapsed().as_millis() >= 25000 {
            if let Err(e) = tx.send(true) {
                error!("{}", e);
            } else {
                *cooldowns.ping.lock().await = Instant::now();
            }
        };

        match serde_json::from_str::<Value>(&String::from_utf8_lossy(&data)) {
            Ok(res) => {
                ws::process_message(&exchange, &partition_count, client.clone(), &res)
                    .await
                    .unwrap();
            },
            Err(e) => {
                warn!("Deserialization error: {}", e);
                warn!("{:?} No data?", data);
            },
        }

        update_partition_count(&cooldowns, &partition_count, cfg).await;
        log_stats(&cooldowns, &inc, start).await;

        {
            let mut counter = inc.lock().await;
            *counter += 1;
        }
    });
    read_future.await;
    Ok(())
}
