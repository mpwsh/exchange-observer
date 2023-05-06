use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use log::debug;
use serde_derive::{Deserialize, Serialize};
use sha2::Sha256;
use std::env;
use std::net::Ipv4Addr;
use thiserror::Error;
pub use time::{error::Format, format_description::well_known::Rfc3339, OffsetDateTime};
pub mod models;
pub mod util;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub database: Database,
    pub mq: MessageQueue,
    pub account: Account,
    pub pushover: Option<Pushover>,
    pub strategy: Strategy,
    pub exchange: Option<Exchange>,
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
    pub enable: bool,
    pub token: String,
    pub key: String,
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
    pub balance: f64,
    pub spendable: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Exchange {
    pub enable_trading: bool,
    pub name: String,
    pub authentication: Authentication,
    pub taker_fee: f64,
    pub maker_fee: f64,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Authentication {
    pub access_key: String,
    pub secret_key: String,
    pub passphrase: String,
    #[serde(skip_deserializing, skip_serializing)]
    pub signature: Signature,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Strategy {
    #[serde(skip_deserializing)]
    pub hash: String,
    pub top: usize,
    pub portfolio_size: u32,
    pub timeframe: i64,
    pub cooldown: i64,
    pub timeout: i64,
    pub min_vol: Option<f64>,
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
    pub deny_list: bool,
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

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Signature {
    #[serde(rename = "sign")]
    pub signature: String,
    pub timestamp: String,
}

#[derive(Debug, Error)]
pub enum SignError {
    #[error("format timestamp error: {0}")]
    FormatTimestamp(#[from] Format),
    #[error("convert timestamp error: {0}")]
    ConvertTimestamp(#[from] time::error::ComponentRange),
    #[error("secretkey length error")]
    SecretKeyLength,
}
impl AppConfig {
    fn default() -> Self {
        Self {
            database: Database::default(),
            mq: MessageQueue::default(),
            account: Account::default(),
            pushover: None,
            exchange: None,
            strategy: Strategy::default(),
            ui: Ui::default(),
            server: None,
        }
    }
    pub fn load() -> Result<Self> {
        let path = env::current_dir()?;
        debug!("The current directory is {}", path.display());
        let config_path =
            env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
        env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
        let cfg = confy::load_path(config_path).unwrap_or_else(|e| {
            log::error!("Loading default config due to:\n{}", e);
            AppConfig::default()
        });
        debug!("config loaded: {:#?}", cfg);
        Ok(cfg)
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
impl Authentication {
    // Code from: Nouzan
    // https://github.com/Nouzan/exc/blob/main/exc-okx/src/key.rs
    pub fn sign(
        &self,
        method: &str,
        uri: &str,
        timestamp: OffsetDateTime,
        use_unix_timestamp: bool,
        body: &str,
    ) -> Result<Signature, SignError> {
        let secret = self.secret_key.clone();
        let timestamp = timestamp.replace_millisecond(timestamp.millisecond())?;
        let timestamp = if use_unix_timestamp {
            timestamp.unix_timestamp().to_string()
        } else {
            timestamp.format(&Rfc3339)?
        };
        let raw_sign = timestamp.clone() + method + uri + body;
        debug!("message to sign: {}", raw_sign);
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| SignError::SecretKeyLength)?;
        mac.update(raw_sign.as_bytes());

        Ok(Signature {
            signature: general_purpose::STANDARD.encode(mac.finalize().into_bytes()),
            timestamp,
        })
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

impl Default for Exchange {
    fn default() -> Self {
        Self {
            name: String::from("okx"),
            enable_trading: false,
            authentication: Authentication::default(),
            taker_fee: 0.1,
            maker_fee: 0.08,
        }
    }
}
impl Strategy {
    fn default() -> Self {
        let timeframe = 5;
        Self {
            hash: String::new(),
            top: 5,
            portfolio_size: 5,
            timeframe,
            cooldown: 40,
            timeout: 40,
            min_vol: Some((timeframe * 3500) as f64),
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
        self.min_vol.unwrap_or((self.timeframe * 3500) as f64);
        self
    }
    pub fn get_hash(&self) -> String {
        sha1_smol::Sha1::from(serde_json::to_string_pretty(&self).unwrap())
            .digest()
            .to_string()
    }
}
impl Ui {
    fn default() -> Self {
        Self {
            dashboard: true,
            portfolio: true,
            debug: true,
            balance: true,
            deny_list: true,
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
