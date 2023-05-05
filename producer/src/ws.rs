use crate::models::*;
use crate::util::parse_symbols;
use crate::Result;
use crypto_market_type::MarketType;
use crypto_markets::fetch_symbols;
use log::info;
use native_tls::TlsConnector;
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::protocol::WebSocketConfig, Connector,
    WebSocketStream,
};
//const UPLINK_LIMIT: (NonZeroU32, std::time::Duration) =
//    (nonzero!(240u32), std::time::Duration::from_secs(3600));
const WS_FRAME_SIZE: usize = 4096;
const WEBSOCKET_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

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
//Websocket
//pub async fn connect() -> Result<WebSocketStream<S>> {
pub async fn connect(
) -> Result<WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, anyhow::Error>
{
    let url = url::Url::parse(WEBSOCKET_URL)?;
    let ws_config = WebSocketConfig {
        max_frame_size: Some(WS_FRAME_SIZE),
        ..Default::default()
    };
    info!("Retrieving websocket data from: {}", url);
    let client = connect_async_tls_with_config(
        url,
        Some(ws_config),
        Some(Connector::NativeTls(TlsConnector::new()?)),
    )
    .await?;
    Ok(client.0)
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
