use crate::{RefCell, Instant, AppConfig, HashMap, info, Elapsed};
pub struct Cooldowns {
    pub stats: RefCell<Instant>,
    pub ping: RefCell<Instant>,
    pub partition_change: RefCell<Instant>,
}

impl Default for Cooldowns {
    fn default() -> Self {
        Self {
            ping: RefCell::new(Instant::now()),
            stats: RefCell::new(Instant::now()),
            partition_change: RefCell::new(Instant::now()),
        }
    }
}
pub async fn update_partition_count(
    cooldowns: &Cooldowns,
    partition_count: &RefCell<HashMap<String, i32>>,
    cfg: &AppConfig,
) {
    if cooldowns.partition_change.borrow().elapsed().as_millis() >= 100 {
        for topic in cfg.mq.topics.iter() {
            let mut map = partition_count.borrow_mut();
            let current_slot = map.get(&topic.name).unwrap();
            if current_slot < &(topic.partitions - 1) {
                if let Some(x) = map.get_mut(&topic.name) {
                    *x += 1;
                }
            } else if let Some(x) = map.get_mut(&topic.name) {
                *x = 0;
            }
        }
    };
}

pub async fn log_stats(cooldowns: &Cooldowns, inc: &RefCell<i32>, start: Instant) {
    if cooldowns.stats.borrow().elapsed().as_millis() >= 5000 {
        let ack_rate = inc.clone().into_inner() / 5;
        info!("Latency: {}", Elapsed::from(&start));
        info!("inc rate: {} messages/s", ack_rate);
        inc.replace(0);
        cooldowns.stats.replace(Instant::now());
    };
}
