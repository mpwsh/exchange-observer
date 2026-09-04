//! Producer binary: streams OKX websocket data into Redpanda.

use std::{sync::Arc, time::Instant};

use anyhow::Context;
use exchange_observer::{AppConfig, ChannelSettings};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use rskafka::client::ClientBuilder;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::{
    error::{ProducerError, Result},
    mq::Clients,
    stats::{log_stats, update_partition_count, Cooldowns},
    ws::WsStream,
};

pub mod error;
pub mod mq;
pub mod stats;
pub mod ws;

/// How often the keep-alive ping fires.
const PING_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25_000);

/// Reconnect backoff when the WS connect step itself fails.
const RECONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::load().context("loading AppConfig")?;
    info!("Connecting to message queue at {} ...", cfg.mq.ip);

    let broker = format!("{}:{}", cfg.mq.ip, cfg.mq.port);
    let admin = Arc::new(
        ClientBuilder::new(vec![broker])
            .build()
            .await
            .context("connecting to redpanda")?,
    );
    let clients = Arc::new(Clients::new(admin));

    mq::create_topics(&clients.admin, &cfg)
        .await
        .context("creating topics")?;

    let channels = cfg
        .exchange
        .as_ref()
        .context("config missing [exchange] section")?
        .channels
        .clone();

    // Each channel gets its own WS connection. When one disconnects it sends
    // itself back through this channel and the main loop respawns it.
    let (disconnect_tx, mut disconnect_rx) =
        mpsc::channel::<ChannelSettings>(channels.len().max(1));

    for channel in &channels {
        tokio::spawn(handle_connection(
            clients.clone(),
            channel.clone(),
            cfg.clone(),
            disconnect_tx.clone(),
        ));
    }
    drop(disconnect_tx); // don't hold an extra ref that keeps the loop alive

    while let Some(dead) = disconnect_rx.recv().await {
        error!("Channel {:?} disconnected. Respawning...", dead.name);
        // Reuse the same channel spec; the receiver end holds the only tx now,
        // so we clone from a fresh handle we grab via the config.
        let (respawn_tx, mut respawn_rx) = mpsc::channel(1);
        tokio::spawn(handle_connection(
            clients.clone(),
            dead,
            cfg.clone(),
            respawn_tx,
        ));
        if let Some(dead_again) = respawn_rx.recv().await {
            error!("Respawned channel {:?} also died", dead_again.name);
        }
    }

    Ok(())
}

/// Owns one websocket connection's lifecycle: connect, run, reconnect on
/// failure. Only pushes to `disconnect_tx` when we give up trying.
async fn handle_connection(
    clients: Arc<Clients>,
    channel: ChannelSettings,
    cfg: AppConfig,
    disconnect_tx: mpsc::Sender<ChannelSettings>,
) {
    loop {
        match ws::connect_and_subscribe(&channel).await {
            Ok(ws_stream) => match run(clients.clone(), ws_stream, &cfg).await {
                Ok(()) => {
                    warn!("Channel {:?} exited cleanly, reconnecting", channel.name);
                }
                Err(ProducerError::Disconnected) => {
                    warn!("Channel {:?} disconnected, reconnecting", channel.name);
                }
                Err(e) => {
                    error!("Channel {:?} run failed: {e}", channel.name);
                    break;
                }
            },
            Err(e) => {
                error!(
                    "Failed to connect to channel {:?}: {e}. Backing off {:?}",
                    channel.name, RECONNECT_BACKOFF
                );
                tokio::time::sleep(RECONNECT_BACKOFF).await;
            }
        }
    }

    if let Err(e) = disconnect_tx.send(channel).await {
        error!("Disconnect notification failed: {e}");
    }
}

/// Consume messages from one websocket until it disconnects or errors.
///
/// Returns `Err(ProducerError::Disconnected)` on a clean close so the caller
/// can distinguish "reconnect" from "genuine failure".
async fn run(clients: Arc<Clients>, mut ws: WsStream, cfg: &AppConfig) -> Result<()> {
    let exchange = "Okx";
    let inc = Arc::new(Mutex::new(0i64));
    let (ping_tx, mut ping_rx) = watch::channel(false);
    let cooldowns = Cooldowns::default();
    let partition_count = Arc::new(Mutex::new(stats::initial_partition_map()));

    // Ping task. Exits gracefully on send failure — the reader will observe
    // the close and end the outer loop.
    let ping_handle = tokio::spawn(async move {
        while ping_rx.changed().await.is_ok() {
            info!("Sending keep-alive ping");
            if let Err(e) = ws.write.send(Message::Ping(Default::default())).await {
                warn!("Ping send failed, ending ping task: {e}");
                break;
            }
        }
    });

    // A plain `while let` loop instead of `for_each` — `for_each` gives us an
    // `FnMut` closure that can't let borrows of local state escape into the
    // async block. Flattening lets us mutate `disconnected` locally.
    let mut disconnected = false;
    while let Some(message) = ws.read.next().await {
        let start = Instant::now();

        // Fire a ping if the interval has elapsed. One lock per message.
        {
            let mut last = cooldowns.ping.lock().await;
            if last.elapsed() >= PING_INTERVAL {
                if let Err(e) = ping_tx.send(true) {
                    error!("Ping trigger send failed: {e}");
                } else {
                    *last = Instant::now();
                }
            }
        }

        let text = match message {
            Ok(Message::Text(t)) => t.to_string(),
            Ok(Message::Binary(b)) => match std::str::from_utf8(&b) {
                Ok(s) => s.to_owned(),
                Err(e) => {
                    warn!("Non-UTF-8 binary frame ({} bytes): {e}", b.len());
                    continue;
                }
            },
            Ok(Message::Pong(_)) => continue, // ack of our ping
            Ok(Message::Ping(_)) => continue, // auto-pong'd by tungstenite
            Ok(Message::Close(frame)) => {
                warn!("WebSocket closed by server: {frame:?}");
                disconnected = true;
                break;
            }
            Ok(Message::Frame(_)) => continue,
            Err(e) => {
                error!("Error receiving message: {e}");
                disconnected = true;
                break;
            }
        };

        let value = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => v,
            Err(e) => {
                warn!("Deserialization error: {e}. Payload: {text}");
                continue;
            }
        };

        if let Err(e) =
            ws::process_message(exchange, &partition_count, clients.clone(), &value).await
        {
            warn!("process_message failed: {e}");
        }

        update_partition_count(&cooldowns, &partition_count, cfg).await;
        log_stats(&cooldowns, &inc, start).await;

        *inc.lock().await += 1;
    }

    ping_handle.abort();

    if disconnected {
        Err(ProducerError::Disconnected)
    } else {
        Ok(())
    }
}
