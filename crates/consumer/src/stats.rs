//! Stats: atomic counters on the hot path, background logger prints them.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use exchange_observer::AppConfig;
use log::info;
use scylla::client::session::Session as DbSession;
use tokio::{sync::Mutex, time::interval};

/// How often to log throughput / catch-up stats.
const STATS_INTERVAL: Duration = Duration::from_secs(5);

/// (current_offset, latest_watermark) per topic, updated as inserts land.
pub type OffsetMap = HashMap<String, (i64, i64)>;

/// Runtime counters. Hot-path fields are atomics — no locks per message.
pub struct Stats {
    /// Messages pulled off Kafka and dispatched (successful parse).
    pub received: AtomicI64,
    /// Successful Scylla inserts.
    pub inserted: AtomicI64,
    /// Any error along the way — parse, insert, dispatch.
    pub errors: AtomicI64,
    /// Total messages waiting between start-offset and high-watermark at boot.
    pub backlog_at_start: AtomicI64,
    /// Per-topic offset tracking. Locked once per stats tick, not per message.
    pub offsets: Mutex<OffsetMap>,
}

impl Stats {
    pub fn new(backlog: i64, offsets: OffsetMap) -> Self {
        Self {
            received: AtomicI64::new(0),
            inserted: AtomicI64::new(0),
            errors: AtomicI64::new(0),
            backlog_at_start: AtomicI64::new(backlog),
            offsets: Mutex::new(offsets),
        }
    }
}

/// Periodic stats logger. Runs until aborted.
///
/// Uses `Ordering::Relaxed` on all reads: we don't need happens-before
/// guarantees, just approximately-current counts.
pub async fn run_stats_logger(stats: Arc<Stats>, session: Arc<DbSession>, cfg: AppConfig) {
    let mut ticker = interval(STATS_INTERVAL);
    ticker.tick().await; // consume immediate first tick

    let mut last_inserted: i64 = 0;
    let mut last_tick = Instant::now();

    loop {
        ticker.tick().await;
        let now = Instant::now();
        let elapsed = now.duration_since(last_tick).as_secs_f64().max(0.001);
        last_tick = now;

        let inserted = stats.inserted.load(Ordering::Relaxed);
        let received = stats.received.load(Ordering::Relaxed);
        let errors = stats.errors.load(Ordering::Relaxed);
        let backlog = stats.backlog_at_start.load(Ordering::Relaxed);

        let delta = inserted - last_inserted;
        last_inserted = inserted;
        let ack_rate = (delta as f64 / elapsed).round() as i64;

        // Per-topic lag report.
        {
            let offsets = stats.offsets.lock().await;
            for topic in &cfg.mq.topics {
                if let Some(&(current, latest)) = offsets.get(&topic.name) {
                    let diff = latest - current;
                    if diff > 1000 {
                        info!(
                            "Syncing topic [{}] {current}/{latest} || {diff} messages left",
                            topic.name
                        );
                    }
                }
            }
        }

        let metrics = session.get_metrics();
        info!(
            "Received: {received} | Inserted: {inserted} | Errors: {errors} | Driver queries: {} | Driver errors: {}",
            metrics.get_queries_num(),
            metrics.get_errors_num(),
        );
        info!(
            "Avg latency: {} ms | P99.9 latency: {} ms",
            metrics.get_latency_avg_ms().unwrap_or(0),
            metrics.get_latency_percentile_ms(99.9).unwrap_or(0),
        );
        info!("Insert rate: {ack_rate} messages/s (over {elapsed:.2}s)");

        let remaining = backlog - inserted;
        if remaining >= 300 && ack_rate > 0 {
            let eta_secs = remaining / ack_rate;
            info!("Catch-up ETA: {} minutes", eta_secs / 60);
        }
    }
}
