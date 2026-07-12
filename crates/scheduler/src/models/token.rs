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

#[derive(DeserializeRow, Serialize, Deserialize, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    pub ts: DateTime<Utc>,
    pub change: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f64,
    // Column in Scylla is `volume` (renamed during typed-model migration),
    // but the field stays `vol` here for backwards compat with the console's
    // wire format and existing scheduler code. The scylla attribute maps to
    // the actual DB column name.
    #[scylla(rename = "volume")]
    pub vol: f64,
}

impl Candlestick {
    /// Blank candle at `now` (truncated to the minute). Time is passed in as
    /// data — model constructors never read ambient time.
    pub fn new(open: f64, now: DateTime<Utc>) -> Self {
        Self {
            instid: String::new(),
            ts: now.with_second(0).unwrap().with_nanosecond(0).unwrap(),
            change: 0.0,
            close: open,
            high: open,
            low: open,
            open,
            range: 0.0,
            vol: 0.0,
        }
    }
    pub fn from_tickers(
        instid: &str,
        tickers: &[(f64, f64, DateTime<Utc>)],
        now: DateTime<Utc>,
    ) -> Option<Candlestick> {
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
        let time = if ts.timestamp_millis() == 0 { now } else { ts };

        Some(Candlestick {
            instid: instid.to_string(),
            ts: time,
            change,
            close,
            high,
            low,
            open,
            range,
            vol,
        })
    }

    /// Trimmed, read-only view of this candle for the strategy boundary.
    pub fn view(&self) -> Candle {
        Candle {
            ts: self.ts,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            change: self.change,
            range: self.range,
            vol: self.vol,
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
            .find(|c| candle.ts.minute() == c.ts.minute())
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
        config: &StrategyConfig,
        now: DateTime<Utc>,
        meta: InstrumentMeta,
    ) -> Result<&Self> {
        // Round the entry price to the exchange's tick_sz so the order
        // book match logic can find our bid. Behavior when meta is UNKNOWN
        // (tick_sz = 0.0): pass through, same as before this change.
        self.buy_price = floor_to_step(self.price, meta.tick_sz);
        let mut order = trade::Order::new(
            &self.instid,
            self.buy_price.to_string(),
            self.balance.start.to_string(),
            Side::Buy,
            &config.order_type,
            &config.hash,
            now,
        );
        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }
    /// Applies a strategy exit decision. Mirrors the old `tag_invalid`: while
    /// the position is `Trading` the exit reason is *replaced* by the
    /// decision (a `Hold` clears any stale reason), and any set reason moves
    /// the token to `Selling` and stamps the report.
    pub fn apply_exit(&mut self, decision: ExitDecision) -> &mut Self {
        if self.status == token::Status::Trading {
            self.exit_reason = match decision {
                ExitDecision::Exit(reason) => Some(reason),
                ExitDecision::Hold => None,
            };
        }
        if let Some(reason) = &self.exit_reason {
            self.status = token::Status::Selling;
            self.report.reason = reason.to_string();
        }
        self
    }

    pub async fn sell(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        config: &StrategyConfig,
        now: DateTime<Utc>,
        meta: InstrumentMeta,
    ) -> Result<&Self> {
        let raw_sell_balance = if trade_enabled {
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
        // Round sell size down to the instrument's lot_sz. Selling too
        // much would leave a dust position we can't close cleanly; floor
        // ensures we never send more than we hold. Cache miss preserves
        // pre-rounding behavior (lot_sz = 0.0 → pass-through).
        let sell_balance = floor_to_step(raw_sell_balance, meta.lot_sz);
        let sell_price = floor_to_step(self.price, meta.tick_sz);

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
            x if x <= 5 => &config.order_type,
            _ => "market",
        };

        let mut order = trade::Order::new(
            &self.instid,
            sell_price.to_string(),
            sell_balance.to_string(),
            Side::Sell,
            ord_type,
            &config.hash,
            now,
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

    /// Read-only snapshot of this open position for `Strategy::should_exit`.
    /// `still_listed` is whether the token still appears in the scheduler's
    /// valid-token list this cycle.
    pub fn position_view(&self, still_listed: bool) -> PositionView {
        PositionView {
            instid: self.instid.clone(),
            change: f64::from(self.change),
            timeout: self.timeout,
            still_listed,
            candles: self.candlesticks.iter().map(Candlestick::view).collect(),
        }
    }

    pub async fn configure_from_report(
        &mut self,
        config: &StrategyConfig,
        db_session: &Session,
    ) -> &Self {
        let mut time_deviation = Vec::new();
        let mut change_deviation = Vec::new();
        //Find old reports and try to get better defaults
        let mut results_count = 0;

        let query = format!(
            "select count(instid) from okx.reports where instid='{}' and strategy='{}' allow filtering;",
            self.instid, &config.hash
        );

        let result = db_session.query_unpaged(&*query, &[]).await.unwrap();
        let rows_result = result.into_rows_result().unwrap();
        for row in rows_result.rows::<(i64,)>().unwrap() {
            let (c,) = row.unwrap();
            results_count = c;
        }
        if results_count >= 1 {
            let query = format!(
                "select highest, highest_elapsed from okx.reports where instid='{}' and strategy='{}' allow filtering;",
                self.instid, config.hash,
            );
            let result = db_session.query_unpaged(&*query, &[]).await.unwrap();
            let rows_result = result.into_rows_result().unwrap();
            for row in rows_result.rows::<(f64, i64)>().unwrap() {
                let (highest, highest_elapsed) = row.unwrap();
                // Report.highest is `double` after schema migration; cast to f32 for
                // Token's std_deviation helper which operates on f32 slices.
                change_deviation.push(highest as f32);
                time_deviation.push(highest_elapsed as f32);
            }
            let change_target = std_deviation(&change_deviation[..]).unwrap();
            let timeout_target =
                Duration::seconds(std_deviation(&time_deviation[..]).unwrap() as i64);

            if timeout_target.num_seconds() < 30 {
                self.config.timeout = Duration::seconds(config.timeout);
                self.timeout = self.config.timeout;
            } else {
                self.timeout = timeout_target;
                self.config.timeout = timeout_target;
            };
            if change_target < 0.1 {
                self.config.sell_floor = config.sell_floor.unwrap();
            } else {
                self.config.sell_floor = change_target;
            };
        } else {
            self.timeout = Duration::seconds(config.timeout);
            self.config.timeout = self.timeout;
            self.config.sell_floor = config.sell_floor.unwrap();
        };
        self
    }

    /// Read-only snapshot of this candidate token for
    /// `Strategy::should_enter`. The threshold checks that used to live here
    /// (`is_valid`) are now `engine::threshold::Thresholds::entry_decision`.
    pub fn entry_view(&self, denied: bool) -> TokenView {
        TokenView {
            instid: self.instid.clone(),
            price: self.price,
            change: f64::from(self.change),
            std_deviation: f64::from(self.std_deviation),
            vol: self.vol,
            denied,
            candles: self.candlesticks.iter().map(Candlestick::view).collect(),
        }
    }

    pub fn sum_candles(&mut self) -> &mut Self {
        //check if vol is enough in the selected timeframe
        self.vol = self.candlesticks.iter().map(|x| x.vol).sum();
        // Sum vol, changes, and range from candlesticks (all f64 from DB).
        let (vol, change, range) = self.candlesticks.iter().fold(
            (0.0_f64, 0.0_f64, 0.0_f64),
            |(vol_acc, change_acc, range_acc), x| {
                (vol_acc + x.vol, change_acc + x.change, range_acc + x.range)
            },
        );
        self.vol = vol;
        // Token.change/range are f32 for the console wire format — cast at the boundary.
        if self.status == token::Status::Waiting {
            self.change = change as f32;
        }
        self.range = range as f32;

        let changes: Vec<f32> = self
            .candlesticks
            .clone()
            .into_iter()
            .map(|x| x.change as f32)
            .collect();
        self.std_deviation = std_deviation(&changes).unwrap_or(0.0);
        self
    }

    pub fn update_reports(&mut self, timeout: i64) -> &mut Self {
        let t = self;
        t.report.time_left = t.timeout.num_seconds();
        t.change = get_percentage_diff(t.price, t.buy_price) as f32;
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
                    use rand::{thread_rng, Rng};
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
