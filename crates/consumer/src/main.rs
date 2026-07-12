//! Consumer binary: drains Redpanda topics into Scylla using typed columns.
//!
//! Key change from v1: instead of `INSERT ... JSON ?` (which makes Scylla
//! parse a JSON string server-side on every write), we prepare per-channel
//! `INSERT ... (cols...) VALUES (?, ?, ...)` statements and bind typed
//! primitives directly. That skips the JSON parse and typically gives 2-3x
//! insert throughput.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::Context;
use exchange_observer::{AppConfig, models::*};
use futures::StreamExt;
use log::{error, info, warn};
use rskafka::client::ClientBuilder;
use scylla::{
    client::{session::Session as DbSession, session_builder::SessionBuilder},
    statement::prepared::PreparedStatement,
};
use stream_throttle::{ThrottlePool, ThrottleRate, ThrottledStream};
use tokio::{
    sync::Semaphore,
    time::{Duration as TokioDuration, timeout},
};

use crate::{
    error::{ConsumerError, Result},
    stats::Stats,
};

pub mod error;
pub mod mq;
pub mod stats;

const MAX_INFLIGHT_INSERTS: usize = 512;
const THROTTLE_RATE_PER_SEC: usize = 50_000;

/// One prepared statement per channel. Kept together so lookups on the hot
/// path are one branch, not a HashMap read.
struct PreparedStmts {
    tickers: Arc<PreparedStatement>,
    candle1m: Arc<PreparedStatement>,
    trades: Arc<PreparedStatement>,
    books: Arc<PreparedStatement>,
}

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
        match timeout(
            TokioDuration::from_secs(10),
            session.await_schema_agreement(),
        )
        .await
        {
            Ok(Ok(_)) => info!("Schema in agreement"),
            Ok(Err(e)) => warn!("Schema agreement returned error (continuing anyway): {e}"),
            Err(_) => warn!("Schema agreement timed out after 10s (continuing anyway)"),
        }
    }

    info!("Preparing typed insert statements...");
    let prepared = build_prepared_statements(&session, &cfg).await?;
    info!("Prepared 4 typed insert statements");

    let broker = format!("{}:{}", cfg.mq.ip, cfg.mq.port);
    info!("Connecting to message queue at {broker}");
    let kafka = ClientBuilder::new(vec![broker])
        .build()
        .await
        .context("connecting to Redpanda")?;

    let (streams, initial_stats) = mq::init_streams(&kafka, &cfg).await?;
    let stats = Arc::new(initial_stats);

    let stats_logger = tokio::spawn(stats::run_stats_logger(
        Arc::clone(&stats),
        Arc::clone(&session),
        cfg.clone(),
    ));

    let insert_permits = Arc::new(Semaphore::new(MAX_INFLIGHT_INSERTS));
    let prepared = Arc::new(prepared);

    let rate = ThrottleRate::new(THROTTLE_RATE_PER_SEC, Duration::from_millis(1000));
    let pool = ThrottlePool::new(rate);

    let mut merged = futures::stream::select_all(streams).throttle(pool);

    while let Some(item) = merged.next().await {
        let record = match item {
            Ok((record_and_offset, _high_watermark)) => record_and_offset.record,
            Err(e) => {
                warn!("Error reading from kafka stream: {e}");
                continue;
            },
        };

        if let Err(e) = dispatch_record(
            record,
            Arc::clone(&prepared),
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

/// Prepare four typed INSERT statements — one per table shape.
async fn build_prepared_statements(session: &DbSession, cfg: &AppConfig) -> Result<PreparedStmts> {
    let ks = &cfg.database.keyspace;
    let ttl = cfg.database.data_ttl;

    let tickers_cql = format!(
        "INSERT INTO {ks}.tickers \
         (instid, askpx, asksz, bidpx, bidsz, high24h, last, lastsz, \
          low24h, open24h, sodutc0, sodutc8, ts, vol24h, volccy24h) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) USING TTL {ttl}"
    );
    let candle_cql = format!(
        "INSERT INTO {ks}.candle1m \
         (instid, open, high, low, close, volume, change, range, ts) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) USING TTL {ttl}"
    );
    let trades_cql = format!(
        "INSERT INTO {ks}.trades \
         (instid, sz, tradeid, px, side, ts) \
         VALUES (?, ?, ?, ?, ?, ?) USING TTL {ttl}"
    );
    let books_cql = format!(
        "INSERT INTO {ks}.books \
         (instid, asks, bids, checksum, prev_seq_id, seq_id, ts) \
         VALUES (?, ?, ?, ?, ?, ?, ?) USING TTL {ttl}"
    );

    Ok(PreparedStmts {
        tickers: Arc::new(session.prepare(tickers_cql).await?),
        candle1m: Arc::new(session.prepare(candle_cql).await?),
        trades: Arc::new(session.prepare(trades_cql).await?),
        books: Arc::new(session.prepare(books_cql).await?),
    })
}

async fn dispatch_record(
    record: rskafka::record::Record,
    prepared: Arc<PreparedStmts>,
    session: Arc<DbSession>,
    stats: Arc<Stats>,
    permits: Arc<Semaphore>,
) -> Result<()> {
    use std::str::FromStr;

    let channel_bytes = record
        .headers
        .get("Channel")
        .ok_or(ConsumerError::MissingField("header:Channel"))?;
    let channel_str = std::str::from_utf8(channel_bytes)
        .map_err(|_| ConsumerError::UnknownChannel("<non-utf8>".to_owned()))?;
    let channel = Channel::from_str(channel_str)
        .map_err(|_| ConsumerError::UnknownChannel(channel_str.to_owned()))?;

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

    // Parse into a typed variant. This is where JSON → f64/i64 happens —
    // once, in Rust, instead of on every Scylla insert.
    let payload = channel
        .parse(value, &inst_id)
        .map_err(|e| ConsumerError::PayloadParse {
            channel: channel_str.to_owned(),
            source: e,
        })?;

    stats.received.fetch_add(1, Ordering::Relaxed);

    let permit = match permits.acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            error!("Insert semaphore closed unexpectedly; dropping record");
            return Ok(());
        },
    };

    tokio::spawn(async move {
        let _permit = permit;
        let result = insert_payload(&session, &prepared, &payload).await;
        match result {
            Ok(warnings) => {
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

/// Dispatch to the correct prepared statement with the typed row tuple.
///
/// Each arm binds a channel-specific tuple (see `models.rs::*Row` type
/// aliases) — no HashMap lookup, no JSON parse, one branch.
async fn insert_payload(
    session: &DbSession,
    prepared: &PreparedStmts,
    payload: &RowPayload,
) -> Result<Vec<String>> {
    let result = match payload {
        RowPayload::Ticker { inst_id, row } => {
            session
                .execute_unpaged(&prepared.tickers, row.to_row(inst_id))
                .await?
        },
        RowPayload::Candle { inst_id, row } => {
            session
                .execute_unpaged(&prepared.candle1m, row.to_row(inst_id))
                .await?
        },
        RowPayload::Trade { inst_id, row } => {
            session
                .execute_unpaged(&prepared.trades, row.to_row(inst_id))
                .await?
        },
        RowPayload::Book { inst_id, row } => {
            session
                .execute_unpaged(&prepared.books, row.to_row(inst_id))
                .await?
        },
    };
    Ok(result.warnings().map(|s| s.to_owned()).collect())
}

