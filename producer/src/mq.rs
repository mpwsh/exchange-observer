use std::collections::{BTreeMap, HashMap};

use exchange_observer::{models::*, AppConfig};
use rskafka::{
    client::{partition::Compression, Client},
    record::Record,
};
use time::OffsetDateTime;

use crate::{info, warn, Arc, Mutex, Result, Value};

pub async fn produce(
    topic: Channel,
    partition: i32,
    client: Arc<Client>,
    record: Record,
) -> Result<()> {
    let partition_client = client.partition_client(topic.to_string(), partition)?;
    partition_client
        .produce(vec![record], Compression::Lz4)
        .await?;

    Ok(())
}

pub fn build_record(
    exchange: &str,
    channel: Channel,
    inst_id: &[u8],
    data: &str,
    partition: String,
) -> Record {
    Record {
        key: Some(inst_id.to_vec()),
        value: Some(data.as_bytes().to_vec()),
        headers: BTreeMap::from([
            ("Exchange".to_owned(), exchange.as_bytes().to_vec()),
            (
                "Channel".to_owned(),
                channel.to_string().as_bytes().to_vec(),
            ),
            ("Partition".to_owned(), partition.as_bytes().to_vec()),
        ]),
        timestamp: OffsetDateTime::now_utc(),
    }
}

pub async fn create_topics(client: &Client, cfg: &AppConfig) -> Result<()> {
    let list = client.list_topics().await?;
    info!("Topic list: {:?}", list);
    for t in cfg.mq.topics.iter() {
        if !list.iter().any(|lt| lt.name == *t.name) {
            warn!(
                "Topic {} doesn't exist. Creating with {} partitions, timeout {}ms, replication_factor: {}",
                t.name, t.partitions, t.max_wait_ms, t.replication_factor
            );
            //create topic
            let controller_client = client.controller_client()?;
            controller_client
                .create_topic(&t.name, t.partitions, t.replication_factor, t.max_wait_ms)
                .await?
        }
    }
    Ok(())
}

pub async fn send_message(
    exchange: &str,
    channel: Channel,
    data: &Value,
    partition_count: &Mutex<HashMap<String, i32>>,
    client: Arc<Client>,
    inst_id_bytes: Vec<u8>,
) -> Result<()> {
    let data = match channel {
        Channel::Tickers => serde_json::to_string(&serde_json::from_str::<Ticker>(
            &data["data"][0].to_string(),
        )?)?,
        Channel::Trades => serde_json::to_string(&serde_json::from_str::<Trade>(
            &data["data"][0].to_string(),
        )?)?,
        Channel::Books => {
            serde_json::to_string(&serde_json::from_str::<Book>(&data["data"][0].to_string())?)?
        },
        Channel::Candle1m => {
            serde_json::to_string(&Candlestick::from_candle(data).get_change().get_range())?
        },
    };

    let p = {
        let map = partition_count.lock().await;
        *map.get(&channel.to_string()).unwrap()
    };

    //Save the partition in a header (dont know how to retrieve afterwards without this)
    let record = build_record(exchange, channel, &inst_id_bytes, &data, p.to_string());
    produce(channel, p, client, record)
        .await
        .expect("failed to produce message");
    Ok(())
}
