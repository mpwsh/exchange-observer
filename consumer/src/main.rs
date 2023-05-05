use anyhow::Result;
use exchange_observer::{models::*, AppConfig};
use futures::StreamExt;
use log::{debug, error, info, warn};
use scylla::transport::session::Session as DbSession;
use scylla::SessionBuilder;
use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use stream_throttle::{ThrottlePool, ThrottleRate, ThrottledStream};

use rskafka::client::{
    consumer::{StartOffset, StreamConsumerBuilder},
    partition::OffsetAt,
    ClientBuilder,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cfg: AppConfig = AppConfig::load()?;
    debug!("config loaded: {:#?}", cfg);
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

    let topics = cfg.mq.topics;
    let mut streams = Vec::new();

    //Topic stats
    let mut offset_stats: HashMap<String, (i64, i64)> = HashMap::new();
    //total msg count
    let mut total_msgs = 0;
    for topic in topics.iter() {
        //creates a stream per topic partition
        for partition in 0..topic.partitions {
            let partition_client = Arc::new(
                client
                    .partition_client(&topic.name, partition)
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "Failed to connect to topic {} partition: {} || Double check your config.toml file and message queue",
                            topic.name, partition
                        )
                    }),
            );
            let earliest = &partition_client.get_offset(OffsetAt::Earliest).await?;
            let latest = &partition_client.get_offset(OffsetAt::Latest).await?;
            let offset = if (earliest..latest).contains(&&topic.offset) {
                &topic.offset
            } else {
                warn!(
                    "Offset for topic {} out of range.. Selecting earliest offset: {}",
                    topic.name, earliest
                );
                earliest
            };

            offset_stats
                .entry(topic.name.clone())
                .and_modify(|x| {
                    let mut i = *x;
                    i.0 += offset;
                    i.1 += latest;
                    total_msgs += latest - offset;
                    *x = i;
                })
                .or_insert((*offset, *latest));
            streams.push(
                StreamConsumerBuilder::new(Arc::clone(&partition_client), StartOffset::At(*offset))
                    .with_min_batch_size(topic.min_batch_size)
                    .with_max_batch_size(topic.max_batch_size)
                    .with_max_wait_ms(topic.max_wait_ms)
                    .build(),
            )
        }
    }
    //basic ack rate counter
    let inc = RefCell::new(0);
    let total_inc = RefCell::new(0);
    //Save partition counter in a cell
    let offset_map = RefCell::new(offset_stats);
    //log cooldown
    let stats_cooldown = RefCell::new(Instant::now());

    //stream throttle so we dont crash the db while testing locally
    let rate = ThrottleRate::new(50000, Duration::from_millis(1000));
    let pool = ThrottlePool::new(rate);

    info!(
        "Found {} messages between selected offset and latest. Starting catchup",
        total_msgs
    );
    let read_future = futures::stream::select_all(streams)
        .throttle(pool)
        .for_each(|record| async {
            //retrieve record
            let (record, _partition_offset) = match record {
                Ok(k) => (k.0.record, k.0.offset),
                Err(e) => {
                    info!("Error while reading message: {}", e);
                    return;
                }
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

            ////Stats
            {
                //update current offset for each topic
                let mut map = offset_map.borrow_mut();
                if let Some(x) = map.get_mut(&topic.to_string()) {
                    let mut i = *x;
                    i.0 += 1;
                    *x = i;
                };

                if stats_cooldown.borrow().elapsed().as_millis() >= 5000 {
                    let metrics = session.get_metrics();
                    topics.iter().for_each(|topic| {
                        if let Some(x) = map.get_mut(&topic.name) {
                            let latest = x.1;
                            let offset = x.0;
                            //Check how far behind we are
                            let diff = latest - offset;
                            if diff > 1000 {
                                info!(
                                    "Syncing topic [{}] {offset}/{latest} || {diff} messages left",
                                    topic.name
                                );
                            };
                        }
                    });
                    info!(
                        "Total messages received / queries processed: {}/{} | [Errors: {}]",
                        total_inc.borrow(),
                        metrics.get_queries_num(),
                        metrics.get_errors_num(),
                    );
                    info!(
                        "Average latency: {} ms | 99.9 latency percentile: {} ms",
                        metrics.get_latency_avg_ms().unwrap(),
                        metrics.get_latency_percentile_ms(99.9).unwrap()
                    );
                    let ack_rate = inc.clone().into_inner() / 5;
                    info!("inc rate: {} messages/s (5 sec avg)", ack_rate);
                    let msg_left = total_msgs - total_inc.clone().into_inner();
                    let catchup_eta = msg_left / ack_rate;
                    if msg_left >= 300 {
                        info!("Catch-up ETA: {} minutes", catchup_eta / 60);
                    }
                    inc.replace(0);
                    stats_cooldown.replace(Instant::now());
                };
            };

            let session = session.clone();
            let payload = topic.parse(data, &inst_id).unwrap();
            let query = format!(
                "INSERT INTO {}.{} JSON '{}' USING TTL {}",
                exchange, channel, payload, cfg.database.data_ttl
            );

            tokio::task::spawn(async move {
                match session.query(query.clone(), &[]).await {
                    Ok(k) => {
                        if !k.warnings.is_empty() {
                            warn!("{:?}", k.warnings)
                        }
                    }
                    Err(e) => error!("{}", e),
                };
            });

            inc.replace_with(|&mut old| old + 1);
            total_inc.replace_with(|&mut old| old + 1);
        });
    read_future.await;
    Ok(())
}
