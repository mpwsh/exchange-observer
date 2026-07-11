//! Kafka/Redpanda producer layer.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use exchange_observer::{models::*, AppConfig};
use log::{info, warn};
use rskafka::{
    chrono::Utc,
    client::{
        partition::{Compression, PartitionClient, UnknownTopicHandling},
        Client,
    },
    record::Record,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{ProducerError, Result};

/// Cache of per-(topic, partition) producer clients so we don't create a fresh
/// one on every message.
#[allow(clippy::type_complexity)]
pub struct Clients {
    pub partitions: Mutex<HashMap<(String, i32), Arc<PartitionClient>>>,
    pub admin: Arc<Client>,
}

impl Clients {
    pub fn new(admin: Arc<Client>) -> Self {
        Self {
            partitions: Mutex::new(HashMap::new()),
            admin,
        }
    }

    pub async fn get_partition_client(
        &self,
        topic: &str,
        partition: i32,
    ) -> Result<Arc<PartitionClient>> {
        let key = (topic.to_owned(), partition);
        let mut clients = self.partitions.lock().await;

        if let Some(existing) = clients.get(&key) {
            return Ok(existing.clone());
        }

        let new = Arc::new(
            self.admin
                .partition_client(topic, partition, UnknownTopicHandling::Error)
                .await?,
        );
        clients.insert(key, new.clone());
        Ok(new)
    }
}

pub async fn produce(
    topic: Channel,
    partition: i32,
    clients: Arc<Clients>,
    record: Record,
) -> Result<()> {
    let partition_client = clients
        .get_partition_client(&topic.to_string(), partition)
        .await?;
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
    partition: i32,
) -> Record {
    Record {
        key: Some(inst_id.to_vec()),
        value: Some(data.as_bytes().to_vec()),
        headers: BTreeMap::from([
            ("Exchange".to_owned(), exchange.as_bytes().to_vec()),
            (
                "Channel".to_owned(),
                channel.to_string().into_bytes(),
            ),
            ("Partition".to_owned(), partition.to_string().into_bytes()),
        ]),
        timestamp: Utc::now(),
    }
}

pub async fn create_topics(client: &Client, cfg: &AppConfig) -> Result<()> {
    let existing = client.list_topics().await?;
    info!("Topic list: {existing:?}");

    for topic in &cfg.mq.topics {
        if existing.iter().any(|t| t.name == topic.name) {
            continue;
        }
        warn!(
            "Topic {} doesn't exist. Creating with {} partitions, timeout {}ms, rf={}",
            topic.name, topic.partitions, topic.max_wait_ms, topic.replication_factor
        );
        let controller = client.controller_client()?;
        controller
            .create_topic(
                &topic.name,
                topic.partitions,
                topic.replication_factor,
                topic.max_wait_ms,
            )
            .await?;
    }
    Ok(())
}

pub async fn send_message(
    exchange: &str,
    channel: Channel,
    data: &Value,
    partition_count: &Mutex<HashMap<String, i32>>,
    clients: Arc<Clients>,
    inst_id_bytes: Vec<u8>,
) -> Result<()> {
    // Some OKX channels wrap the payload in `data[0]`, others build a candle
    // from the whole message. Extract once, per-channel.
    let first_entry = || -> Result<String> {
        data.get("data")
            .and_then(|d| d.get(0))
            .ok_or(ProducerError::MissingField("data[0]"))
            .map(ToString::to_string)
    };

    let payload = match channel {
        Channel::Tickers => {
            let ticker: Ticker = serde_json::from_str(&first_entry()?)?;
            serde_json::to_string(&ticker)?
        },
        Channel::Trades => {
            let trade: Trade = serde_json::from_str(&first_entry()?)?;
            serde_json::to_string(&trade)?
        },
        Channel::Books => {
            let book: Book = serde_json::from_str(&first_entry()?)?;
            serde_json::to_string(&book)?
        },
        Channel::Candle1m => {
            let candle = Candlestick::from_candle(data).get_change().get_range();
            serde_json::to_string(&candle)?
        },
    };

    let partition = {
        let map = partition_count.lock().await;
        map.get(&channel.to_string()).copied().unwrap_or(0)
    };

    let record = build_record(exchange, channel, &inst_id_bytes, &payload, partition);
    produce(channel, partition, clients, record).await
}
