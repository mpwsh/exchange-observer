//! Redpanda/Kafka stream setup.

use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};

use exchange_observer::AppConfig;
use futures::{Stream, StreamExt};
use log::{info, warn};
use rskafka::client::{
    consumer::{StartOffset, StreamConsumer, StreamConsumerBuilder},
    partition::{OffsetAt, UnknownTopicHandling},
    Client,
};

use crate::{
    error::Result,
    stats::{OffsetMap, PartitionProgress, Stats},
};

/// Build one stream per (topic, partition) pair, seeded at the configured
/// start offset (or `earliest` if the configured value is out of range on
/// the broker).
///
/// Each stream is wrapped so that every received record updates its
/// partition's [`PartitionProgress`] (relaxed atomic stores — no locks on
/// the hot path). The same progress cells are shared with the returned
/// `Stats`, which is how the stats logger sees live catch-up numbers.
pub async fn init_streams(
    client: &Client,
    cfg: &AppConfig,
) -> Result<(
    Vec<impl Stream<Item = <StreamConsumer as Stream>::Item>>,
    Stats,
)> {
    let mut offset_map: OffsetMap = OffsetMap::new();
    let mut total_backlog: i64 = 0;
    let mut streams = Vec::new();

    for topic in &cfg.mq.topics {
        for partition in 0..topic.partitions {
            let partition_client = Arc::new(
                client
                    .partition_client(&topic.name, partition, UnknownTopicHandling::Error)
                    .await?,
            );

            let earliest = partition_client.get_offset(OffsetAt::Earliest).await?;
            let latest = partition_client.get_offset(OffsetAt::Latest).await?;

            // Inclusive upper bound: `offset == latest` is the normal
            // "fully caught up, wait for new data" position. Treating it as
            // out-of-range (the old `earliest..latest`) silently fell back
            // to earliest and re-consumed the whole partition on restart.
            let start_offset = if (earliest..=latest).contains(&topic.offset) {
                topic.offset
            } else {
                warn!(
                    "Configured offset {} for topic {} out of range [{earliest}, {latest}]; \
                     falling back to earliest",
                    topic.offset, topic.name
                );
                earliest
            };

            let progress = Arc::new(PartitionProgress {
                current: AtomicI64::new(start_offset),
                latest: AtomicI64::new(latest),
            });
            offset_map
                .entry(topic.name.clone())
                .or_default()
                .push(Arc::clone(&progress));
            total_backlog += latest - start_offset;

            let stream =
                StreamConsumerBuilder::new(partition_client, StartOffset::At(start_offset))
                    .with_min_batch_size(topic.min_batch_size)
                    .with_max_batch_size(topic.max_batch_size)
                    .with_max_wait_ms(topic.max_wait_ms)
                    .build();

            // Record progress as items flow through, then pass them along
            // untouched — the consume loop in main.rs is unaffected.
            streams.push(stream.map(move |item| {
                if let Ok((record_and_offset, high_watermark)) = &item {
                    progress
                        .current
                        .store(record_and_offset.offset + 1, Ordering::Relaxed);
                    progress.latest.store(*high_watermark, Ordering::Relaxed);
                }
                item
            }));
        }
    }

    info!("Found {total_backlog} messages between selected offsets and latest. Starting catchup");

    Ok((streams, Stats::new(total_backlog, offset_map)))
}
