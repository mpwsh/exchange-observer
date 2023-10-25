use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use exchange_observer::{models::*, AppConfig};
use futures::StreamExt;
use log::{error, info, warn};
use rskafka::client::ClientBuilder;
use scylla::{transport::session::Session as DbSession, SessionBuilder};
use stream_throttle::{ThrottlePool, ThrottleRate, ThrottledStream};
use tokio::sync::Mutex;

pub mod mq;

pub struct Stats {
    pub inc: Mutex<usize>,
    pub total_msgs: Mutex<i64>,
    pub total_inc: Mutex<i64>,
    pub offset_map: Mutex<HashMap<String, (i64, i64)>>,
    pub cooldown: Mutex<Instant>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg: AppConfig = AppConfig::load()?;
    info!(
        "Connecting to database at {}:{} ...",
        cfg.database.ip, cfg.database.port
    );

    let session: DbSession = SessionBuilder::new()
        .known_node(&cfg.database.ip.to_string())
        .build()
        .await?;
    let session = Arc::new(session);

    //Check for Schema Agreement
    info!("Waiting for schema agreement for 5 seconds...");
    match session
        .await_timed_schema_agreement(Duration::from_secs(5))
        .await
    {
        Ok(_) => info!("Schema is in agreement - Proceeding"),
        Err(e) => error!("Error while retrieving schema agrement. Error: {e}"),
    };

    // setup redpanda client
    let connection = format!("{}:{}", cfg.mq.ip, cfg.mq.port);
    info!("Connecting to message queue at {} ...", connection);
    let client = ClientBuilder::new(vec![connection]).build().await?;
    let (streams, stats) = mq::init_streams(&client, &cfg).await?;
    let stats = Arc::new(stats);
    //stream throttle so we dont crash the db while testing locally
    let rate = ThrottleRate::new(50000, Duration::from_millis(1000));
    let pool = ThrottlePool::new(rate);

    let read_future = futures::stream::select_all(streams)
        .throttle(pool)
        .for_each(|record| {
            let stats = Arc::clone(&stats);
            let cfg = cfg.clone();
            let session = session.clone();
            async move {
                //retrieve record
                let (record, _partition_offset) = match record {
                    Ok(k) => (k.0.record, k.0.offset),
                    Err(e) => {
                        info!("Error while reading message: {}", e);
                        return;
                    },
                };

                let no_data = vec![0u8];
                let exchange =
                    String::from_utf8_lossy(record.headers.get("Exchange").unwrap_or(&no_data));
                let data = &record.value.expect("unable to get record value");
                let channel =
                    String::from_utf8_lossy(record.headers.get("Channel").unwrap_or(&no_data));
                let record_key = record.key.expect("Unable to get record key");
                let inst_id = String::from_utf8_lossy(&record_key);
                let topic = Channel::from_str(&channel).unwrap();

                let session = session.clone();
                let payload = topic.parse(data, &inst_id).unwrap();
                let query = format!(
                    "INSERT INTO {}.{} JSON '{}' USING TTL {}",
                    exchange, channel, payload, cfg.database.data_ttl
                );

                let stats = mq::update_stats(&session, topic, stats, &cfg)
                    .await
                    .unwrap();
                tokio::task::spawn(async move {
                    match session.query(query.clone(), &[]).await {
                        Ok(k) => {
                            if !k.warnings.is_empty() {
                                warn!("{:?}", k.warnings)
                            }
                        },
                        Err(e) => error!("{}", e),
                    };
                });
                let mut inc = stats.inc.lock().await;
                *inc += 1;

                let mut total_inc = stats.total_inc.lock().await;
                *total_inc += 1;
            }
        });
    read_future.await;
    Ok(())
}
