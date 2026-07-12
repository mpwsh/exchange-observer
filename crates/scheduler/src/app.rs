use std::{collections::HashMap, sync::Arc};

use console::Term;
use futures::stream::{self, StreamExt, TryStreamExt};
use pushover_rs::{
    Message, MessageBuilder, PushoverResponse, PushoverSound, send_pushover_request,
};

use crate::prelude::*;

#[derive(Debug)]
pub struct App {
    /// The single source of time for the scheduler (hexagonal Clock port).
    pub clock: Arc<dyn Clock>,
    pub cycles: u64,
    pub time: Time,
    pub logs: Vec<String>,
    pub tokens: Vec<Token>,
    pub cooldown: Duration,
    pub round_id: u64,
    pub term: Term,
    pub pushover: Pushover,
    pub exchange: Exchange,
    pub deny_list: Vec<String>,
    pub db_session: Arc<Session>,
    /// If set, closed reports are fanned out through this channel to the
    /// WS transmit task. `None` when the console/server is disabled.
    pub report_tx: Option<tokio::sync::mpsc::Sender<crate::ws::channel::Data>>,
    /// Round-ids we've already emitted report events for this session.
    /// The `sell_tokens` guard re-fires every cycle while a token is in
    /// `Selling` status with an unfilled sell order; Scylla dedups on the
    /// primary key so the DB stays clean, but the WS stream would show N
    /// duplicates. Simple set keeps emit idempotent.
    pub emitted_reports: std::collections::HashSet<u64>,
    /// Precision + minimum-size metadata for every spot instrument,
    /// fetched from OKX at startup. Order construction rounds size to
    /// `lot_sz` and price to `tick_sz` per instrument. Missing entries
    /// fall back to `InstrumentMeta::UNKNOWN`, which is the pre-rounding
    /// behavior — safe when OKX is unreachable at boot.
    pub instruments: std::collections::HashMap<String, InstrumentMeta>,
}

#[derive(Debug, Clone)]
pub struct Time {
    pub started: DateTime<Utc>,
    pub utc: DateTime<Utc>,
    /// Monotonic reading taken at the start of the current cycle; spans are
    /// measured as `clock.monotonic() - time.mono` (replaces `Instant`).
    pub mono: time::Duration,
    pub elapsed: Duration,
    pub uptime: Duration,
}
impl Time {
    pub fn new(clock: &dyn Clock) -> Self {
        Self {
            started: clock.now_utc(),
            utc: clock.now_utc(),
            mono: clock.monotonic(),
            elapsed: Duration::milliseconds(0),
            uptime: Duration::seconds(0),
        }
    }
}
impl App {
    pub async fn init(cfg: &AppConfig, clock: Arc<dyn Clock>) -> Result<Self> {
        let db_uri = format!("{}:{}", cfg.database.ip, cfg.database.port);
        let session: Session = SessionBuilder::new()
            .known_node(db_uri)
            .compression(Some(Compression::Snappy))
            .build()
            .await?;
        session.use_keyspace(&cfg.database.keyspace, false).await?;
        let session = Arc::new(session);

        // Fetch instrument precision metadata once. On failure we still
        // boot with an empty map — order construction falls back to
        // InstrumentMeta::UNKNOWN, which preserves pre-rounding behavior.
        // Better to run without rounding than not run at all.
        let instruments = match crate::okx::fetch_spot_instruments().await {
            Ok(list) => {
                let mut map = std::collections::HashMap::with_capacity(list.len());
                let mut skipped = 0usize;
                for raw in list {
                    // Only trade live instruments; parked ones would just
                    // clutter the map. Also skip if any critical field
                    // failed to parse — safer than 0.0 defaults that
                    // could imply "no lot size" when the real meaning is
                    // "malformed response".
                    if raw.state != "live" {
                        continue;
                    }
                    let (Ok(lot_sz), Ok(tick_sz), Ok(min_sz)) = (
                        raw.lot_sz.parse::<f64>(),
                        raw.tick_sz.parse::<f64>(),
                        raw.min_sz.parse::<f64>(),
                    ) else {
                        skipped += 1;
                        continue;
                    };
                    map.insert(raw.inst_id, InstrumentMeta { lot_sz, tick_sz, min_sz });
                }
                log::info!(
                    "Loaded precision metadata for {} instruments ({} skipped)",
                    map.len(),
                    skipped
                );
                map
            },
            Err(e) => {
                log::warn!(
                    "Could not fetch OKX instruments ({e}); orders will use raw f64 precision"
                );
                std::collections::HashMap::new()
            },
        };

        Ok(App {
            round_id: 0,
            cycles: 0,
            cooldown: Duration::seconds(5),
            time: Time::new(clock.as_ref()),
            clock,
            logs: Vec::new(),
            tokens: Vec::new(),
            deny_list: cfg.strategy.deny_list.clone().unwrap_or_default(),
            exchange: cfg.exchange.clone().unwrap_or_default(),
            term: Term::stdout(),
            pushover: cfg.pushover.clone().unwrap_or_default(),
            db_session: session,
            // Wired later in main() if the WS server is enabled.
            report_tx: None,
            emitted_reports: std::collections::HashSet::new(),
            instruments,
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
                    },
                    ExitReason::Stoploss => {
                        self.notify(
                            "Stoploss Triggered".to_string(),
                            format!(
                                "Token: {} | Change: %{:.2}\nLoss: {:.2}\nTime Left: {} secs",
                                t.instid, t.report.change, t.report.earnings, t.report.time_left,
                            ),
                        )
                        .await?;
                    },
                    _ => (),
                }
            }
        }
        Ok(())
    }
    pub async fn notify(&self, title: String, msg: String) -> Result<PushoverResponse> {
        let now = self.time.utc.timestamp();
        let message: Message = MessageBuilder::new(&self.pushover.key, &self.pushover.token, &msg)
            .set_title(&title)
            //.add_url("https://pushover.net/", Some("Pushover"))
            .set_priority(-1)
            .set_sound(PushoverSound::GAMELAN)
            .set_timestamp(now as u64)
            .build();

        Ok(send_pushover_request(message).await.unwrap())
    }

    pub async fn get_tickers(&mut self) -> Result<&mut Self> {
        for t in self.tokens.iter_mut() {
            let query = format!(
                "select last,sodutc0,volccy24h, high24h, low24h from tickers WHERE instid='{}' limit 1;",
                t.instid,
            );

            let result = self.db_session.query_unpaged(&*query, &[]).await?;
            let rows_result = result.into_rows_result()?;
            for row in rows_result.rows::<(f64, f64, f64, f64, f64)>()? {
                let (last, open24h, volccy24h, high24h, low24h) = row?;
                t.vol24h = volccy24h;
                t.change24h = get_percentage_diff(last, open24h) as f32;
                t.range24h = get_percentage_diff(high24h, low24h) as f32;
            }
        }
        Ok(self)
    }
    pub fn update_timeouts(&mut self, mut tokens: Vec<Token>, config: &StrategyConfig) -> Vec<Token> {
        let now = self.clock.now_utc();
        self.tokens.iter().for_each(|s| {
            if let Some(token) = tokens.iter_mut().find(|t| t.instid == s.instid) {
                if token
                    .candlesticks
                    .last()
                    .unwrap_or(&Candlestick::new(token.price, now))
                    .change
                    > config.min_change as f64
                {
                    token.timeout = token.config.timeout
                }
            }
        });

        for t in tokens.iter_mut() {
            if !self.tokens.iter_mut().any(|top| top.instid == t.instid) {
                t.timeout -= self.time.elapsed;
            };

            if t.change == 0.0 && t.timeout.num_seconds() <= 0 {
                t.timeout = Duration::seconds(config.timeout)
            };
        }
        tokens
    }

    pub async fn save_strategy(&self, config: &StrategyConfig) -> Result<()> {
        let payload = serde_json::to_string_pretty(&config)?;
        let query = format!("INSERT INTO okx.strategies JSON '{}'", payload);
        self.db_session.query_unpaged(&*query, &[]).await?;
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
            .prepare(
                "SELECT instid, ts, change, close, high, low, open, range, volume \
                 FROM candle1m WHERE instid=? AND ts <= ? LIMIT ?",
            )
            .await?;

        //Last min tickers
        let get_tickers_query = self
            .db_session
            .prepare(
                "SELECT last, lastsz, ts FROM tickers WHERE instid=? AND ts >= ? order by ts asc",
            )
            .await?;

        //Current price
        let get_price_query = self
            .db_session
            .prepare("SELECT last FROM tickers WHERE instid=? LIMIT 1")
            .await?;

        stream::iter(tokens.into_iter().map(|mut token| {
            let get_candle_stmt = get_candles_query.clone();
            let get_ticker_stmt = get_tickers_query.clone();
            let get_price_stmt = get_price_query.clone();
            async move {
                //Get all candles in the selected timeframe
                let result = self
                    .db_session
                    .execute_unpaged(&get_candle_stmt, (&token.instid, dt, timeframe as i32))
                    .await?;
                let rows_result = result.into_rows_result()?;
                for row in rows_result.rows::<Candlestick>()? {
                    let candle =
                        row.unwrap_or_else(|_| Candlestick::new(token.price, self.clock.now_utc()));
                    token.add_or_update_candle(candle)
                }

                let dt = self.time.utc;
                let last_min = match token.candlesticks.last() {
                    Some(candlestick) if candlestick.ts.minute() == dt.minute() => {
                        dt - Duration::seconds(1)
                    },
                    _ => dt,
                };

                //Token price
                let price_result = self
                    .db_session
                    .execute_unpaged(&get_price_stmt, (&token.instid,))
                    .await?;
                let price_rows = price_result.into_rows_result()?;
                let tickers: Vec<(f64,)> = price_rows
                    .rows::<(f64,)>()?
                    .filter_map(Result::ok)
                    .collect();
                token.price = tickers.last().map(|t| t.0).unwrap_or(token.price);

                //Last candle built from last minute of tickers
                let ticker_result = self
                    .db_session
                    .execute_unpaged(&get_ticker_stmt, (&token.instid, last_min))
                    .await?;
                let ticker_rows = ticker_result.into_rows_result()?;
                let tickers: Vec<(f64, f64, DateTime<Utc>)> = ticker_rows
                    .rows::<(f64, f64, DateTime<Utc>)>()?
                    .filter_map(Result::ok)
                    .collect();

                token.candlesticks.sort_by(|a, b| {
                    a.ts.partial_cmp(&b.ts)
                        .expect("unable to compare timestamps")
                });
                // If tickers came back with data for the in-progress minute,
                // fold it in. If tickers are empty, DON'T synthesize a blank
                // candle — appending vol=0 change=0 poisons the strategy's
                // last-candle checks and looks like the token had no activity
                // when what actually happened is nobody traded yet this minute.
                // Skipping lets `should_enter` evaluate against the last real
                // completed candle from Scylla, which is the honest reading.
                if let Some(mut last_candle) =
                    Candlestick::from_tickers(&token.instid, &tickers, self.clock.now_utc())
                {
                    if last_candle.change == 0.0 {
                        last_candle.open = token.price;
                        last_candle.high = token.price;
                        last_candle.low = token.price;
                        last_candle.close = token.price;
                    }
                    token.add_or_update_candle(last_candle);
                }

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
        strategy: &dyn Strategy,
        config: &StrategyConfig,
    ) -> Result<Account> {
        //Add to portfolio first
        let mut entry_sizes: HashMap<String, f64> = HashMap::new();
        for token in self.tokens.iter_mut() {
            if token.cooldown <= Duration::milliseconds(0)
                && !account.portfolio.iter().any(|p| token.instid == p.instid)
            {
                let portfolio_view = account.portfolio_view();
                let ctx = Context {
                    clock: self.clock.as_ref(),
                    config,
                    portfolio: &portfolio_view,
                };
                let denied = self
                    .deny_list
                    .iter()
                    .any(|i| format!("{}-USDT", i) == token.instid);

                // Tokens here already passed `should_enter` in
                // `filter_invalid` and none of its inputs change in between,
                // so this re-evaluation always agrees; it exists to source
                // the position size from the strategy rather than hardcode it.
                match strategy.should_enter(&ctx, &token.entry_view(denied)) {
                    EnterDecision::Enter { size_quote } => {
                        account.add_token(token, config);
                        entry_sizes.insert(token.instid.clone(), size_quote);
                    },
                    EnterDecision::Skip(reason) => {
                        log::debug!("[{}] entry skipped by {}: {}", token.instid, strategy.name(), reason);
                    },
                }
                token.cooldown = self.cooldown;
            }
        }
        //trigger order creation
        for t in account.portfolio.iter_mut() {
            let buy_orders = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|o| o.side == Side::Buy && o.state != OrderState::Cancelled);

            if !buy_orders {
                // Fresh entries are sized by the strategy's decision;
                // re-buys after a cancelled order (no fresh decision this
                // cycle) keep the original sizing rule, which is the same
                // value under ThresholdStrategy.
                let size_quote = entry_sizes
                    .remove(&t.instid)
                    .unwrap_or(account.balance.spendable);

                // Round size to the instrument's lot_sz so the exchange
                // will accept it. Cache miss → UNKNOWN (step 0.0), which
                // `floor_to_step` passes through unchanged — pre-rounding
                // behavior for instruments we don't have metadata for.
                let meta = self
                    .instruments
                    .get(&t.instid)
                    .copied()
                    .unwrap_or(InstrumentMeta::UNKNOWN);
                t.balance.start = floor_to_step(size_quote / t.price, meta.lot_sz);
                t.configure_from_report(config, &self.db_session).await;

                {
                    let order = t
                        .buy(
                            self.exchange.enable_trading,
                            account.authentication.clone(),
                            config,
                            self.clock.now_utc(),
                            meta,
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

                t.report = Report::new(self.round_id, &config.hash, t, self.clock.now_utc());
            }
        }
        Ok(account)
    }

    pub fn build_order_log(&self, order: &Order) -> String {
        format!(
            "[{timestamp}] {side} Order {state} for [{token}] > Type {ord_type} - price: {price} - size: {size} | Response: {response} | id: {order_id}",
            timestamp = self.time.utc.format("%Y-%m-%d %H:%M:%S"),
            state = order.state.to_string(),
            token = order.inst_id,
            side = order.side.to_string(),
            ord_type = order.ord_type,
            price = order.px,
            size = order.sz,
            response = if self.exchange.enable_trading {
                match order.clone().response {
                    Some(r) => r.data[0].clone().s_msg,
                    None => format!("{:?}", order.response),
                }
            } else {
                "N/A".to_string()
            },
            order_id = match order.state {
                OrderState::Created => "Creating",
                _ => &order.id,
            }
        )
    }
    pub async fn sell_tokens(
        &mut self,
        mut account: Account,
        config: &StrategyConfig,
    ) -> Result<Account> {
        for t in account.portfolio.iter_mut() {
            let filled_orders_amount: f64 = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .filter_map(|o| {
                    if o.side == Side::Sell && o.state == OrderState::Filled {
                        o.sz.parse::<f64>().ok()
                    } else {
                        None
                    }
                })
                .sum();

            let live_orders = t
                .orders
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|o| o.side == Side::Sell && o.state == OrderState::Live);

            let balance_threshold = t.balance.start * 0.99;
            if t.status == token::Status::Selling
                && filled_orders_amount < balance_threshold
                && !live_orders
                && t.exit_reason.is_some()
            {
                {
                    let meta = self
                        .instruments
                        .get(&t.instid)
                        .copied()
                        .unwrap_or(InstrumentMeta::UNKNOWN);
                    let order = t
                        .sell(
                            self.exchange.enable_trading,
                            account.authentication.clone(),
                            config,
                            self.clock.now_utc(),
                            meta,
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

                //deny tokens to be bought again
                if t.exit_reason == Some(ExitReason::Stoploss)
                    && config.avoid_after_stoploss
                    && !denied
                {
                    self.deny_list.push(t.instid.replace("-USDT", ""))
                };

                // Create token report
                let usdt_balance = t.balance.current * t.price;
                let usdt_fee = calculate_fees(usdt_balance, self.exchange.taker_fee);
                let usdt_balance_after_fees = usdt_balance - usdt_fee;
                let earnings = (t.balance.start * t.buy_price) - usdt_balance_after_fees;
                let earnings = if earnings < 0.0 {
                    usdt_balance_after_fees - (t.balance.start * t.buy_price)
                } else {
                    -earnings
                };
                t.report.earnings = earnings;
                t.report.change = t.change;

                t.report.save(&self.db_session).await?;

                // Fan out to the console over WS, if a listener exists.
                // `try_send` is deliberate — a slow/dead consumer must never
                // stall the trade loop. Dropping a report here just means
                // the console misses one; Scylla has the ground truth.
                //
                // The enclosing guard re-fires while the token stays in
                // `Selling` status with an unfilled sell order (which can
                // last many cycles in the IOC-retry simulator path), so we
                // dedup by round_id — one emit per closed position, even
                // though the DB write is idempotent by primary key.
                if let Some(tx) = &self.report_tx {
                    if self.emitted_reports.insert(t.report.round_id) {
                        let event = crate::ws::channel::ReportEvent {
                            report: t.report.clone(),
                            instid: t.instid.clone(),
                            buy_price: t.buy_price,
                            sell_price: t.price,
                            ts: self.clock.now_utc(),
                        };
                        let _ = tx.try_send(crate::ws::channel::Data::Report(event));
                    }
                }
            }
        }
        Ok(account)
    }

    /// Keeps only tokens the strategy would enter. The threshold checks that
    /// used to live in `Token::is_valid` now run behind `should_enter`.
    pub fn filter_invalid(
        &mut self,
        strategy: &dyn Strategy,
        config: &StrategyConfig,
        portfolio: &PortfolioView,
    ) -> &mut Self {
        let ctx = Context {
            clock: self.clock.as_ref(),
            config,
            portfolio,
        };
        let deny_list = &self.deny_list;
        self.tokens.retain(|t| {
            let denied = deny_list.iter().any(|i| format!("{}-USDT", i) == t.instid);
            match strategy.should_enter(&ctx, &t.entry_view(denied)) {
                EnterDecision::Enter { .. } => true,
                EnterDecision::Skip(reason) => {
                    log::debug!("[{}] filtered by {}: {}", t.instid, strategy.name(), reason);
                    false
                },
            }
        });
        // Descending sort by `change` (highest momentum first). Was
        // comparing `b.std_deviation` to `a.change` — two different fields,
        // which isn't a total order and panics in `smallsort` since Rust
        // 1.81 validates comparators. NaN falls to `Equal` so a bad tick
        // can't take the loop down.
        self.tokens.sort_by(|a, b| {
            b.change
                .partial_cmp(&a.change)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self
    }

    pub fn update_cooldowns(&mut self, portfolio: &[Token]) -> &mut Self {
        self.tokens.iter_mut().for_each(|t| {
            t.cooldown = if portfolio.iter().any(|x| x.instid == t.instid) {
                self.cooldown
            } else {
                t.cooldown - self.time.elapsed
            }
        });
        self
    }

    pub fn clean_top(&mut self, num: usize) -> &mut Self {
        while self.tokens.len() > num {
            self.tokens.pop();
        }
        self
    }

    pub async fn fetch_tokens(&mut self, timeframe: i64) -> Result<&mut Self> {
        let xdt = self.time.utc - Duration::minutes(timeframe);
        let dt = xdt.with_second(0).unwrap().with_nanosecond(0).unwrap();
        let query = format!(
            "SELECT instid, ts, change, close, high, low, open, range, volume \
             FROM okx.candle1m WHERE ts >= '{}' ALLOW FILTERING",
            dt.timestamp_millis()
        );

        let result = self.db_session.query_unpaged(&*query, &[]).await?;
        let rows_result = result.into_rows_result()?;
        for row in rows_result.rows::<Candlestick>()? {
            let candle = row?;
            if let Some(token) = self.tokens.iter_mut().find(|t| candle.instid == t.instid) {
                token.add_or_update_candle(candle);
            } else {
                let mut new_token =
                    Token::new(&candle.instid).set_cooldown(self.cooldown.num_seconds());
                new_token.add_or_update_candle(candle);
                self.tokens.push(new_token);
            }
        }
        Ok(self)
    }
}
