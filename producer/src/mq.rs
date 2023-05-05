use crate::{Result, Value, error, RefCell, info, warn};
use exchange_observer::{models::*, AppConfig};
use rskafka::{
    client::{partition::Compression, Client},
    record::Record,
    //record::RecordAndOffset,
    topic::Topic,
};

use std::collections::{HashMap, BTreeMap};
use time::OffsetDateTime;

pub async fn produce(topic: &str, partition: i32, client: &Client, record: Record) -> Result<()> {
    let partition_client = client
        .partition_client(topic.to_owned(), partition)
        .unwrap();
    partition_client
        .produce(vec![record], Compression::Lz4)
        .await
        .unwrap();

    Ok(())
}

pub fn build_record(
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

pub async fn create_topics(client: &Client, cfg: &AppConfig) -> Result<()> {
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

pub async fn send_message(
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


