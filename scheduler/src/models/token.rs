use crate::prelude::*;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub enum Status {
    #[default]
    Waiting,
    Buying,
    Trading,
    Selling,
    Exited,
}

impl Status {
    pub fn from_order(order: &Order) -> Self {
        match order.state {
            OrderState::Filled => match order.side {
                Side::Buy => Status::Trading,
                Side::Sell => Status::Exited,
            },
            OrderState::Cancelled | OrderState::Failed => match order.side {
                Side::Buy => Status::Waiting,
                Side::Sell => Status::Trading,
            },
            _ => match order.side {
                Side::Buy => Status::Buying,
                Side::Sell => Status::Selling,
            },
        }
    }
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
    pub fn from_tickers(instid: &str, tickers: &[(f64, f64, Duration)]) -> Option<Candlestick> {
        if tickers.is_empty() {
            return None;
        }

        let open = tickers.first()?.0;
        let close = tickers.last()?.0;

        let mut high = tickers[0].0;
        let mut low = tickers[0].0;
        let mut vol = 0.0;

        for &(price, size, _) in tickers {
            high = high.max(price);
            low = low.min(price);
            vol += size * price;
        }
        let change = get_percentage_diff(close, open);
        let range = get_percentage_diff(high, low);
        let ts = tickers.last()?.2;
        let datetime =
            DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp_opt(0, 0)?, Utc) + ts;

        Some(Candlestick {
            instid: instid.to_string(),
            ts: Duration::milliseconds(
                datetime
                    .with_second(0)
                    .unwrap()
                    .with_nanosecond(0)
                    .unwrap()
                    .timestamp_millis(),
            ),
            change,
            close,
            high,
            low,
            open,
            range,
            vol,
        })
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
        if let Some(existing_candle) = self
            .candlesticks
            .iter_mut()
            .find(|c| candle.ts.num_minutes() == c.ts.num_minutes())
        {
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
        let sell_balance = match self.balance.available {
            x if x > 1_000_000.0 => ((x / 1_000_000.0).floor() * 1_000_000.0) - 1.0,
            x if x > 100_000.0 => (x / 100_000.0).floor() * 100_000.0,
            x if x > 1_000.0 => (x / 1_000.0).floor() * 1_000.0,
            _ => self.balance.available, // if it's less than 1000, leave it as it is
        };
        let mut order = trade::Order::new(
            &self.instid,
            self.price.to_string(),
            sell_balance.to_string(),
            Side::Sell,
            strategy,
        );

        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }
    pub fn get_exit_reason(&self, strategy: &Strategy, token_found: bool) -> Option<ExitReason> {
        let sell_floor = strategy.sell_floor.unwrap_or(0.0);
        let timeout_threshold = Duration::seconds(strategy.timeout - 5);
        let reason = if self.timeout.num_seconds() <= 0 {
            Some(ExitReason::Timeout)
        } else if self.change <= -strategy.stoploss {
            Some(ExitReason::Stoploss)
        } else if self.change >= strategy.cashout && self.timeout < timeout_threshold {
            Some(ExitReason::Cashout)
        } else if self.change >= sell_floor && self.timeout < timeout_threshold && !token_found {
            Some(ExitReason::FloorReached)
        } else {
            None
        };
        reason
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
    pub fn is_valid(&self, deny_list: &[String], strategy: &Strategy, spendable: f64) -> bool {
        let denied = deny_list
            .iter()
            .any(|i| format!("{}-USDT", i) == self.instid);
        let pcc = self
            .candlesticks
            .iter()
            .filter(|x| x.vol > spendable)
            .count();
        let non_zero_candles = self.candlesticks.iter().filter(|x| x.vol > 0.00).count();
        let blank_candle = Candlestick::new();
        let last_candle = self.candlesticks.last().unwrap_or(&blank_candle);

        !denied
            // No missing candles in our data
            && self.candlesticks.len() >= strategy.timeframe as usize
            && self.candlesticks.len() == non_zero_candles
            // At least half of the candles should have higher volume than our spendable
            && pcc >= strategy.timeframe as usize / 2
            && self.change >= strategy.min_change
            && self.std_deviation >= strategy.min_deviation
            && last_candle.vol >= spendable
            && last_candle.change > strategy.min_change_last_candle
            && self.vol > strategy.min_vol.unwrap()
    }

    pub fn sum_candles(&mut self) -> &mut Self {
        //check if vol is enough in the selected timeframe
        self.vol = self.candlesticks.iter().map(|x| x.vol).sum();
        // Sum vol, changes, and range from candlesticks
        let (vol, change, range) = self.candlesticks.iter().fold(
            (0.0, 0.0, 0.0),
            |(vol_acc, change_acc, range_acc), x| {
                (vol_acc + x.vol, change_acc + x.change, range_acc + x.range)
            },
        );
        self.vol = vol;
        if self.status == token::Status::Waiting {
            self.change = change;
        }
        self.range = range;

        let changes: Vec<f32> = self
            .candlesticks
            .clone()
            .into_iter()
            .map(|x| x.change)
            .collect();
        self.std_deviation = std_deviation(&changes).unwrap_or(0.0);
        self
    }
    pub fn set_status(&mut self, order: &Order) -> &mut Self {
        self.status = match order.state {
            OrderState::Filled => match order.side {
                Side::Buy => token::Status::Trading,
                Side::Sell => token::Status::Exited,
            },
            OrderState::Cancelled => match order.side {
                Side::Buy => token::Status::Waiting,
                Side::Sell => token::Status::Trading,
            },
            _ => match order.side {
                Side::Buy => token::Status::Buying,
                Side::Sell => token::Status::Selling,
            },
        };
        self
    }
}
