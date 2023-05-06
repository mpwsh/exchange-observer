use crate::{warn,info, Stats};
use exchange_observer::{models::*, AppConfig};
use anyhow::Result;
use tokio::sync::Mutex;
use scylla::transport::session::Session as DbSession;
use std::time::Instant;
use std::sync::Arc;
use std::collections::HashMap;
use rskafka::client::{
    consumer::{StartOffset, StreamConsumer, StreamConsumerBuilder},
    partition::OffsetAt,
    Client,
};

pub async fn init_streams(client: &Client, cfg: &AppConfig) -> Result<(Vec<StreamConsumer>, Stats)> {
    //Topic stats
    let mut offset_stats: HashMap<String, (i64, i64)> = HashMap::new();
    //total msg count
    let mut total_msgs = 0;
    let mut streams = Vec::new();
    for topic in cfg.mq.topics.iter() {
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
    let stats = Stats {
        //basic ack rate counter
        inc: Mutex::new(0),
        total_inc: Mutex::new(0),
        total_msgs: Mutex::new(total_msgs),
        offset_map: Mutex::new(offset_stats),
        cooldown: Mutex::new(Instant::now()),
    };
   info!(
        "Found {} messages between selected offset and latest. Starting catchup",
        stats.total_msgs.lock().await
    );
    Ok((streams, stats))
}

pub async fn update_stats(session: &DbSession, topic: Channel, stats: Arc<Stats>, cfg: &AppConfig) -> Result<Arc<Stats>> {
    let mut map = stats.offset_map.lock().await;
    if let Some(x) = map.get_mut(&topic.to_string()) {
        let mut i = *x;
        i.0 += 1;
        *x = i;
    };

    if stats.cooldown.lock().await.elapsed().as_millis() >= 5000 {
        let metrics = session.get_metrics();
        cfg.mq.topics.iter().for_each(|topic| {
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
            *stats.total_inc.lock().await,
            metrics.get_queries_num(),
            metrics.get_errors_num(),
        );
        info!(
            "Average latency: {} ms | 99.9 latency percentile: {} ms",
            metrics.get_latency_avg_ms().unwrap(),
            metrics.get_latency_percentile_ms(99.9).unwrap()
        );
        let ack_rate = *stats.inc.lock().await / 5;
        info!("inc rate: {} messages/s (5 sec avg)", ack_rate);
        let msg_left =
            *stats.total_msgs.lock().await - *stats.total_inc.lock().await;
        let catchup_eta = msg_left / ack_rate as i64;
        if msg_left >= 300 {
            info!("Catch-up ETA: {} minutes", catchup_eta / 60);
        }
        let mut inc = stats.inc.lock().await;
        *inc = 0;
        *stats.cooldown.lock().await = Instant::now();
    };
    Ok(stats.clone())
}
