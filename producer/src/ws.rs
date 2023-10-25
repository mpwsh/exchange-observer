use std::{collections::HashMap, str::FromStr, sync::Arc};

use crypto_market_type::MarketType;
use crypto_markets::fetch_symbols;
use exchange_observer::{models::*, ChannelSettings};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use log::info;
use native_tls::TlsConnector;
use serde_json::{json, Value};
use tokio::{net::TcpStream, sync::Mutex, task};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::protocol::{Message, WebSocketConfig},
    Connector, MaybeTlsStream, WebSocketStream,
};

use crate::{mq::send_message, Client, Result};

//const UPLINK_LIMIT: (NonZeroU32, std::time::Duration) =
//    (nonzero!(240u32), std::time::Duration::from_secs(3600));
const WS_FRAME_SIZE: usize = 4096;

pub struct WsStream {
    pub read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}

pub fn build_args(channel: Channel, pairs: &[String]) -> Vec<SubscribeArg> {
    let mut args: Vec<SubscribeArg> = Vec::new();
    for i in pairs.iter() {
        let arg = match channel {
            Channel::Tickers | Channel::Candle1m => SubscribeArg {
                channel: channel.to_string(),
                inst_type: Some("SPOT".to_string()),
                inst_id: Some(i.to_string()),
            },
            Channel::Trades | Channel::Books => SubscribeArg {
                channel: channel.to_string(),
                inst_type: None,
                inst_id: Some(i.to_string()),
            },
        };
        args.push(arg);
    }
    args
}

pub async fn connect_and_subscribe(channel: ChannelSettings) -> Result<WsStream> {
    let ws_config = WebSocketConfig {
        max_frame_size: Some(WS_FRAME_SIZE),
        ..Default::default()
    };

    let url = url::Url::parse(&channel.endpoint)?;

    let (ws_stream, _response) = connect_async_tls_with_config(
        &url,
        Some(ws_config),
        Some(Connector::NativeTls(TlsConnector::new()?)),
    )
    .await?;

    let (mut write, read) = ws_stream.split();

    let subscribe_msgs =
        task::spawn_blocking(move || build_subscribe(channel.name.clone())).await??;
    for msg in subscribe_msgs {
        info!(
            "Sending subscription to channel {} on endpoint {}",
            msg.args[0].channel, url,
        );
        let msg_cr = serde_json::to_string(&msg)? + "\n";
        write.send(Message::Text(msg_cr)).await?;
    }

    Ok(WsStream { read, write })
}

pub fn build_subscribe(channel: String) -> Result<Vec<SubscribeMsg>> {
    let mut msgs = Vec::new();
    let symbols = parse_symbols(fetch_symbols("okx", MarketType::Spot)?);
    let channel = Channel::from_str(&channel).unwrap();
    info!("Building subscribe for channel {:?}", channel);
    let args_ws = build_args(channel, &symbols);
    let subscribe_msg = SubscribeMsg {
        op: String::from("subscribe"),
        args: args_ws,
    };
    msgs.push(subscribe_msg);
    Ok(msgs)
}

fn parse_symbols(mut pairs: Vec<String>) -> Vec<String> {
    let mut index = 0;
    pairs.retain(|x| {
        let keep = x.contains("-USDT");
        index += 1;
        keep
    });
    pairs
}

pub async fn process_message(
    exchange: &str,
    partition_count: &Mutex<HashMap<String, i32>>,
    client: Arc<Client>,
    res: &Value,
) -> Result<()> {
    let msg_str =
        serde_json::to_string_pretty(&res).expect("Unable to parse message from Websocket");
    if msg_str.to_lowercase().contains("ping") || msg_str.to_lowercase().contains("pong") {
        info!("Got: {}", msg_str);
    };
    let inst_id = res["arg"]["instId"].clone();
    let inst_id_bytes = format!("{}", inst_id).replace('\"', "").as_bytes().to_vec();
    let chan = Channel::from_str(res["arg"]["channel"].as_str().unwrap_or_default());

    if let Ok(channel) = chan {
        if res["data"] != json!(null) {
            match channel {
                Channel::Tickers => {
                    send_message(
                        exchange,
                        channel,
                        res,
                        partition_count,
                        client,
                        inst_id_bytes,
                    )
                    .await
                },
                Channel::Trades => {
                    send_message(
                        exchange,
                        channel,
                        res,
                        partition_count,
                        client,
                        inst_id_bytes,
                    )
                    .await
                },
                Channel::Books => {
                    send_message(
                        exchange,
                        channel,
                        res,
                        partition_count,
                        client,
                        inst_id_bytes,
                    )
                    .await
                },
                Channel::Candle1m => {
                    send_message(
                        exchange,
                        channel,
                        res,
                        partition_count,
                        client,
                        inst_id_bytes,
                    )
                    .await
                },
            }?;
        };
    } else {
        info!("Nothing to do with channel: {chan:?}");
    }
    Ok(())
}
