use crate::{info, AppConfig, Elapsed, HashMap, Instant, Mutex};
pub struct Cooldowns {
    pub stats: Mutex<Instant>,
    pub ping: Mutex<Instant>,
    pub partition_change: Mutex<Instant>,
}

impl Default for Cooldowns {
    fn default() -> Self {
        Self {
            ping: Mutex::new(Instant::now()),
            stats: Mutex::new(Instant::now()),
            partition_change: Mutex::new(Instant::now()),
        }
    }
}

pub async fn update_partition_count(
    cooldowns: &Cooldowns,
    partition_count: &Mutex<HashMap<String, i32>>,
    cfg: &AppConfig,
) {
    if cooldowns
        .partition_change
        .lock()
        .await
        .elapsed()
        .as_millis()
        >= 100
    {
        let mut map = partition_count.lock().await;

        for topic in &cfg.mq.topics {
            let current_slot = map.entry(topic.name.clone()).or_insert(0);

            if *current_slot < topic.partitions - 1 {
                *current_slot += 1;
            } else {
                *current_slot = 0;
            }
        }
        *cooldowns.partition_change.lock().await = Instant::now();
    };
}

pub async fn log_stats(cooldowns: &Cooldowns, inc: &Mutex<i32>, start: Instant) {
    if cooldowns.stats.lock().await.elapsed().as_millis() >= 5000 {
        let ack_rate = *inc.lock().await / 5;
        info!(
            "Latency: {} | inc rate: {} messages/s",
            Elapsed::from(&start),
            ack_rate
        );

        *inc.lock().await = 0;
        *cooldowns.stats.lock().await = Instant::now();
    };
}
