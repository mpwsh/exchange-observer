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
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub buy_ts: Duration,
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
        Candlestick::new(0.0)
    }
}
impl Candlestick {
    pub fn new(open: f64) -> Self {
        Self {
            instid: String::new(),
            ts: Duration::milliseconds(
                Utc::now()
                    .with_second(0)
                    .unwrap()
                    .with_nanosecond(0)
                    .unwrap()
                    .timestamp_millis(),
            ),
            change: 0.0,
            close: open,
            high: open,
            low: open,
            open,
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
        let time = if datetime.timestamp_millis() == 0 {
            Utc::now()
        } else {
            datetime
        };

        Some(Candlestick {
            instid: instid.to_string(),
            ts: Duration::milliseconds(
                time.with_second(0)
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
            buy_ts: Duration::seconds(0),
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
        strategy: &Strategy,
    ) -> Result<&Self> {
        self.buy_price = self.price;
        let mut order = trade::Order::new(
            &self.instid,
            self.buy_price.to_string(),
            self.balance.start.to_string(),
            Side::Buy,
            &strategy.order_type,
            &strategy.hash,
        );
        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }

    pub async fn sell(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        strategy: &Strategy,
    ) -> Result<&Self> {
        let sell_balance = if trade_enabled {
            Account::get_balance(&self.instid.replace("-USDT", ""), &auth)
                .await
                .unwrap_or(self.balance.available)
        } else {
            self.balance.available
            /*
            match self.balance.available {
                x if x > 1_000_000.0 => ((x / 1_000_000.0).floor() * 1_000_000.0) - 1.0,
                x if x > 100_000.0 => (x / 100_000.0).floor() * 100_000.0,
                x if x > 1_000.0 => (x / 1_000.0).floor() * 1_000.0,
                _ => self.balance.available,
            }*/
        };

        //Count sell atempts and sell to market_price if above x
        let sell_count = self
            .orders
            .clone()
            .unwrap_or_default()
            .iter()
            .filter(|o| o.side == Side::Sell)
            .count();

        //sell to market price if we tried to sell 5 times
        let ord_type = match sell_count {
            x if x <= 5 => &strategy.order_type,
            _ => "market",
        };

        let mut order = trade::Order::new(
            &self.instid,
            self.price.to_string(),
            sell_balance.to_string(),
            Side::Sell,
            ord_type,
            &strategy.hash,
        );

        order.publish(trade_enabled, &auth).await?;

        if order
            .response
            .as_ref()
            .map_or(false, |response| response.code.parse::<i64>().unwrap() != 0)
        {
            order.state = OrderState::Failed;
        }

        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }

    pub fn get_exit_reason(&self, strategy: &Strategy, token_found: bool) -> Option<ExitReason> {
        //thresholds
        let timeout_threshold = Duration::seconds(strategy.timeout - 5);
        let sell_floor = strategy.sell_floor.unwrap_or(0.0);
        let volume_threshold = strategy
            .min_vol
            .unwrap_or((strategy.timeframe * 1600) as f64)
            / strategy.timeframe as f64;
        let low_volume_condition = |c: &&Candlestick| c.vol < volume_threshold;

        let low_volume = self
            .candlesticks
            .iter()
            .filter(low_volume_condition)
            .count();

        // Take the last 5 candlesticks (or fewer if there are not enough)
        let last_candles_change: Vec<_> = self
            .candlesticks
            .iter()
            .rev()
            .take(5)
            .filter(|&c| c.change == 0.0)
            .collect::<Vec<&Candlestick>>();

        if last_candles_change.len() >= (strategy.timeframe / 2) as usize {
            return Some(ExitReason::LowChange);
        }

        if self.timeout.num_seconds() <= 0 {
            return Some(ExitReason::Timeout);
        }

        if self.change <= -strategy.stoploss {
            return Some(ExitReason::Stoploss);
        }

        if self.change >= strategy.cashout {
            return Some(ExitReason::Cashout);
        }

        if self.change >= sell_floor && self.timeout < timeout_threshold && !token_found {
            return Some(ExitReason::FloorReached);
        }

        if low_volume as i64 >= strategy.timeframe / 2 {
            //Half of the candles in the selected timeframe show volume lower than our spendable
            //  return Some(ExitReason::LowVolume);
        }

        None
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
        if results_count >= 1 {
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
            let change_target = std_deviation(&change_deviation[..]).unwrap();
            let timeout_target =
                Duration::seconds(std_deviation(&time_deviation[..]).unwrap() as i64);

            if timeout_target.num_seconds() < 30 {
                self.config.timeout = Duration::seconds(strategy.timeout);
                self.timeout = self.config.timeout;
            } else {
                self.timeout = timeout_target;
                self.config.timeout = timeout_target;
            };
            if change_target < 0.1 {
                self.config.sell_floor = strategy.sell_floor.unwrap();
            } else {
                self.config.sell_floor = change_target;
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

        let cchange = self.candlesticks.iter().filter(|x| x.change > 0.0).count();

        let blank_candle = Candlestick::new(self.price);
        let last_candle = self.candlesticks.last().unwrap_or(&blank_candle);

        !denied
            // No missing candles in our data
            && self.candlesticks.len() >= strategy.timeframe as usize
            // At least half of the candles should have higher volume than our spendable
            && pcc >= strategy.timeframe as usize / 2
            // At least half of the candles have some change
            && cchange >= strategy.timeframe as usize / 2
            && self.change >= strategy.min_change
            && (self.std_deviation >= strategy.min_deviation && self.std_deviation <= strategy.max_deviation)
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

    pub fn update_reports(&mut self, timeout: i64) -> &mut Self {
        let mut t = self;
        t.report.time_left = t.timeout.num_seconds();
        t.change = get_percentage_diff(t.price, t.buy_price);
        if t.change >= t.report.highest {
            t.report.highest = t.change;
            t.report.highest_elapsed = timeout - t.timeout.num_seconds();
        };
        if t.change <= t.report.lowest {
            t.report.lowest = t.change;
            t.report.lowest_elapsed = timeout - t.timeout.num_seconds();
        }
        t
    }

    pub async fn update_orders(
        &mut self,
        enable_trading: bool,
        auth: &Authentication,
    ) -> Result<&mut Self> {
        if let Some(orders) = &mut self.orders {
            for order in orders.iter_mut().filter(|o| {
                o.state == OrderState::Live
                    && o.prev_state != OrderState::Created
                    && o.state != OrderState::Filled
            }) {
                log::info!(
                    "[{}] Checking order state. result: {}",
                    self.instid,
                    order.state.to_string()
                );
                if enable_trading {
                    log::info!("[{}] Retrieving order state form exchange", self.instid);
                    let got_state = order.get_state(auth).await?;
                    if order.state != got_state {
                        order.state = got_state.clone();
                    }
                } else {
                    use rand::thread_rng;
                    use rand::Rng;
                    let mut rng = thread_rng();
                    let random_state = if rng.gen_bool(1.0 / 6.0) {
                        OrderState::Filled
                    } else {
                        OrderState::Cancelled
                    };

                    order.state = random_state;
                    order.id = order.cl_ord_id.clone();
                };
                self.status = Status::from_order(order);
            }
        }
        Ok(self)
    }
}
