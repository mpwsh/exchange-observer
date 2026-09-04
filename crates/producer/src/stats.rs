//! Cooldown-driven housekeeping: rotating Kafka partitions and periodic stats.

use std::{collections::HashMap, time::Instant};

use exchange_observer::{models::Channel, util::Elapsed, AppConfig};
use log::info;
use tokio::sync::Mutex;

/// How often to rotate through partitions to spread load.
const PARTITION_ROTATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// How often to emit the stats line.
const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5_000);

/// Timestamps used to throttle periodic side-effects.
pub struct Cooldowns {
    pub stats: Mutex<Instant>,
    pub ping: Mutex<Instant>,
    pub partition_change: Mutex<Instant>,
}

impl Default for Cooldowns {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            ping: Mutex::new(now),
            stats: Mutex::new(now),
            partition_change: Mutex::new(now),
        }
    }
}

/// Initial partition-index map with all known channels set to 0.
pub fn initial_partition_map() -> HashMap<String, i32> {
    [
        Channel::Candle1m,
        Channel::Tickers,
        Channel::Books,
        Channel::Trades,
    ]
    .iter()
    .map(|c| (c.to_string(), 0))
    .collect()
}

/// Advance each topic's partition index round-robin. No-op if the cooldown
/// hasn't elapsed yet.
pub async fn update_partition_count(
    cooldowns: &Cooldowns,
    partition_count: &Mutex<HashMap<String, i32>>,
    cfg: &AppConfig,
) {
    // Lock once, check, and update — avoids the double/triple lock pattern
    // in the previous implementation.
    let mut last = cooldowns.partition_change.lock().await;
    if last.elapsed() < PARTITION_ROTATE_INTERVAL {
        return;
    }

    let mut map = partition_count.lock().await;
    for topic in &cfg.mq.topics {
        let slot = map.entry(topic.name.clone()).or_insert(0);
        *slot = if *slot < topic.partitions - 1 {
            *slot + 1
        } else {
            0
        };
    }
    *last = Instant::now();
}

/// Log throughput stats. No-op if the cooldown hasn't elapsed yet.
pub async fn log_stats(cooldowns: &Cooldowns, inc: &Mutex<i64>, start: Instant) {
    let mut last = cooldowns.stats.lock().await;
    if last.elapsed() < STATS_INTERVAL {
        return;
    }

    let mut counter = inc.lock().await;
    let ack_rate = *counter / (STATS_INTERVAL.as_secs() as i64).max(1);
    info!(
        "Latency: {} | inc rate: {} messages/s",
        Elapsed::from(&start),
        ack_rate
    );
    *counter = 0;
    *last = Instant::now();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_partition_map_covers_all_channels() {
        let map = initial_partition_map();
        assert_eq!(map.len(), 4);
        assert!(map.values().all(|v| *v == 0));
    }
}
