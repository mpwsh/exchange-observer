//! Consumer binary: drains Redpanda topics into Scylla.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};

use tokio::time::{Duration, timeout};

use anyhow::Context;
use exchange_observer::AppConfig;
use futures::StreamExt;
use log::{error, info, warn};
use rskafka::client::ClientBuilder;
use scylla::{
    client::{session::Session as DbSession, session_builder::SessionBuilder},
    statement::prepared::PreparedStatement,
};
use stream_throttle::{ThrottlePool, ThrottleRate, ThrottledStream};
use tokio::sync::Semaphore;

use crate::{
    error::{ConsumerError, Result},
    stats::Stats,
};

pub mod error;
pub mod mq;
pub mod stats;

/// Max in-flight inserts. Bounds memory and gives the driver room to pipeline
/// without letting a slow database create unbounded backlog.
const MAX_INFLIGHT_INSERTS: usize = 512;

/// Throttle: at most this many messages/sec pulled off the Kafka streams.
/// The old value was 50k/sec — kept the same because the local Scylla can
/// keep up with it in dev; adjust down if you saturate the DB.
const THROTTLE_RATE_PER_SEC: usize = 50_000;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::load().context("loading AppConfig")?;
    info!(
        "Connecting to database at {}:{}",
        cfg.database.ip, cfg.database.port
    );

    let session = SessionBuilder::new()
        .known_node(format!("{}:{}", cfg.database.ip, cfg.database.port))
        .build()
        .await
        .context("connecting to Scylla")?;
    let session = Arc::new(session);

    if cfg.database.skip_schema_agreement {
        info!("skip_schema_agreement=true; skipping check");
    } else {
        info!("Waiting for schema agreement (max 10s)...");
        match timeout(Duration::from_secs(10), session.await_schema_agreement()).await {
            Ok(Ok(_)) => info!("Schema in agreement"),
            Ok(Err(e)) => warn!("Schema agreement returned error (continuing anyway): {e}"),
            Err(_) => warn!("Schema agreement timed out after 10s (continuing anyway)"),
        }
    }

    info!("Building prepared statements...");
    let prepared = build_prepared_statements(&session, &cfg).await?;
    info!("Prepared {} INSERT statements", prepared.len());

    let broker = format!("{}:{}", cfg.mq.ip, cfg.mq.port);
    info!("Connecting to message queue at {broker}");
    let kafka = ClientBuilder::new(vec![broker])
        .build()
        .await
        .context("connecting to Redpanda")?;

    let (streams, initial_stats) = mq::init_streams(&kafka, &cfg).await?;
    let stats = Arc::new(initial_stats);

    // Kick off the periodic stats logger. It reads only atomics and one mutex,
    // outside the hot path.
    let stats_logger = tokio::spawn(stats::run_stats_logger(
        Arc::clone(&stats),
        Arc::clone(&session),
        cfg.clone(),
    ));

    // Bounded concurrency for inserts.
    let insert_permits = Arc::new(Semaphore::new(MAX_INFLIGHT_INSERTS));

    let rate = ThrottleRate::new(THROTTLE_RATE_PER_SEC, Duration::from_millis(1000));
    let pool = ThrottlePool::new(rate);

    let mut merged = futures::stream::select_all(streams).throttle(pool);

    while let Some(item) = merged.next().await {
        let (record, _partition_offset) = match item {
            Ok((record_and_offset, _high_watermark)) => {
                (record_and_offset.record, record_and_offset.offset)
            },
            Err(e) => {
                warn!("Error reading from kafka stream: {e}");
                continue;
            },
        };

        if let Err(e) = dispatch_record(
            record,
            &prepared,
            Arc::clone(&session),
            Arc::clone(&stats),
            Arc::clone(&insert_permits),
        )
        .await
        {
            warn!("Failed to dispatch record: {e}");
            stats.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    stats_logger.abort();
    Ok(())
}

/// Prepare one INSERT-JSON statement per configured channel. Called once at
/// startup; the returned map is `Arc<PreparedStatement>` per channel name so
/// clones on the hot path are cheap ref bumps.
async fn build_prepared_statements(
    session: &DbSession,
    cfg: &AppConfig,
) -> Result<HashMap<String, Arc<PreparedStatement>>> {
    let keyspace = &cfg.database.keyspace;
    let ttl = cfg.database.data_ttl;
    let mut out = HashMap::new();

    for topic in &cfg.mq.topics {
        let cql = format!(
            "INSERT INTO {keyspace}.{name} JSON ? USING TTL {ttl}",
            name = topic.name
        );
        info!("Preparing: {cql}");
        let prepared = session.prepare(cql).await?;
        info!("Prepared: {}", topic.name);
        out.insert(topic.name.clone(), Arc::new(prepared));
    }
    info!("All prepared statements built");
    Ok(out)
}

/// Extract the channel + payload from one Kafka record and hand it off to an
/// insert task under the semaphore's backpressure.
async fn dispatch_record(
    record: rskafka::record::Record,
    prepared: &HashMap<String, Arc<PreparedStatement>>,
    session: Arc<DbSession>,
    stats: Arc<Stats>,
    permits: Arc<Semaphore>,
) -> Result<()> {
    use exchange_observer::models::Channel;
    use std::str::FromStr;

    let channel_bytes = record
        .headers
        .get("Channel")
        .ok_or(ConsumerError::MissingField("header:Channel"))?;
    let channel_str = std::str::from_utf8(channel_bytes)
        .map_err(|_| ConsumerError::UnknownChannel("<non-utf8>".to_owned()))?;
    let channel = Channel::from_str(channel_str)
        .map_err(|_| ConsumerError::UnknownChannel(channel_str.to_owned()))?;

    let stmt = prepared
        .get(channel_str)
        .cloned()
        .ok_or_else(|| ConsumerError::UnknownChannel(channel_str.to_owned()))?;

    let key = record
        .key
        .as_deref()
        .ok_or(ConsumerError::MissingField("record.key"))?;
    let inst_id = std::str::from_utf8(key)
        .map_err(|_| ConsumerError::MissingField("record.key(utf8)"))?
        .to_owned();

    let value = record
        .value
        .as_deref()
        .ok_or(ConsumerError::MissingField("record.value"))?;

    let payload = channel
        .parse(value, &inst_id)
        .map_err(|e| ConsumerError::PayloadParse {
            channel: channel_str.to_owned(),
            source: anyhow::anyhow!("{e:?}"),
        })?;

    // Track offsets — cheap: single atomic increment.
    stats.received.fetch_add(1, Ordering::Relaxed);

    // Acquire a permit; blocks (async-waits) if MAX_INFLIGHT_INSERTS is
    // reached. This is the backpressure — the read loop stops pulling from
    // Kafka until the DB catches up.
    let permit = match permits.acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            // Semaphore was closed. We don't close it anywhere, but be
            // defensive and drop the record instead of panicking.
            error!("Insert semaphore closed unexpectedly; dropping record");
            return Ok(());
        },
    };

    tokio::spawn(async move {
        let _permit = permit; // drop at end of task = release backpressure slot
        match session.execute_unpaged(&stmt, (payload,)).await {
            Ok(result) => {
                let warnings = result.warnings().collect::<Vec<_>>();
                if !warnings.is_empty() {
                    warn!("Scylla warnings: {warnings:?}");
                }
                stats.inserted.fetch_add(1, Ordering::Relaxed);
            },
            Err(e) => {
                error!("Insert failed: {e}");
                stats.errors.fetch_add(1, Ordering::Relaxed);
            },
        }
    });

    Ok(())
}
