use crate::prelude::*;
use console::Term;
use futures::stream::{self, StreamExt, TryStreamExt};
use pushover_rs::{
    send_pushover_request, Message, MessageBuilder, PushoverResponse, PushoverSound,
};
use rand::thread_rng;
use rand::Rng;
use std::sync::Arc;
use time::Instant;
use tokio::sync::mpsc::{self, Receiver, Sender};

#[derive(Debug)]
pub struct App {
    pub cycles: u64,
    pub time: Time,
    pub logs: Vec<String>,
    pub tokens: Vec<Token>,
    pub cooldown: Duration,
    pub round_id: u64,
    pub term: Term,
    pub tx: Sender<Message>,
    pub rx: Receiver<Message>,
    pub pushover: Pushover,
    pub exchange: Exchange,
    pub deny_list: Vec<String>,
    pub db_session: Arc<Session>,
}

#[derive(Debug, Clone)]
pub struct Time {
    pub started: DateTime<Utc>,
    pub utc: DateTime<Utc>,
    pub now: Instant,
    pub elapsed: Duration,
    pub uptime: Duration,
}
impl Default for Time {
    fn default() -> Self {
        Self {
            //Double check the time coming from the websocket connection or override with your own
            //ts.
            started: Utc::now(), // - Duration::minutes(1) - Duration::seconds(30),
            utc: Utc::now(),     // - Duration::minutes(1) - Duration::seconds(30),
            now: Instant::now(),
            elapsed: Duration::milliseconds(0),
            uptime: Duration::seconds(0),
        }
    }
}
impl App {
    pub async fn init(cfg: &AppConfig) -> Result<Self> {
        let db_uri = format!("{}:{}", cfg.database.ip, cfg.database.port);
        let session: Session = SessionBuilder::new()
            .known_node(db_uri)
            .compression(Some(Compression::Snappy))
            .build()
            .await?;
        session.use_keyspace(&cfg.database.keyspace, false).await?;
        let session = Arc::new(session);
        // Create a Tokio channel with a sender and receiver
        let (tx, rx) = mpsc::channel(100);

        Ok(App {
            round_id: 0,
            cycles: 0,
            cooldown: Duration::seconds(5),
            time: Time::default(),
            logs: Vec::new(),
            tokens: Vec::new(),
            tx,
            rx,
            deny_list: cfg.strategy.deny_list.clone().unwrap_or_default(),
            exchange: cfg.exchange.clone().unwrap_or_default(),
            term: Term::stdout(),
            pushover: cfg.pushover.clone().unwrap_or_default(),
            db_session: session,
        })
    }
    pub async fn send_notifications(&self, account: &Account) -> Result<()> {
        for t in account.portfolio.iter() {
            //send notifications
            if let Some(reason) = t.exit_reason.as_ref() {
                match reason {
                    ExitReason::Cashout => {
                        self.notify(
                            "Cashout Triggered".to_string(),
                            format!(
                                "Token: {} | Change: %{:.2}\nEarnings: {:.2}\nTime Left: {} secs",
                                t.instid, t.report.change, t.report.earnings, t.report.time_left,
                            ),
                        )
                        .await?;
                    }
                    ExitReason::Stoploss => {
                        self.notify(
                            "Stoploss Triggered".to_string(),
                            format!(
                                "Token: {} | Change: %{:.2}\nLoss: {:.2}\nTime Left: {} secs",
                                t.instid, t.report.change, t.report.earnings, t.report.time_left,
                            ),
                        )
                        .await?;
                    }
                    _ => (),
                }
            }
        }
        Ok(())
    }
    pub async fn notify(&self, title: String, msg: String) -> Result<PushoverResponse> {
        let now = self.time.utc.timestamp();
        let message: Message = MessageBuilder::new(&self.pushover.key, &self.pushover.token, &msg)
            .add_title(&title)
            //.add_url("https://pushover.net/", Some("Pushover"))
            .set_priority(-1)
            .set_sound(PushoverSound::GAMELAN)
            .set_timestamp(now as u64)
            .build();

        Ok(send_pushover_request(message).await.unwrap())
    }

    pub fn update_reports(&mut self, mut tokens: Vec<Token>, timeout: i64) -> Vec<Token> {
        for t in tokens.iter_mut() {
            t.change = get_percentage_diff(t.price, t.buy_price);
            if t.change >= t.report.highest {
                t.report.highest = t.change;
                t.report.highest_elapsed = timeout - t.timeout.num_seconds();
            };
            if t.change <= t.report.lowest {
                t.report.lowest = t.change;
                t.report.lowest_elapsed = timeout - t.timeout.num_seconds();
            }
        }
        tokens
    }

    pub async fn update_order_states(&mut self, mut tokens: Vec<Token>) -> Result<Vec<Token>> {
        if self.exchange.enable_trading {
            for token in &mut tokens {
                self.update_live_orders(token).await?;
            }
        } else {
            for token in &mut tokens {
                self.simulate_order_updates(token);
            }
        }
        Ok(tokens)
    }

    async fn update_live_orders(&mut self, token: &mut Token) -> Result<()> {
        if let Some(orders) = &mut token.orders {
            for order in orders
                .iter_mut()
                .filter(|o| o.state == OrderState::Live && o.prev_state != OrderState::Created)
            {
                let got_state = order.get_state(&self.exchange.authentication).await?;
                if order.state != got_state {
                    order.state = got_state.clone();
                    token.status = token::Status::from_order(order);
                }
            }
        }
        Ok(())
    }

    fn simulate_order_updates(&self, token: &mut Token) {
        let mut rng = thread_rng();
        if let Some(orders) = &mut token.orders {
            for order in orders
                .iter_mut()
                .filter(|o| o.state == OrderState::Live && o.prev_state != OrderState::Created)
            {
                let random_state = if rng.gen_bool(1.0 / 3.0) {
                    OrderState::Cancelled
                } else {
                    OrderState::Filled
                };
                let random_order_id = if rng.gen_bool(1.0 / 3.0) {
                    String::new()
                } else {
                    order.cl_ord_id.clone()
                };
                order.id = random_order_id;
                order.state = random_state;
                token.status = token::Status::from_order(order);
            }
        }
    }

    pub async fn get_tickers(&mut self) -> &mut Self {
        for t in self.tokens.iter_mut() {
            //Get ticker data
            let query = format!(
                "select last,sodutc0,volccy24h, high24h, low24h from tickers WHERE instid='{}' order by ts desc limit 1;",
                t.instid,
            );

            if let Some(rows) = self.db_session.query(&*query, &[]).await.unwrap().rows {
                for row in rows.into_typed::<(f64, f64, f64, f64, f64)>() {
                    let (last, open24h, volccy24h, high24h, low24h): (f64, f64, f64, f64, f64) =
                        row.unwrap();
                    t.vol24h = volccy24h;
                    t.change24h = get_percentage_diff(last, open24h);
                    t.range24h = get_percentage_diff(high24h, low24h);
                }
            };
        }
        self
    }
    pub fn update_timeouts(&mut self, mut tokens: Vec<Token>, strategy: &Strategy) -> Vec<Token> {
        self.tokens.iter().for_each(|s| {
            if let Some(token) = tokens.iter_mut().find(|t| t.instid == s.instid) {
                if token
                    .candlesticks
                    .last()
                    .unwrap_or(&Candlestick::new(token.price))
                    .change
                    > strategy.min_change
                {
                    token.timeout = token.config.timeout
                }
            }
        });

        for t in tokens.iter_mut() {
            if !self.tokens.iter_mut().any(|top| top.instid == t.instid) {
                t.timeout = t.timeout - self.time.elapsed;
            };
            //TODO: Expose as config?
            //reset_timer_if_no_change maybe
            if t.change == 0.0 && t.timeout.num_seconds() <= 0 {
                t.timeout = Duration::seconds(strategy.timeout)
            };
        }
        tokens
    }

    pub async fn save_strategy(&self, strategy: &Strategy) -> Result<()> {
        let payload = serde_json::to_string_pretty(&strategy)?;
        let query = format!("INSERT INTO okx.strategies JSON '{}'", payload);
        self.db_session.query(&*query, &[]).await?;
        Ok(())
    }

    pub fn set_cooldown(&mut self, num: i64) -> &mut Self {
        self.cooldown = Duration::milliseconds(num * 1000);
        self
    }

    pub async fn update_candles(
        &self,
        timeframe: i64,
        tokens: Vec<Token>,
    ) -> Result<Vec<Token>, Box<dyn Error>> {
        let dt = self
            .time
            .utc
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();

        //last -timeframe- candles
        let get_candles_query = self
            .db_session
            .prepare("SELECT * FROM candle1m WHERE instid=? AND ts <= ? LIMIT ?")
            .await?;

        //Last min tickers
        let get_tickers_query = self
            .db_session
            .prepare(
                "SELECT last, lastsz, ts FROM tickers WHERE instid=? AND ts >= ? order by ts asc ",
            )
            .await?;

        stream::iter(tokens.into_iter().map(|mut token| {
            let get_candle_stmt = get_candles_query.clone();
            let get_ticker_stmt = get_tickers_query.clone();
            async move {
                if let Some(rows) = self
                    .db_session
                    .execute(
                        &get_candle_stmt,
                        (&token.instid, dt.timestamp_millis(), timeframe as i32),
                    )
                    .await?
                    .rows
                {
                    for row in rows.into_typed::<Candlestick>() {
                        let candle = row.unwrap_or(Candlestick::new(token.price));
                        token.add_or_update_candle(candle)
                    }
                };

                let dt = self.time.utc;
                let last_min = match token.candlesticks.last() {
                    Some(candlestick) if candlestick.ts.num_minutes() == dt.minute() as i64 => {
                        dt - Duration::seconds(1)
                    }
                    _ => dt.with_second(0).unwrap().with_nanosecond(0).unwrap(),
                };

                if let Some(rows) = self
                    .db_session
                    .execute(
                        &get_ticker_stmt,
                        (&token.instid, last_min.timestamp_millis()),
                    )
                    .await?
                    .rows
                {
                    let tickers: Vec<(f64, f64, Duration)> = rows
                        .into_typed::<(f64, f64, Duration)>()
                        .filter_map(Result::ok)
                        .collect();

                    token.price = if let Some(ticker) = tickers.last() {
                        ticker.0
                    } else {
                        token.price
                    };
                    token.candlesticks.sort_by(|a, b| {
                        a.ts.partial_cmp(&b.ts)
                            .expect("unable to compare timestamps")
                    });
                    let mut last_candle =
                        Candlestick::from_tickers(&token.instid, &tickers).unwrap_or_default();
                    if last_candle.change == 0.0 {
                        last_candle.open = token.price;
                        last_candle.high = token.price;
                        last_candle.low = token.price;
                        last_candle.close = token.price;
                    }
                    token.add_or_update_candle(last_candle);
                };

                while token.candlesticks.len() > timeframe as usize {
                    token.candlesticks.remove(0);
                }
                token.change = 0.0;
                token.sum_candles();
                token.candlesticks.sort_by(|a, b| {
                    a.ts.partial_cmp(&b.ts)
                        .expect("unable to compare timestamps")
                });
                Ok(token)
            }
        }))
        .buffered(5000)
        .try_collect::<Vec<Token>>()
        .await
    }
    pub async fn buy_tokens(
        &mut self,
        mut account: Account,
        strategy: &Strategy,
    ) -> Result<Account> {
        for token in self.tokens.iter_mut() {
            if token.cooldown <= Duration::milliseconds(0)
                && !account.portfolio.iter().any(|p| token.instid == p.instid)
            {
                account.add_token(token, strategy);
                token.cooldown = self.cooldown;
            }
        }

        for t in account.portfolio.iter_mut() {
            let buy_orders = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|o| o.side == Side::Buy);

            //Only create buy order of no other buy order is live
            if !buy_orders {
                let fee = calculate_fees(account.balance.spendable, self.exchange.taker_fee);
                //will have this amount to buy tokens after deducting fees
                let spendable_after_fees = account.balance.spendable - fee;
                //token balance to buy
                t.buy_price = t.price;
                t.balance.start = spendable_after_fees / t.price;

                //Configure Token using previously saved reports
                t.configure_from_report(strategy, &self.db_session).await;

                {
                    // Create order
                    let order = t
                        .buy(
                            self.exchange.enable_trading,
                            account.authentication.clone(),
                            &strategy.hash,
                        )
                        .await?
                        .orders
                        .as_ref()
                        .and_then(|orders| orders.last())
                        .unwrap();

                    order.save(&self.db_session).await?;
                    let log_line = self.build_order_log(order);
                    self.logs.push(log_line);
                    self.round_id += 1;
                }

                t.report = Report::new(self.round_id, &strategy.hash, t);
            }
        }
        Ok(account)
    }

    pub fn tag_invalid_tokens(
        &mut self,
        mut account: Account,
        strategy: &Strategy,
    ) -> Result<Account> {
        for t in account.portfolio.iter_mut() {
            let found = self.tokens.iter().any(|s| t.instid == s.instid);
            if t.status == token::Status::Trading {
                t.exit_reason = t.get_exit_reason(strategy, found);
            }
            if t.exit_reason.is_some() && t.status != token::Status::Exited {
                t.status = token::Status::Selling;
                t.report.reason = t.exit_reason.as_ref().unwrap().to_string();
            }
        }
        Ok(account)
    }
    pub fn build_report_log(&self, order: &Order) -> String {
        format!(
                    "[{timestamp}] Order: {order_id} > {state}: {side} {token} - order type {ord_type} - price: {price} - size: {size} | Response: {response}",
                    timestamp = self.time.utc.format("%Y-%m-%d %H:%M:%S"),
                    order_id = order.id,
                    state = order.state.to_string(),
                    token = order.inst_id,
                    side = order.side.to_string(),
                    ord_type = order.ord_type,
                    price = order.px,
                    size = order.sz,
                    response = if self.exchange.enable_trading {
                        match order.clone().response {
                            Some(r) => r.data[0].clone().s_msg,
                            None => format!("{:?}", order.response)
                        }
                    } else {
                        String::from("N/A")
                    }
                )
    }
    pub fn build_order_log(&self, order: &Order) -> String {
        format!(
                    "[{timestamp}] Order: {order_id} > {state}: {side} {token} - order type {ord_type} - price: {price} - size: {size} | Response: {response}",
                    timestamp = self.time.utc.format("%Y-%m-%d %H:%M:%S"),
                    order_id = order.id,
                    state = order.state.to_string(),
                    token = order.inst_id,
                    side = order.side.to_string(),
                    ord_type = order.ord_type,
                    price = order.px,
                    size = order.sz,
                    response = if self.exchange.enable_trading {
                        match order.clone().response {
                            Some(r) => r.data[0].clone().s_msg,
                            None => format!("{:?}", order.response)
                        }
                    } else {
                        String::from("N/A")
                    }
                )
    }
    pub async fn sell_tokens(
        &mut self,
        mut account: Account,
        strategy: &Strategy,
    ) -> Result<Account> {
        for t in account.portfolio.iter_mut() {
            let filled_sell_orders = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|o| o.side == Side::Sell && o.state == OrderState::Filled);
            let failed_sell_orders = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|o| o.side == Side::Sell && o.state == OrderState::Failed);

            if t.status == token::Status::Selling
                && (!filled_sell_orders || failed_sell_orders)
                && t.exit_reason.is_some()
                && (t.balance.available * t.price) >= 2.0
            {
                t.balance.available -= calculate_fees(t.balance.available, self.exchange.taker_fee);
                {
                    let order = t
                        .sell(
                            self.exchange.enable_trading,
                            account.authentication.clone(),
                            &strategy.hash,
                        )
                        .await?
                        .orders
                        .as_ref()
                        .and_then(|orders| orders.last())
                        .unwrap();
                    order.save(&self.db_session).await?;
                    let log_line = self.build_order_log(order);
                    self.logs.push(log_line);
                }

                //build up deny list if stoploss.
                let denied = self
                    .deny_list
                    .iter()
                    .any(|i| format!("{}-USDT", i) == t.instid);
                if t.exit_reason == Some(ExitReason::Stoploss)
                    && strategy.avoid_after_stoploss
                    && !denied
                {
                    self.deny_list.push(t.instid.replace("-USDT", ""))
                };

                // Create token report
                let usdt_balance = t.balance.available * t.price;
                let usdt_fee = calculate_fees(usdt_balance, self.exchange.taker_fee);
                let usdt_balance_after_fees = usdt_balance - usdt_fee;
                let earnings = usdt_balance_after_fees - (t.balance.start * t.buy_price);

                t.report.earnings = earnings;

                t.report.save(&self.db_session).await?;
            }
        }
        Ok(account)
    }
    pub fn filter_invalid(&mut self, strategy: &Strategy, spendable: f64) -> &mut Self {
        let deny_list = self.deny_list.clone();
        self.tokens
            .retain(|t| t.is_valid(&deny_list, strategy, spendable));
        self.tokens.sort_by(|a, b| {
            b.change
                .partial_cmp(&a.change)
                .expect("unable to compare change")
        });
        self
    }

    pub fn update_cooldown(&mut self, portfolio: &[Token]) -> &mut Self {
        for t in self.tokens.iter_mut() {
            if portfolio.iter().any(|x| t.instid == x.instid) {
                t.cooldown = self.cooldown
            } else {
                t.cooldown = t.cooldown - self.time.elapsed
            }
        }
        self
    }
    pub fn clean_top(&mut self, num: usize) -> &mut Self {
        while self.tokens.len() > num {
            self.tokens.pop();
        }
        self
    }

    pub async fn fetch_tokens(&mut self, timeframe: i64) -> &mut Self {
        let xdt = self.time.utc - Duration::minutes(timeframe);
        let dt = xdt.with_second(0).unwrap().with_nanosecond(0).unwrap();

        let query = format!(
            "SELECT * FROM okx.candle1m WHERE ts >= '{}'",
            dt.timestamp_millis()
        );

        if let Some(rows) = self.db_session.query(&*query, &[]).await.unwrap().rows {
            for row in rows.into_typed::<Candlestick>() {
                let candle = row.unwrap();
                if let Some(token) = self.tokens.iter_mut().find(|t| candle.instid == t.instid) {
                    token.add_or_update_candle(candle);
                } else {
                    let mut new_token =
                        Token::new(&candle.instid).set_cooldown(self.cooldown.num_seconds());
                    new_token.add_or_update_candle(candle);
                    self.tokens.push(new_token);
                }
            }
        };
        self
    }
}
