//! Websocket layer: connect, subscribe, dispatch messages by channel.

use std::{collections::HashMap, str::FromStr, sync::Arc};

use crypto_market_type::MarketType;
use crypto_markets::fetch_symbols;
use exchange_observer::{models::*, ChannelSettings};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use log::{info, trace};
use native_tls::TlsConnector;
use serde_json::Value;
use tokio::{net::TcpStream, sync::Mutex, task};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::protocol::{Message, WebSocketConfig},
    Connector, MaybeTlsStream, WebSocketStream,
};

use crate::{
    error::{ProducerError, Result},
    mq::{send_message, Clients},
};

/// Max size of a single WebSocket frame. 4 KiB (the previous value) truncated
/// OKX ticker frames when many instruments batched into one message; 1 MiB is
/// well above what OKX sends while still bounding memory.
const WS_FRAME_SIZE: usize = 1024 * 1024;

/// Max size of a fully-assembled WebSocket message. Guardrail against memory
/// exhaustion from a misbehaving peer.
const WS_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// USDT market suffix used to filter the OKX symbol list.
const USDT_SUFFIX: &str = "-USDT";

pub struct WsStream {
    pub read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}

pub fn build_args(channel: Channel, pairs: &[String]) -> Vec<SubscribeArg> {
    pairs
        .iter()
        .map(|pair| match channel {
            Channel::Tickers | Channel::Candle1m => SubscribeArg {
                channel: channel.to_string(),
                inst_type: Some("SPOT".to_owned()),
                inst_id: Some(pair.clone()),
            },
            Channel::Trades | Channel::Books => SubscribeArg {
                channel: channel.to_string(),
                inst_type: None,
                inst_id: Some(pair.clone()),
            },
        })
        .collect()
}

/// Establish a TLS websocket connection to the given channel endpoint and send
/// the subscription frames.
pub async fn connect_and_subscribe(channel: &ChannelSettings) -> Result<WsStream> {
    // WebSocketConfig is #[non_exhaustive] in tungstenite >= 0.22, so build via
    // its builder methods.
    let ws_config = WebSocketConfig::default()
        .max_frame_size(Some(WS_FRAME_SIZE))
        .max_message_size(Some(WS_MESSAGE_SIZE));

    let url = url::Url::parse(&channel.endpoint)?;
    let tls = TlsConnector::new()?;

    let (ws_stream, _response) = connect_async_tls_with_config(
        url.as_str(),
        Some(ws_config),
        false,
        Some(Connector::NativeTls(tls)),
    )
    .await?;

    let (mut write, read) = ws_stream.split();

    // `fetch_symbols` does a blocking HTTP call; keep it off the runtime thread.
    let channel_name = channel.name.clone();
    let subscribe_msgs = task::spawn_blocking(move || build_subscribe(&channel_name)).await??;

    for msg in &subscribe_msgs {
        if let Some(first_arg) = msg.args.first() {
            info!(
                "Sending subscription to channel {} on endpoint {url}",
                first_arg.channel,
            );
        }
        let payload = serde_json::to_string(msg)? + "\n";
        write.send(Message::Text(payload.into())).await?;
    }

    Ok(WsStream { read, write })
}

pub fn build_subscribe(channel: &str) -> Result<Vec<SubscribeMsg>> {
    let symbols = filter_usdt(fetch_symbols("okx", MarketType::Spot)?);
    let channel =
        Channel::from_str(channel).map_err(|_| ProducerError::UnknownChannel(channel.to_owned()))?;
    info!("Building subscribe for channel {channel:?}");

    Ok(vec![SubscribeMsg {
        op: "subscribe".to_owned(),
        args: build_args(channel, &symbols),
    }])
}

/// Keep only USDT-quoted symbols.
fn filter_usdt(mut pairs: Vec<String>) -> Vec<String> {
    pairs.retain(|s| s.contains(USDT_SUFFIX));
    pairs
}

/// Route a decoded websocket message to the appropriate Kafka topic.
pub async fn process_message(
    exchange: &str,
    partition_count: &Mutex<HashMap<String, i32>>,
    clients: Arc<Clients>,
    res: &Value,
) -> Result<()> {
    // Log control events but not on the hot data path.
    if let Some(event) = res.get("event").and_then(Value::as_str) {
        if event == "subscribe" || event == "error" {
            info!("Event: {res}");
        }
    }

    // `arg.channel` identifies the stream. Missing or unknown → nothing to do.
    let Some(channel_str) = res
        .get("arg")
        .and_then(|a| a.get("channel"))
        .and_then(Value::as_str)
    else {
        trace!("Message without arg.channel: {res}");
        return Ok(());
    };

    let Ok(channel) = Channel::from_str(channel_str) else {
        trace!("Unrecognized channel: {channel_str}");
        return Ok(());
    };

    // No data payload (e.g. subscribe ack) → nothing to route.
    if res.get("data").is_none_or(Value::is_null) {
        return Ok(());
    }

    let inst_id_bytes = res
        .get("arg")
        .and_then(|a| a.get("instId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .as_bytes()
        .to_vec();

    send_message(
        exchange,
        channel,
        res,
        partition_count,
        clients,
        inst_id_bytes,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_usdt_keeps_only_usdt_pairs() {
        let input = vec![
            "BTC-USDT".to_owned(),
            "ETH-USDC".to_owned(),
            "SOL-USDT".to_owned(),
            "DOGE-BTC".to_owned(),
        ];
        assert_eq!(filter_usdt(input), vec!["BTC-USDT", "SOL-USDT"]);
    }

    #[test]
    fn build_args_sets_spot_only_for_tickers_and_candles() {
        let pairs = vec!["BTC-USDT".to_owned()];

        let tickers = build_args(Channel::Tickers, &pairs);
        assert_eq!(tickers[0].inst_type.as_deref(), Some("SPOT"));

        let trades = build_args(Channel::Trades, &pairs);
        assert!(trades[0].inst_type.is_none());
    }
}
