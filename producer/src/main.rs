#![feature(async_closure)]
use anyhow::Result;
use crypto_market_type::MarketType;
use crypto_markets::fetch_symbols;
use exchange_observer::{AppConfig, models::*, util::Elapsed};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use log::{debug, error, info, warn};
use native_tls::TlsConnector;
use rskafka::{
    client::{partition::Compression, Client, ClientBuilder},
    record::Record,
    //record::RecordAndOffset,
    topic::Topic,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    env,
    //   num::NonZeroU32,
    time::Instant,
};
//use nonzero_ext::nonzero;
use time::OffsetDateTime;
use tokio::sync::watch;
use tokio::{net::TcpStream, task};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, Connector, MaybeTlsStream, WebSocketStream,
};

//const UPLINK_LIMIT: (NonZeroU32, std::time::Duration) =
//    (nonzero!(240u32), std::time::Duration::from_secs(3600));
const WS_FRAME_SIZE: usize = 4096;
const WEBSOCKET_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

pub struct Cooldowns {
    stats: RefCell<Instant>,
    ping: RefCell<Instant>,
    partition_change: RefCell<Instant>,
}
impl Default for Cooldowns {
    fn default() -> Self {
        Self {
            ping: RefCell::new(Instant::now()),
            stats: RefCell::new(Instant::now()),
            partition_change: RefCell::new(Instant::now()),
        }
    }
}
pub struct WsStream {
    read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = env::current_dir()?;
    let config_path = env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let cfg: AppConfig = AppConfig::load(&config_path);
    debug!("The current directory is {}", path.display());
    debug!("config loaded: {:#?}", cfg);
    info!("Connecting to message queue at: {} ...", cfg.mq.ip);

    //setup redpanda
    let client = ClientBuilder::new(vec![format!("{}:{}", cfg.mq.ip, cfg.mq.port)])
        .build()
        .await?;
    create_topics(&client, &cfg).await?;
    let not = false;
    let mut errors = 0;
    loop {
        let ws_stream = connect_and_subscribe(&cfg).await?;
        match run(&client, ws_stream, &cfg).await {
            Ok(k) => info!("Closing OK?: {:?}", k),
            Err(e) => error!("Connection closed due to {}. Trying to reconnect", e),
        }
        errors += errors;
        error!("Disconnect count: {errors}");
        if not {
            break;
        }
    }
    Ok(())
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
                .send(Message::Text(String::from("ping")))
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
        //Send ping every 25 secs to keep connection alive
        if cooldowns.ping.borrow().elapsed().as_millis() >= 25000 {
            if let Err(e) = tx.send(true) {
                error!("{}", e);
            } else {
                cooldowns.ping.replace(Instant::now());
            }
        };

        if let Ok(res) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&data)) {
            process_message(&exchange, &partition_count, client, &res)
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

async fn produce(topic: &str, partition: i32, client: &Client, record: Record) -> Result<()> {
    let partition_client = client
        .partition_client(topic.to_owned(), partition)
        .unwrap();
    partition_client
        .produce(vec![record], Compression::Lz4)
        .await
        .unwrap();

    Ok(())
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

fn build_subscribe(channels: Vec<String>) -> Result<String> {
    let symbols =
        parse_symbols(fetch_symbols("okx", MarketType::Spot).expect("Unable to connect to okx"));
    let args_ws = build_ws_args(channels, symbols);
    let subscribe_msg = SubscribeMsg {
        op: String::from("subscribe"),
        args: args_ws,
    };
    Ok(serde_json::to_string(&subscribe_msg)?)
}
fn build_ws_args(channels: Vec<String>, pairs: Vec<String>) -> Vec<SubArg> {
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

fn build_record(
    exchange: &str,
    channel: &str,
    inst_id: &[u8],
    data: &str,
    partition: String,
) -> Record {
    Record {
        key: Some(inst_id.to_vec()),
        value: Some(data.as_bytes().to_vec()),
        headers: BTreeMap::from([
            ("Exchange".to_owned(), exchange.as_bytes().to_vec()),
            ("Channel".to_owned(), channel.as_bytes().to_vec()),
            ("Partition".to_owned(), partition.as_bytes().to_vec()),
        ]),
        timestamp: OffsetDateTime::now_utc(),
    }
}

async fn connect_and_subscribe(cfg: &AppConfig) -> Result<WsStream> {
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

async fn create_topics(client: &Client, cfg: &AppConfig) -> Result<()> {
    let list_topics = client.list_topics().await?;
    info!("found topics: {:?}", list_topics);
    for topic in cfg.mq.topics.iter() {
        let found: Option<&Topic> = list_topics.iter().find(|t| t.name == *topic.name);
        if found.is_none() {
            warn!("Topic {} doesn't exist. Creating with {} partitions, timeout {}ms, replication_factor: {}", topic.name, topic.partitions, topic.max_wait_ms, topic.replication_factor);
            //create topic
            let controller_client = client.controller_client().unwrap();
            controller_client
                .create_topic(
                    &topic.name,
                    topic.partitions,
                    topic.replication_factor,
                    topic.max_wait_ms,
                )
                .await
                .unwrap()
        }
    }
    Ok(())
}
async fn send_message(
    exchange: &str,
    channel: &str,
    data: &Value,
    partition_count: &RefCell<HashMap<String, i32>>,
    client: &Client,
    inst_id_bytes: Vec<u8>,
) -> Result<()> {
    let data = match channel {
        "tickers" => serde_json::to_string(&serde_json::from_str::<Ticker>(
            &data["data"][0].to_string(),
        )?)?,
        "trades" => serde_json::to_string(&serde_json::from_str::<Trade>(
            &data["data"][0].to_string(),
        )?)?,
        "candle1m" => {
            serde_json::to_string(&Candlestick::from_candle(data).get_change().get_range())?
        }
        _ => {
            error!("Unknown channel received: {}", channel);
            String::new()
        }
    };

    let p = {
        let map = partition_count.borrow();
        *map.get(&channel.to_string()).unwrap()
    };

    //Save the partition in a header (dont know how to retrieve afterwards without this)
    let record = build_record(exchange, channel, &inst_id_bytes, &data, p.to_string());
    produce(channel, p, client, record)
        .await
        .expect("failed to produce message");
    Ok(())
}

async fn process_message(
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
async fn update_partition_count(
    cooldowns: &Cooldowns,
    partition_count: &RefCell<HashMap<String, i32>>,
    cfg: &AppConfig,
) {
    if cooldowns.partition_change.borrow().elapsed().as_millis() >= 100 {
        for topic in cfg.mq.topics.iter() {
            let mut map = partition_count.borrow_mut();
            let current_slot = map.get(&topic.name).unwrap();
            if current_slot < &(topic.partitions - 1) {
                if let Some(x) = map.get_mut(&topic.name) {
                    *x += 1;
                }
            } else if let Some(x) = map.get_mut(&topic.name) {
                *x = 0;
            }
        }
    };
}

async fn log_stats(cooldowns: &Cooldowns, inc: &RefCell<i32>, start: Instant) {
    if cooldowns.stats.borrow().elapsed().as_millis() >= 5000 {
        let ack_rate = inc.clone().into_inner() / 5;
        info!("Latency: {}", Elapsed::from(&start));
        info!("inc rate: {} messages/s", ack_rate);
        inc.replace(0);
        cooldowns.stats.replace(Instant::now());
    };
}
