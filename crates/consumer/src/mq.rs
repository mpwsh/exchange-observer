//! Redpanda/Kafka stream setup.

use std::sync::Arc;

use exchange_observer::AppConfig;
use log::{info, warn};
use rskafka::client::{
    consumer::{StartOffset, StreamConsumer, StreamConsumerBuilder},
    partition::{OffsetAt, UnknownTopicHandling},
    Client,
};

use crate::{
    error::Result,
    stats::{OffsetMap, Stats},
};

/// Build one `StreamConsumer` per (topic, partition) pair, seeded at the
/// configured start offset (or `earliest` if the configured value is out of
/// range on the broker).
///
/// Returns the streams plus initial `Stats` populated with the backlog.
pub async fn init_streams(
    client: &Client,
    cfg: &AppConfig,
) -> Result<(Vec<StreamConsumer>, Stats)> {
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

            let start_offset = if (earliest..latest).contains(&topic.offset) {
                topic.offset
            } else {
                warn!(
                    "Configured offset {} for topic {} out of range [{earliest}, {latest}); \
                     falling back to earliest",
                    topic.offset, topic.name
                );
                earliest
            };

            // Aggregate lag across all partitions of a topic.
            let entry = offset_map
                .entry(topic.name.clone())
                .or_insert((0, 0));
            entry.0 += start_offset;
            entry.1 += latest;
            total_backlog += latest - start_offset;

            streams.push(
                StreamConsumerBuilder::new(partition_client, StartOffset::At(start_offset))
                    .with_min_batch_size(topic.min_batch_size)
                    .with_max_batch_size(topic.max_batch_size)
                    .with_max_wait_ms(topic.max_wait_ms)
                    .build(),
            );
        }
    }

    info!(
        "Found {total_backlog} messages between selected offsets and latest. Starting catchup"
    );

    Ok((streams, Stats::new(total_backlog, offset_map)))
}
