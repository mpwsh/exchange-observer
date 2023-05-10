use crate::prelude::*;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub enum Status {
    Buying,
    Selling,
    #[default]
    Waiting,
    Trading,
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Token {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    #[serde(rename = "px")]
    pub price: f64,
    pub change: f32,
    pub std_deviation: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
    pub balance: Balance,
    pub earnings: f64,
    pub status: Status,
    pub fees_deducted: bool,
    pub vol: f64,
    pub vol24h: f64,
    pub change24h: f32,
    pub range: f32,
    pub range24h: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub cooldown: Duration,
    pub candlesticks: Vec<Candlestick>,
    pub config: Config,
    pub orders: Option<Vec<trade::Order>>,
    pub exit_reason: Option<trade::ExitReason>,
    pub report: Report,
}

#[serde_with::serde_as]
#[derive(FromRow, Serialize, Deserialize, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub ts: Duration,
    pub change: f32,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f32,
    pub vol: f64,
}

impl Default for Candlestick {
    fn default() -> Self {
        Candlestick::new()
    }
}
impl Candlestick {
    pub fn new() -> Self {
        Self {
            instid: String::new(),
            ts: Duration::seconds(0),
            change: 0.0,
            close: 0.0,
            high: 0.0,
            low: 0.0,
            open: 0.0,
            range: 0.0,
            vol: 0.0,
        }
    }
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub sell_floor: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: Duration::seconds(0),
            sell_floor: 0.0,
        }
    }
}
impl Token {
    pub fn new(instid: &str) -> Self {
        Self {
            round_id: 0,
            instid: instid.to_string(),
            buy_price: 0.0,
            price: 0.0,
            std_deviation: 0.0,
            balance: Balance {
                current: 0.0,
                start: 0.0,
                available: 0.0,
                spendable: 0.0,
            },
            earnings: 0.00,
            fees_deducted: false,
            change24h: 0.0,
            range: 0.0,
            vol: 0.0,
            vol24h: 0.0,
            range24h: 0.0,
            timeout: Duration::seconds(0),
            cooldown: Duration::seconds(0),
            config: Config::default(),
            exit_reason: None,
            change: 0.00,
            candlesticks: Vec::new(),
            orders: None,
            report: Report::default(),
            status: token::Status::Waiting,
        }
    }
    pub fn set_cooldown(mut self, cooldown: i64) -> Self {
        self.cooldown = Duration::seconds(cooldown);
        self
    }

    pub fn add_or_update_candle(&mut self, candle: Candlestick) {
        if let Some(existing_candle) = self.candlesticks.iter_mut().find(|c| candle.ts == c.ts) {
            *existing_candle = candle;
        } else {
            self.candlesticks.push(candle);
        }
    }

    pub async fn buy(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        strategy: &str,
    ) -> Result<&Self> {
        let mut order = trade::Order::new(
            &self.instid,
            self.buy_price.to_string(),
            self.balance.start.to_string(),
            Side::Buy,
            strategy,
        );
        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }

    pub async fn sell(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        strategy: &str,
    ) -> Result<&Self> {
        let mut order = trade::Order::new(
            &self.instid,
            self.price.to_string(),
            self.balance.available.to_string(),
            Side::Sell,
            strategy,
        );

        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }
    pub fn get_exit_reason(&self, strategy: &Strategy, token_found: bool) -> Option<ExitReason> {
        let sell_floor = self
            .config
            .sell_floor
            .max(strategy.sell_floor.unwrap_or(0.0));

        if self.timeout.num_seconds() <= 0 {
            Some(ExitReason::Timeout)
        } else if self.change <= -strategy.stoploss {
            Some(ExitReason::Stoploss)
        } else if self.change >= strategy.cashout
            && self.timeout < Duration::seconds(strategy.timeout - 5)
        {
            Some(ExitReason::Cashout)
        } else if self.change >= sell_floor
            && self.timeout < Duration::seconds(strategy.timeout - 5)
            && !token_found
        {
            Some(ExitReason::FloorReached)
        } else {
            None
        }
    }
    pub async fn configure_from_report(
        &mut self,
        strategy: &Strategy,
        db_session: &Session,
    ) -> &Self {
        let mut time_deviation = Vec::new();
        let mut change_deviation = Vec::new();
        //Find old reports and try to get better defaults
        let mut results_count = 0;

        let query = format!(
                "select count(instid) from okx.reports where instid='{}' and strategy='{}' allow filtering;",
                self.instid,
                &strategy.hash
                );

        if let Some(rows) = db_session.query(&*query, &[]).await.unwrap().rows {
            for row in rows.into_typed::<(i64,)>() {
                let (c,): (i64,) = row.unwrap();
                results_count = c;
            }
        };
        if results_count != 0 {
            let query = format!(
                "select highest, highest_elapsed from okx.reports where instid='{}' and strategy='{}' allow filtering;",
                self.instid,
                strategy.hash,
                );
            if let Some(rows) = db_session.query(&*query, &[]).await.unwrap().rows {
                for row in rows.into_typed::<(f32, i64)>() {
                    let (highest, highest_elapsed): (f32, i64) = row.unwrap();
                    change_deviation.push(highest);
                    time_deviation.push(highest_elapsed as f32);
                }
            };
            self.config.sell_floor = std_deviation(&change_deviation[..]).unwrap();
            self.config.timeout = Duration::seconds(strategy.timeout);
            //    Duration::seconds(std_deviation(&time_deviation[..]).unwrap() as i64);
            if self.config.timeout.num_seconds() >= 30 || self.config.sell_floor >= 0.1 {
                /*
                self.logs.push(format!(
                    "[DISABLED] Found reports for {}, using new floor {} and timeout {}",
                    self.instid, self.config.sell_floor, self.config.timeout
                ));*/
                self.timeout = self.config.timeout;
            } else {
                self.timeout = Duration::seconds(strategy.timeout);
                self.config.timeout = self.timeout;
                self.config.sell_floor = strategy.sell_floor.unwrap();
            };
        } else {
            self.timeout = Duration::seconds(strategy.timeout);
            self.config.timeout = self.timeout;
            self.config.sell_floor = strategy.sell_floor.unwrap();
        };
        self
    }
}
