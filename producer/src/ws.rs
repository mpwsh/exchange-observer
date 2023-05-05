use exchange_observer::{AppConfig, models::*};
use tokio::{net::TcpStream, task};
use crate::{
    Client,
    mq::send_message,
    Result,
    RefCell,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use crypto_market_type::MarketType;
use crypto_markets::fetch_symbols;
use log::info;
use native_tls::TlsConnector;
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, Connector, MaybeTlsStream, WebSocketStream,
};

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};

//const UPLINK_LIMIT: (NonZeroU32, std::time::Duration) =
//    (nonzero!(240u32), std::time::Duration::from_secs(3600));
const WS_FRAME_SIZE: usize = 4096;
const WEBSOCKET_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

pub struct WsStream {
    pub read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}


pub fn build_args(channels: Vec<String>, pairs: Vec<String>) -> Vec<SubArg> {
    let mut args: Vec<SubArg> = Vec::new();
    for channel in channels.iter() {
        for i in pairs.iter() {
            let arg = SubArg {
                channel: channel.to_string(),
                inst_type: "SPOT".to_string(),
                inst_id: Some(i.to_string()),
            };
            args.push(arg);
        }
    }
    args
}

pub async fn connect_and_subscribe(cfg: &AppConfig) -> Result<WsStream> {
    //Websocket
    let ws_config = WebSocketConfig {
        max_frame_size: Some(WS_FRAME_SIZE),
        ..Default::default()
    };
    let url = url::Url::parse(WEBSOCKET_URL)?;
    info!("Retrieving websocket data from: {}", url);
    let (ws_stream, _response) = connect_async_tls_with_config(
        url,
        Some(ws_config),
        //Some(WebSocketConfig::default()),
        Some(Connector::NativeTls(TlsConnector::new()?)),
    )
    .await?;
    info!("WebSocket handshake has been successfully completed");
    let (mut write, read) = ws_stream.split();

    //send subscribe msg to exchange
    let topic_names: Vec<String> = cfg.mq.topics.clone().into_iter().map(|t| t.name).collect();
    let subscribe_msg = task::spawn_blocking(move || build_subscribe(topic_names)).await?;
    write.send(Message::Text(subscribe_msg? + "\n")).await?;

    info!("sent subscription");
    Ok(WsStream { read, write })
}

pub fn build_subscribe(channels: Vec<String>) -> Result<String> {
    let symbols = parse_symbols(fetch_symbols("okx", MarketType::Spot)?);
    let args_ws = build_args(channels, symbols);
    let subscribe_msg = SubscribeMsg {
        op: String::from("subscribe"),
        args: args_ws,
    };
    Ok(serde_json::to_string(&subscribe_msg)?)
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
    partition_count: &RefCell<HashMap<String, i32>>,
    client: &Client,
    res: &Value,
) -> Result<()> {
    let msg_str =
        serde_json::to_string_pretty(&res).expect("Unable to parse message from Websocket");
    if msg_str.to_lowercase().contains("ping") || msg_str.to_lowercase().contains("pong") {
        info!("Got: {}", msg_str);
    };
    let inst_id = res["arg"]["instId"].clone();
    let inst_id_bytes = format!("{}", inst_id).replace('\"', "").as_bytes().to_vec();
    if res["data"] != json!(null) {
        match res["arg"]["channel"].as_str() {
            Some("tickers") => {
                send_message(
                    exchange,
                    "tickers",
                    res,
                    partition_count,
                    client,
                    inst_id_bytes,
                )
                .await
            }
            Some("trades") => {
                send_message(
                    exchange,
                    "trades",
                    res,
                    partition_count,
                    client,
                    inst_id_bytes,
                )
                .await
            }
            Some("candle1m") => {
                send_message(
                    exchange,
                    "candle1m",
                    res,
                    partition_count,
                    client,
                    inst_id_bytes,
                )
                .await
            }
            _ => {
                info!("{:?}", &res["data"].to_string());
                info!("Nothing to do");
                Ok(())
            }
        }?;
    };
    Ok(())
}
