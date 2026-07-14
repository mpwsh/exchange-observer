use std::{env, net::Ipv4Addr};

use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use log::debug;
use serde_derive::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
pub use time::{OffsetDateTime, error::Format, format_description::well_known::Rfc3339};
pub mod models;
pub mod util;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Serialize, Deserialize, Clone)]
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
    #[serde(default)]
    pub skip_schema_agreement: bool,
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
    /// Taker fee as a **percent**, e.g. `0.1` means 0.10%.
    ///
    /// Every order this system sends is a taker order (`ioc`, or `market` on the
    /// sell fallback), so a round trip costs `2 * taker_fee` = 0.20% at the
    /// default — before the spread. Against a `cashout` of 1.0% and a `stoploss`
    /// of 1.0%, that is a 60% break-even win rate.
    pub taker_fee: f64,
    pub maker_fee: f64,
    pub order_ttl: u32,
    pub channels: Vec<ChannelSettings>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChannelSettings {
    pub name: String,
    pub topic: String,
    pub endpoint: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Authentication {
    pub access_key: String,
    pub secret_key: String,
    pub passphrase: String,
    #[serde(skip_deserializing, skip_serializing)]
    pub signature: Signature,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Strategy {
    #[serde(skip_deserializing)]
    pub hash: String,
    /// Which strategy to run: `"threshold"` (momentum) or `"reversion"`.
    ///
    /// `None` means threshold, for configs written before the field existed.
    ///
    /// This field has existed all along and was read by *nothing*:
    /// `scheduler/src/main.rs` hardcoded `let strategy = ThresholdStrategy;`.
    /// A config that said `reversion` ran momentum — with `min_change`,
    /// `min_deviation` and `min_change_last_candle` zeroed out, on the belief
    /// that they were inert under reversion. They are the only entry gates
    /// `ThresholdStrategy` reads.
    pub strategy_type: Option<String>,

    pub order_type: String,
    pub top: usize,
    pub portfolio_size: u32,
    pub timeframe: i64,
    pub cooldown: i64,
    pub timeout: i64,
    /// Minimum quote-currency volume over the timeframe.
    ///
    /// `Option` for backwards compatibility, but note that
    /// `engine::common_checks::volume_below_min` **panics** when this is `None`.
    /// The default below keeps that from being reachable via a config that
    /// simply omits the key.
    pub min_vol: Option<f64>,
    /// Widest quoted spread, in basis points, an entry may cross.
    ///
    /// New. Nothing in this system has ever looked at the cost of getting in and
    /// out: every price came from `tickers.last`, even though `tickers` has
    /// carried `askpx`/`bidpx` since the producer was written. `None` disables
    /// the check (the historical behavior).
    pub max_spread_bps: Option<f32>,
    pub min_change: f32,
    pub min_change_last_candle: f32,
    pub min_deviation: f32,
    pub max_deviation: f32,
    pub deny_list: Option<Vec<String>>,
    pub cashout: f32,
    pub quickstart: bool,
    pub stoploss: f32,
    pub avoid_after_stoploss: bool,
    pub sell_floor: Option<f32>,
    pub min_rising_candles: Option<u32>,

    // ReversionStrategy tunables. All `Option` so existing config files still
    // deserialize; the strategy applies its own defaults on `None`. Ignored
    // entirely by ThresholdStrategy.
    pub dip_window: Option<u32>,
    pub bounce_window: Option<u32>,
    /// Absolute floor on the dip, in percent. A **cost** test: a dislocation
    /// smaller than the round trip (2 x taker + spread, ~0.22%) cannot pay for
    /// itself even if it reverts perfectly.
    pub min_dip: Option<f32>,
    /// Dip requirement in multiples of the token's own volatility. A
    /// **dislocation** test. `None` disables it, leaving `min_dip` alone.
    ///
    /// The threshold applied is `max(min_dip, min_dip_std * sigma * sqrt(dip_window))`,
    /// so both must be satisfied. The floor asks "is there enough here to pay for
    /// the trade?" and the sigma term asks "is this unusual for *this* token?".
    /// Neither alone is sufficient.
    ///
    /// Why: a fixed percentage means wildly different things across the universe.
    /// At `min_dip = 0.5` with SOL's ~0.05% 1-minute sigma, a 3-candle dip scales as
    /// `0.05 * sqrt(3) = 0.087%` — so 0.5% is a **5.7 sigma** demand and SOL can never
    /// pass. PI's sigma is ~0.5%, so the same 0.5% is **0.58 sigma** and fires on the
    /// token merely breathing. One number, ten-fold difference in meaning, and it
    /// silently reduced a 250-token universe to whichever handful was noisiest —
    /// which is also the handful whose order book cannot absorb `spendable`.
    pub min_dip_std: Option<f32>,
    /// Absolute floor on the bounce, in percent.
    pub min_bounce: Option<f32>,
    /// Bounce requirement in multiples of the token's own volatility. See
    /// [`Self::min_dip_std`].
    pub min_bounce_std: Option<f32>,
    pub avoid_falling_knives: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Ui {
    pub enable: bool,
    pub dashboard: bool,
    pub portfolio: bool,
    pub strategy: bool,
    pub system: bool,
    pub deny_list: bool,
    pub balance: bool,
    pub logs: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Server {
    pub enable: bool,
    pub listen_address: Ipv4Addr,
    pub port: u16,
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

// ---------------------------------------------------------------------------
// Defaults
//
// Every config type here previously had *two* defaults: a derived `Default`
// (all zeros / empty strings) and a private inherent `fn default()` holding the
// real values. Inherent methods win at the call site, so `AppConfig::default()`
// inside this crate got the sane one — but any generic caller going through the
// `Default` trait (confy creating a missing config file,
// `StrategyConfig::default()` in the engine's tests) silently got the all-zeros
// one. Among other things that meant `min_vol: None`, which makes
// `engine::common_checks::volume_below_min` panic.
//
// There is now exactly one default per type, and it is the trait impl.
// ---------------------------------------------------------------------------

impl Default for AppConfig {
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
}

impl Default for Database {
    fn default() -> Self {
        Self {
            ip: Ipv4Addr::new(127, 0, 0, 1),
            port: 9042,
            keyspace: String::from("okx"),
            data_ttl: (3600 * 24),
            skip_schema_agreement: false,
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

impl Default for Exchange {
    fn default() -> Self {
        Self {
            name: String::from("okx"),
            enable_trading: false,
            authentication: Authentication::default(),
            taker_fee: 0.1,
            channels: Vec::new(),
            maker_fee: 0.08,
            order_ttl: 60,
        }
    }
}

impl Default for Strategy {
    fn default() -> Self {
        let timeframe = 5;
        Self {
            hash: String::new(),
            strategy_type: None,
            top: 5,
            portfolio_size: 5,
            timeframe,
            cooldown: 40,
            timeout: 40,
            min_vol: Some((timeframe * 1500) as f64),
            max_spread_bps: None,
            min_change: 0.1,
            min_change_last_candle: 0.1,
            min_deviation: 0.0,
            max_deviation: 0.5,
            min_rising_candles: Some(3),
            deny_list: None,
            cashout: 10.0,
            quickstart: false,
            stoploss: 3.0,
            avoid_after_stoploss: false,
            sell_floor: None,
            order_type: "ioc".to_string(),
            dip_window: None,
            bounce_window: None,
            min_dip: None,
            min_dip_std: None,
            min_bounce: None,
            min_bounce_std: None,
            avoid_falling_knives: None,
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            enable: true,
            dashboard: true,
            portfolio: true,
            strategy: true,
            system: true,
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
        }
    }
}

impl AppConfig {
    /// Loads `config.toml` (or `$CONFIG_PATH`).
    ///
    /// **Now fails closed.** This used to swallow the error and fall back to
    /// `AppConfig::default()`:
    ///
    /// ```ignore
    /// let cfg = confy::load_path(config_path).unwrap_or_else(|e| {
    ///     log::error!("Loading default config due to:\n{}", e);
    ///     AppConfig::default()
    /// });
    /// ```
    ///
    /// A typo in the config file therefore started a trading process running a
    /// strategy nobody chose, with thresholds nobody wrote, after one ERROR line
    /// that scrolled past. That is precisely the failure mode this codebase has
    /// been suffering from in a slower form. If the config doesn't parse, stop.
    pub fn load() -> Result<Self> {
        let path = env::current_dir()?;
        debug!("The current directory is {}", path.display());
        let config_path =
            env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));

        // `init` panics on a second call; a library-level loader shouldn't be
        // able to abort a process just because it ran twice.
        let _ = env_logger::try_init_from_env(env_logger::Env::new().default_filter_or("info"));

        let cfg: AppConfig = confy::load_path(&config_path)
            .with_context(|| format!("failed to load config from {config_path}"))?;

        debug!("config loaded: {:#?}", cfg);
        Ok(cfg)
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
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| SignError::SecretKeyLength)?;
        mac.update(raw_sign.as_bytes());

        Ok(Signature {
            signature: general_purpose::STANDARD.encode(mac.finalize().into_bytes()),
            timestamp,
        })
    }
}

impl Strategy {
    /// Fills in a `min_vol` floor when the config omits it.
    ///
    /// The old body was `self.min_vol.unwrap_or((self.timeframe * 3500) as f64);`
    /// — it computed a value and threw it away. The `;` made the whole function
    /// a no-op that returned `self` unchanged, and it has presumably never done
    /// anything since it was written.
    pub fn sane_defaults(&mut self) -> &mut Self {
        if self.min_vol.is_none() {
            self.min_vol = Some((self.timeframe * 3500) as f64);
        }
        self
    }

    /// Content hash of the strategy, used as the `strategy` key in
    /// `okx.reports` and `okx.orders`.
    ///
    /// Clears `hash` before hashing so the operation is idempotent. Previously
    /// `hash` was `skip_deserializing` but **not** `skip_serializing`, so it went
    /// into the JSON being hashed: calling `get_hash()` on a config whose hash
    /// was already populated produced a *different* hash. It happened to work
    /// because `main` only ever called it once, on a freshly-loaded config.
    pub fn get_hash(&self) -> String {
        let mut unhashed = self.clone();
        unhashed.hash = String::new();
        let payload = serde_json::to_string_pretty(&unhashed)
            .expect("Strategy is a plain data struct; serialization cannot fail");
        sha1_smol::Sha1::from(payload).digest().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_default_should_set_min_vol_so_entry_checks_cannot_panic() {
        // `engine::common_checks::volume_below_min` expects `Some`.
        assert!(Strategy::default().min_vol.is_some());
    }

    #[test]
    fn get_hash_should_be_idempotent_once_the_hash_is_populated() {
        let mut s = Strategy::default();
        let first = s.get_hash();
        s.hash = first.clone();
        assert_eq!(s.get_hash(), first);
    }

    #[test]
    fn get_hash_should_change_with_strategy_type() {
        let threshold = Strategy::default();
        let reversion = Strategy {
            strategy_type: Some("reversion".to_string()),
            ..Strategy::default()
        };
        assert_ne!(threshold.get_hash(), reversion.get_hash());
    }

    #[test]
    fn sane_defaults_should_actually_assign_min_vol() {
        let mut s = Strategy {
            min_vol: None,
            timeframe: 20,
            ..Strategy::default()
        };
        s.sane_defaults();
        assert_eq!(s.min_vol, Some(70_000.0));
    }

    #[test]
    fn sane_defaults_should_not_clobber_a_configured_min_vol() {
        let mut s = Strategy {
            min_vol: Some(15_000.0),
            ..Strategy::default()
        };
        s.sane_defaults();
        assert_eq!(s.min_vol, Some(15_000.0));
    }
}
