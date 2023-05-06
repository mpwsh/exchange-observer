use crate::utils::*;
use crate::{
    models::{Candlestick, Reason, Report, Selected, Token, TokenStatus, TradeOrder},
    Account,
};
use crate::{time::Instant, DateTime, Duration, SecondsFormat, Utc};
use anyhow::Result;
use chrono::Timelike;
use console::Term;
use exchange_observer::{Exchange, Pushover, Strategy};
use pushover_rs::{
    send_pushover_request, Message, MessageBuilder, PushoverResponse, PushoverSound,
};
use std::str::FromStr;

use crate::AppConfig;
use scylla::transport::Compression;
use scylla::{IntoTypedRows, QueryResult, Session, SessionBuilder};
use std::sync::Arc;
#[derive(Debug, Clone)]
pub struct App {
    pub cycles: u64,
    pub token_count: usize,
    pub time: Time,
    pub trades: u64,
    pub logs: Vec<String>,
    pub tokens: Vec<Token>,
    pub cooldown: Duration,
    pub round_id: u64,
    pub term: Term,
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
    pub async fn init(cfg: &AppConfig) -> Self {
        let db_uri = format!("{}:{}", cfg.database.ip, cfg.database.port);
        let session: Session = SessionBuilder::new()
            .known_node(db_uri)
            .compression(Some(Compression::Snappy))
            .build()
            .await
            .unwrap();
        session
            .use_keyspace(&cfg.database.keyspace, false)
            .await
            .unwrap();
        let session = Arc::new(session);

        App {
            round_id: 0,
            cycles: 0,
            token_count: 0,
            trades: 0,
            cooldown: Duration::seconds(5),
            time: Time::default(),
            logs: Vec::new(),
            tokens: Vec::new(),
            deny_list: cfg.strategy.deny_list.clone().unwrap_or_default(),
            exchange: cfg.exchange.clone().unwrap_or_default(),
            term: Term::stdout(),
            pushover: cfg.pushover.clone().unwrap_or_default(),
            db_session: session,
        }
    }
    pub async fn send_notifications(&self, account: &Account, cfg: &AppConfig) -> Result<()> {
        for t in account.portfolio.iter() {
            //send notifications
            if t.change >= cfg.strategy.cashout {
                self.notify(
                    "Cashout Triggered".to_string(),
                    format!(
                        "Token: {} | Change: %{:.2}\nEarnings: {:.2}\nTime Left: {} secs",
                        t.instid, t.report.change, t.report.earnings, t.report.time_left,
                    ),
                )
                .await?;
            }
            if t.change <= -cfg.strategy.stoploss {
                self.notify(
                    "Stoploss Triggered".to_string(),
                    format!(
                        "Token: {} | Change: %{:.2}\nLoss: {:.2}\nTime Left: {} secs",
                        t.instid, t.report.change, t.report.earnings, t.report.time_left,
                    ),
                )
                .await?;
            };
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
    pub async fn save_report(&self, report: &Report) -> Result<QueryResult> {
        let payload = serde_json::to_string_pretty(&report).unwrap();
        let payload = payload.replace("null", "0");
        let query = format!("INSERT INTO okx.reports JSON '{}'", payload);
        Ok(self.db_session.query(&*query, &[]).await?)
    }
    pub async fn save_trade_order(&self, order: &TradeOrder) -> Result<QueryResult> {
        let payload = serde_json::to_string_pretty(&order).unwrap();
        let payload = payload.replace("null", "0");
        let query = format!("INSERT INTO okx.orders JSON '{}'", payload);
        Ok(self.db_session.query(&*query, &[]).await?)
    }

    pub async fn update_reports(
        &mut self,
        mut tokens: Vec<Selected>,
        timeout: i64,
    ) -> Vec<Selected> {
        for t in tokens.iter_mut() {
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
    pub async fn fetch_portfolio_tickers(&mut self, mut tokens: Vec<Selected>) -> Vec<Selected> {
        for t in tokens.iter_mut() {
            if t.status == TokenStatus::Buying {
                t.status = TokenStatus::Trading;
            };
            let query = format!(
                "SELECT last FROM tickers WHERE instid='{}' order by ts desc limit 1;",
                t.instid,
            );
            if let Some(rows) = self.db_session.query(&*query, &[]).await.unwrap().rows {
                for row in rows.into_typed::<(f64,)>() {
                    let (last,): (f64,) = row.unwrap();
                    t.price = last;
                }
            };
            t.change =
                get_percentage_diff(t.balance.current * t.price, t.buy_price * t.balance.start);

            //update timeout
            t.timeout = t.timeout - self.time.elapsed;
        }
        tokens
    }
    pub fn reset_timeouts(
        &mut self,
        mut tokens: Vec<Selected>,
        //timeout: i64,
        //floor_threshold: f32,
    ) -> Vec<Selected> {
        self.tokens.iter().for_each(|s| {
            if let Some(token) = tokens.iter_mut().find(|t| t.instid == s.instid) {
                if token
                    .candlesticks
                    .last()
                    .unwrap_or(&Candlestick::new())
                    .change
                    > 0.0
                {
                    //only reset timeout if change is above floor_threshold
                    //if token.change >= token.config.sell_floor {
                    token.timeout = token.config.timeout
                    // }
                }
            }
        });
        for t in tokens.iter_mut() {
            if t.change == 0.0 && t.timeout.num_seconds() <= 0 {
                self.logs.push(format!(
                    "Resetting timer on token {} due to same buy_price than potential sell_price",
                    t.instid
                ));
            };
        }
        tokens
    }
    pub async fn get_tickers(&mut self) -> &mut Self {
        for t in self.tokens.iter_mut() {
            //Get ticker data
            let query = format!(
                "select last,sodutc0,volccy24h, high24h, low24h from tickers WHERE instid='{}' order by ts desc limit 1;",
                t.instid,
            );
            //self.logs.push(query.clone());
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
    pub async fn save_strategy(&self, strategy: &Strategy) -> &Self {
        let payload = serde_json::to_string_pretty(&strategy).unwrap();
        let query = format!("INSERT INTO okx.strategies JSON '{}'", payload);
        match self.db_session.query(&*query, &[]).await {
            Ok(k) => k,
            Err(e) => panic!(
                "Unable to save stragegy due to {}. Payload: {} ",
                e, payload
            ),
        };
        self
    }
    pub fn set_cooldown(&mut self, num: i64) -> &mut Self {
        self.cooldown = Duration::milliseconds(num * 1000);
        self
    }
    pub fn sum_candles(&mut self) -> &mut Self {
        self.tokens.iter_mut().for_each(|t| {
            //check if vol is enough in the selected timeframe
            t.vol = t.candlesticks.iter().map(|x| x.vol).sum();
            //t.instid = t.instid.replace("-USDT", "");
            //sum changes and range from candlesticks
            t.change = t.candlesticks.iter().map(|x| x.change).sum();
            let changes: Vec<f32> = t
                .candlesticks
                .clone()
                .into_iter()
                .map(|x| x.change)
                .collect();
            t.std_deviation = std_deviation(&changes).unwrap_or(0.0);
            t.range = t.candlesticks.iter().map(|x| x.range).sum();
            t.px = t
                .candlesticks
                .last()
                .expect(&format!(
                    "Failed to read px of candles {:#?}",
                    t.candlesticks
                ))
                .close;
        });
        self
    }
    pub fn calculate_fees(&self, amount: f64, fee: f64) -> f64 {
        amount * (fee / 100.0)
    }
    pub async fn buy_valid(
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
            if t.status == TokenStatus::Buy {
                let fee = self.calculate_fees(account.balance.spendable, self.exchange.maker_fee);
                //Add transaction fee to fee spend
                account.fee_spend += fee;
                //will have this amount to buy tokens after deducting fees
                let spendable_after_fees = account.balance.spendable - fee;

                //remove spent from available balance
                account.balance.available -= account.balance.spendable;

                //token balance
                t.balance.start = spendable_after_fees / t.price;
                t.balance.current = t.balance.start;
                t.balance.available = t.balance.start;
                t.balance.spendable = t.balance.start;

                let order = t
                    .buy(
                        self.exchange.enable_trading,
                        account.authentication.clone(),
                        &strategy.hash,
                    )
                    .await?
                    .clone()
                    .order;
                self.save_trade_order(&order).await?;
                let log_line = format!(
                    "[Order ID: {order_id}] {side} {token} - order type {ord_type} - price: {price} - size: {size} | Response: {response}",
                    order_id = order.ord_id,
                    token = t.instid,
                    side = order.side,
                    ord_type = order.ord_type,
                    price = order.px,
                    size = order.sz,
                    response = if self.exchange.enable_trading {
                        order.response.unwrap().data[0].clone().s_msg
                    } else {
                        String::from("N/A")
                    }
                );
                self.logs.push(log_line);
                //create report
                self.trades += 1;
                self.round_id += 1;

                let mut time_deviation = Vec::new();
                let mut change_deviation = Vec::new();
                //Find old reports and try to get better defaults
                let mut results_count = 0;
                let query = format!(
                "select count(instid) from okx.reports where instid='{}' and strategy='{}' allow filtering;",
                t.instid,
                &strategy.hash
                );
                if let Some(rows) = self
                    .db_session
                    .query(&*query, &[])
                    .await
                    .unwrap_or_else(|_| {
                        self.term.clear_screen().unwrap();
                        panic!("Failed to execute query {}", query)
                    })
                    .rows
                {
                    for row in rows.into_typed::<(i64,)>() {
                        let (c,): (i64,) = row.unwrap();
                        results_count = c;
                    }
                };
                //let skip = true;
                if results_count != 0 {
                    // if !skip {
                    let query = format!(
                "select highest, highest_elapsed from okx.reports where instid='{}' and strategy='{}' allow filtering;",
                t.instid,
                strategy.hash,
                );
                    if let Some(rows) = self
                        .db_session
                        .query(&*query, &[])
                        .await
                        .unwrap_or_else(|_| {
                            self.term.clear_screen().unwrap();
                            panic!("Failed to execute query {}", query)
                        })
                        .rows
                    {
                        for row in rows.into_typed::<(f32, i64)>() {
                            let (highest, highest_elapsed): (f32, i64) = row.unwrap();
                            change_deviation.push(highest);
                            time_deviation.push(highest_elapsed as f32);
                        }
                    };
                    t.config.sell_floor = std_deviation(&change_deviation[..]).unwrap();
                    t.config.timeout = Duration::seconds(strategy.timeout);
                    //    Duration::seconds(std_deviation(&time_deviation[..]).unwrap() as i64);
                    if t.config.timeout.num_seconds() >= 30 || t.config.sell_floor >= 0.1 {
                        self.logs.push(format!(
                            "[DISABLED] Found reports for {}, using new floor {} and timeout {}",
                            t.instid, t.config.sell_floor, t.config.timeout
                        ));
                        t.timeout = t.config.timeout;
                    } else {
                        t.timeout = Duration::seconds(strategy.timeout);
                        t.config.timeout = t.timeout;
                        t.config.sell_floor = strategy.sell_floor.unwrap();
                    };
                } else {
                    /*
                    self.logs.push(format!(
                        "no reports found for token {} -- setting defaults",
                        t.instid
                    ));*/
                    t.timeout = Duration::seconds(strategy.timeout);
                    t.config.timeout = t.timeout;
                    t.config.sell_floor = strategy.sell_floor.unwrap();
                };
                t.report = Report {
                    instid: t.instid.clone(),
                    reason: Reason::Buy.to_string(),
                    round_id: self.round_id,
                    ts: Utc::now().timestamp_millis().to_string(),
                    buy_price: t.price,
                    strategy: strategy.hash.clone(),
                    change: t.change,
                    highest: t.change,
                    lowest: t.change,
                    highest_elapsed: strategy.timeout - t.timeout.num_seconds(),
                    lowest_elapsed: strategy.timeout - t.timeout.num_seconds(),
                    ..Default::default()
                };

                //self.save_report(&t.report).await
            }
        }
        Ok(account)
    }

    pub async fn sell_invalid(
        &mut self,
        mut account: Account,
        strategy: &Strategy,
    ) -> Result<Account> {
        for t in account.portfolio.iter_mut() {
            let found = self.tokens.iter().any(|s| t.instid == s.instid);
            /*
            if t.candlesticks
                .get(t.candlesticks.len() - 3..t.candlesticks.len())
                .is_none()
            {
                self.logs
                    .push("Unable to fetch last candles using get. will use default".to_string())
            };*/
            let change =
                get_percentage_diff(t.balance.current * t.price, t.buy_price * t.balance.start);
            let sell_floor = if t.config.sell_floor == 0.0 {
                strategy.sell_floor.unwrap()
            } else {
                t.config.sell_floor
            };
            let reason = if t.timeout.num_seconds() <= 0 {
                Reason::Timeout
            } else if t.change <= -strategy.stoploss {
                Reason::Stoploss
            } else if t.change >= strategy.cashout {
                Reason::Cashout
            } else if t.change >= sell_floor && !found
            // && (t.change >= t.report.highest-strategy.sell_floor.unwrap())
            {
                Reason::FloorReached
            } else {
                Reason::from_str(&t.report.reason).unwrap()
            };
            //Unused Reasons
            //LowChange,
            //LowVolume

            //If reason != Buy -- Execute sell
            if reason != Reason::Buy {
                t.status = TokenStatus::Sell;

                /*
                self.logs.push(format!(
                    "{} to {:?} || reason: {}",
                    t.instid,
                    t.status,
                    reason.to_string()
                ));*/
            }
            if t.status == TokenStatus::Sell {
                //calculate fees
                let usdt_balance = t.balance.current * t.price;
                let usdt_fee = self.calculate_fees(usdt_balance, self.exchange.taker_fee);
                let token_fees = self.calculate_fees(t.balance.current, self.exchange.taker_fee);
                let token_balance_after_fees = t.balance.current - token_fees;
                account.fee_spend += usdt_fee;
                let balance_after_fees = usdt_balance - usdt_fee;
                let earnings = balance_after_fees - (t.balance.start * t.buy_price);
                account.balance.available += balance_after_fees;
                t.balance.current = token_balance_after_fees;

                let order = t
                    .sell(
                        self.exchange.enable_trading,
                        account.authentication.clone(),
                        &strategy.hash,
                    )
                    .await?
                    .clone()
                    .order;
                self.save_trade_order(&order).await?;
                let log_line = format!(
                    "[Order ID: {order_id}] {side} {token} - order type {ord_type} - price: {price} - size: {size} | Response: {response}",
                    order_id = order.ord_id,
                    token = t.instid,
                    side = order.side,
                    ord_type = order.ord_type,
                    price = order.px,
                    size = order.sz,
                    response = if self.exchange.enable_trading {
                        order.response.unwrap().data[0].clone().s_msg
                    } else {
                        String::from("N/A")
                    }
                );
                self.logs.push(log_line);

                self.trades += 1;
                //build up deny list if stoploss
                if reason == Reason::Stoploss && strategy.avoid_after_stoploss {
                    self.deny_list.push(t.instid.replace("-USDT", ""))
                };
                //Update report
                t.report.time_left = t.timeout.num_seconds();
                //fake resetting balance and returning usdt
                t.balance.available = 0.0;
                t.balance.current = 0.0;
                t.balance.start = 0.0;
                let report = Report {
                    reason: reason.to_string(),
                    ts: Utc::now().timestamp_millis().to_string(),
                    buy_price: t.price,
                    strategy: strategy.hash.to_string(),
                    change,
                    sell_price: t.price,
                    earnings,
                    time_left: t.timeout.num_seconds(),
                    ..t.report.clone()
                };
                t.report = report;
                //push log
                self.logs.push(format!(
                    "Round report for{} {}: Time left: {} - Change: [Highest: %{}, Lowest: %{}, Exit: %{}] - Earnings: {:.2} - Exit Balance: {:.2} - Reason: {}",
                    if self.exchange.enable_trading {
                        format!(" order ID: {}", order.ord_id)
                    } else {
                        String::from(" ")
                    },
                    t.instid,
                    t.timeout.num_seconds(),
                    t.report.highest,
                    t.report.lowest,
                    t.change,
                    earnings,
                    balance_after_fees,
                    reason.to_string(),
                ));

                self.save_report(&t.report).await?;
            }
        }

        Ok(account)
    }

    pub fn filter_invalid(&mut self, strategy: &Strategy, spendable: f64) -> &mut Self {
        //- 24h volume < 800k
        //- < 100*min transactions in selected timeframe
        let mut valid: Vec<Token> = self
            .tokens
            .drain_filter(|t| {
                let denied = self
                    .deny_list
                    .iter()
                    .any(|i| format!("{}-USDT", i) == t.instid);
                let pcc = t
                    .candlesticks
                    .iter()
                    //At least half of the candles should have higher volume than our spendable
                    .filter(|x| x.vol > spendable && x.change > 0.00)
                    .count();
                let blank_candle = Candlestick::new();
                let last_candle = t.candlesticks.last().unwrap_or(&blank_candle);
                !denied
                    //No missing candles in our data
                    && t.candlesticks.len() >= strategy.timeframe as usize
                    //Half of the candles of our timeframe had more volume than our spendable 
                    && pcc >= strategy.timeframe as usize /2
                    && t.change >= strategy.min_change
                    && t.std_deviation >= strategy.min_deviation
                    && last_candle.clone().vol >= spendable
                    //half of the desired change should come from the last candle to be selected
                    && last_candle.change > strategy.min_change / strategy.timeframe as f32
                    && t.vol
                        > strategy
                            .min_vol.unwrap()
            })
            .collect();
        valid.sort_by(|a, b| {
            b.std_deviation
                .partial_cmp(&a.std_deviation)
                .expect("unable to compare change")
        });
        self.tokens = valid;
        self
    }
    pub fn update_cooldown(&mut self, portfolio: &[Selected]) -> &mut Self {
        for t in self.tokens.iter_mut() {
            if portfolio.iter().any(|x| t.instid == x.instid) {
                t.cooldown = self.cooldown
            } else {
                t.cooldown = t.cooldown - self.time.elapsed
            }
        }
        self
    }
    pub fn keep(&mut self, num: usize) -> &mut Self {
        while self.tokens.len() > num {
            self.tokens.pop();
        }
        self
    }
    pub async fn update_portfolio_candles(
        &mut self,
        timeframe: i64,
        mut tokens: Vec<Selected>,
    ) -> Vec<Selected> {
        //sort tokens alphabetically
        //tokens.sort_by_key(|token| token.instid.to_lowercase());
        let xdt = self.time.utc - Duration::minutes(timeframe);
        let dt = xdt.with_second(0).unwrap().with_nanosecond(0).unwrap();
        for t in tokens.iter_mut() {
            let query = format!(
                //"SELECT * FROM candle1m WHERE instid='{}' AND ts >= '{}' LIMIT {}",
                "SELECT * FROM candle1m WHERE instid='{}' LIMIT {}",
                t.instid,
                // dt.to_rfc3339_opts(SecondsFormat::Millis, true),
                timeframe,
                //
            );
            if let Some(rows) = self
                .db_session
                .query(&*query, &[])
                .await
                .unwrap_or_default()
                .rows
            {
                for row in rows.into_typed::<Candlestick>() {
                    let candle: Candlestick = row.unwrap();
                    //self.logs.push(format!("{:?}", candle.ts));
                    if let Some(ci) = t.candlesticks.iter().position(|c| candle.ts == c.ts) {
                        t.candlesticks[ci] = candle;
                    } else {
                        t.candlesticks.push(candle);
                    }
                }
            }
            t.candlesticks
                .retain(|c| c.ts >= Duration::milliseconds(dt.timestamp_millis()));
            let changes: Vec<f32> = t
                .candlesticks
                .clone()
                .into_iter()
                .map(|x| x.change)
                .collect();
            t.std_deviation = std_deviation(&changes).unwrap_or(0.0);
        }

        tokens
    }
    pub async fn fetch_candles(&mut self, timeframe: i64) -> &mut Self {
        let xdt = Utc::now() - Duration::minutes(timeframe);
        let dt = xdt.with_second(0).unwrap().with_nanosecond(0).unwrap();
        let query = format!(
            "SELECT * FROM okx.candle1m WHERE ts >= '{}'",
            dt.to_rfc3339_opts(SecondsFormat::Millis, true)
        );

        if let Some(rows) = self.db_session.query(&*query, &[]).await.unwrap().rows {
            for row in rows.into_typed::<Candlestick>() {
                let candle = row.unwrap();
                if let Some(ti) = self.tokens.iter().position(|t| candle.instid == t.instid) {
                    //candlestick timestamp matches an existing candlestick
                    if let Some(ci) = self.tokens[ti]
                        .candlesticks
                        .iter()
                        .position(|c| candle.ts == c.ts)
                    {
                        self.tokens[ti].candlesticks[ci] = candle;
                    } else {
                        self.tokens[ti].candlesticks.push(candle);
                    }
                } else {
                    let mut t = Token::new(candle.instid.clone(), self.cooldown.num_seconds());
                    t.candlesticks.push(candle);
                    self.tokens.push(t)
                }
            }
        }

        self.tokens.iter_mut().for_each(|t| {
            t.candlesticks
                .sort_by(|a, b| a.ts.partial_cmp(&b.ts).expect("unable to compare change"))
        });

        self.tokens = self
            .tokens
            .drain_filter(|t| t.candlesticks.len() >= 5)
            .collect();

        self
    }
}
