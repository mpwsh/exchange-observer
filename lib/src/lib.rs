use serde_derive::{Deserialize, Serialize};
use std::net::Ipv4Addr;
pub mod models;
pub mod util;
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub database: Database,
    pub mq: MessageQueue,
    pub account: Account,
    pub pushover: Pushover,
    pub strategy: Strategy,
    pub ui: Ui,
    pub server: Option<Server>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Database {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub keyspace: String,
    pub data_ttl: u32,
}
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Pushover {
    pub token: Option<String>,
    pub key: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MessageQueue {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub topics: Vec<Topic>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Topic {
    pub name: String,
    pub partitions: i32,
    pub replication_factor: i16,
    pub offset: i64,
    pub min_batch_size: i32,
    pub max_batch_size: i32,
    pub max_wait_ms: i32,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Account {
    pub exchange: Option<String>,
    pub api_key: Option<String>,
    pub api_token: Option<String>,
    pub balance: u32,
    pub taker_fee: f32,
    pub spendable: u32,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Strategy {
    pub hash: Option<String>,
    pub top: usize,
    pub portfolio_size: u32,
    pub timeframe: i64,
    pub cooldown: i64,
    pub timeout: i64,
    pub min_vol: Option<f32>,
    pub min_change: f32,
    pub min_change_last_candle: f32,
    pub min_deviation: f32,
    pub deny_list: Option<Vec<String>>,
    pub cashout: f32,
    pub quickstart: bool,
    pub stoploss: f32,
    pub avoid_after_stoploss: bool,
    pub sell_floor: Option<f32>,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Ui {
    pub dashboard: bool,
    pub portfolio: bool,
    pub debug: bool,
    pub balance: bool,
    pub logs: bool,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Server {
    pub enable: bool,
    pub listen_address: Ipv4Addr,
    pub port: u16,
    pub log_level: String,
    pub workers: u8,
}
impl AppConfig {
    fn default() -> Self {
        Self {
            database: Database::default(),
            mq: MessageQueue::default(),
            account: Account::default(),
            pushover: Pushover::default(),
            strategy: Strategy::default(),
            ui: Ui::default(),
            server: None,
        }
    }
    pub fn load(config_path: &str) -> Self {
        confy::load_path(config_path).unwrap_or_else(|e| {
            log::error!("Loading default config due to:\n{}", e);
            AppConfig::default()
        })
    }
}
impl Default for Database {
    fn default() -> Self {
        Self {
            ip: Ipv4Addr::new(127, 0, 0, 1),
            port: 9042,
            keyspace: String::from("okx"),
            //1 day
            data_ttl: (3600 * 24),
        }
    }
}

impl Default for MessageQueue {
    fn default() -> Self {
        Self {
            ip: Ipv4Addr::new(127, 0, 0, 1),
            port: 9092,
            topics: Vec::new(),
        }
    }
}
impl Default for Topic {
    fn default() -> Self {
        Self {
            name: "topic".to_string(),
            partitions: 1,
            offset: 0,
            replication_factor: 1,
            min_batch_size: 100,
            max_batch_size: 10000,
            max_wait_ms: 200,
        }
    }
}

impl Account {
    fn default() -> Self {
        Self {
            exchange: None,
            api_key: None,
            api_token: None,
            balance: 700,
            taker_fee: 0.1,
            spendable: 100,
        }
    }
}
impl Strategy {
    fn default() -> Self {
        let timeframe = 5;
        Self {
            hash: None,
            top: 5,
            portfolio_size: 5,
            timeframe,
            cooldown: 40,
            timeout: 40,
            min_vol: Some((timeframe * 3500) as f32),
            min_change: 0.1,
            min_change_last_candle: 0.1,
            min_deviation: 0.1,
            deny_list: None,
            cashout: 10.0,
            quickstart: false,
            stoploss: 3.0,
            avoid_after_stoploss: false,
            sell_floor: None,
        }
    }
    pub fn sane_defaults(&mut self) -> &mut Self {
        self.min_vol.unwrap_or((self.timeframe * 3500) as f32);
        self
    }
    pub fn get_hash(&self) -> Option<String> {
        Some(
            sha1_smol::Sha1::from(serde_json::to_string_pretty(&self).unwrap())
                .digest()
                .to_string(),
        )
    }
}
impl Ui {
    fn default() -> Self {
        Self {
            dashboard: true,
            portfolio: true,
            debug: true,
            balance: true,
            logs: true,
        }
    }
}
impl Default for Server {
    fn default() -> Self {
        Self {
            enable: false,
            listen_address: Ipv4Addr::new(127, 0, 0, 1),
            port: 3030,
            log_level: String::from("info"),
            workers: 8,
        }
    }
}
