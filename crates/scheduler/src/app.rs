use std::{collections::HashMap, sync::Arc};

use console::Term;
use futures::stream::{self, StreamExt, TryStreamExt};
use pushover_rs::{
    Message, MessageBuilder, PushoverResponse, PushoverSound, send_pushover_request,
};

use crate::prelude::*;

/// An entry the strategy authorized this cycle, carried from the decision loop to
/// the order loop.
///
/// Everything here is captured from the **candidate** token, not the portfolio
/// copy. `Account::add_token` builds a fresh `Token`, and anything it doesn't
/// explicitly copy comes back as zero — which is how the book nearly ended up
/// being read as "missing" at the exact moment we priced the order off it. Same
/// trap, so we don't go near it: the values that justified the trade travel with
/// the trade.
#[derive(Debug, Clone, Copy)]
struct PendingEntry {
    /// Base-currency size, floored to `lot_sz` and checked against `min_sz`.
    size: f64,
    /// The dip and bounce that actually fired.
    signal: EntrySignal,
    /// The token's volatility at entry.
    std_deviation: f64,
    /// The quoted spread at entry, in basis points.
    spread_bps: f64,
}

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
    pub report_tx: Option<tokio::sync::mpsc::Sender<crate::ws::channel::Data>>,
    pub emitted_reports: std::collections::HashSet<u64>,
    pub instruments: std::collections::HashMap<String, InstrumentMeta>,
    /// Per-token cooldown, keyed by instid, surviving `clean_top`.
    ///
    /// It used to live on the `Token` inside `self.tokens` — a vector that
    /// `clean_top` truncates to `top` entries every single cycle. A token that fell
    /// out of the top N was destroyed, and `fetch_tokens` rebuilt it next cycle
    /// with a *fresh full* cooldown. So a token could only become eligible to trade
    /// by holding a top-N rank **continuously** for `cooldown` seconds of wall
    /// time; anything that flickered in and out had its clock reset forever and
    /// could never be bought at all.
    ///
    /// Combined with `rank_score = -dip_sum` — under which a token in a sustained
    /// downtrend has the deepest dip by construction, and therefore a permanent
    /// top-N seat — the two built a machine that could only buy tokens that were
    /// still falling. A 1000-token universe funnelled into whichever name was
    /// bleeding out hardest. PI-USDT: 7 of 9 trades, and more than all of the loss.
    ///
    /// Only tokens actually cooling down are kept; expired entries are dropped, so
    /// this stays small rather than growing to the size of the universe.
    pub cooldowns: HashMap<String, Duration>,
}

#[derive(Debug, Clone)]
pub struct Time {
    pub started: DateTime<Utc>,
    pub utc: DateTime<Utc>,
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

        let instruments = match crate::okx::fetch_spot_instruments().await {
            Ok(list) => {
                let mut map = std::collections::HashMap::with_capacity(list.len());
                let mut skipped = 0usize;
                for raw in list {
                    if raw.state != "live" {
                        continue;
                    }
                    // `from_raw` keeps the decimal precision of the original
                    // strings, which `parse::<f64>()` throws away and
                    // `to_string()` then reinvents as 333.33000000000004.
                    match InstrumentMeta::from_raw(&raw) {
                        Some(meta) => {
                            map.insert(raw.inst_id, meta);
                        },
                        None => skipped += 1,
                    }
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
            report_tx: None,
            emitted_reports: std::collections::HashSet::new(),
            instruments,
            cooldowns: HashMap::new(),
        })
    }

    pub async fn send_notifications(&self, account: &Account) -> Result<()> {
        for t in account.portfolio.iter() {
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
            .set_priority(-1)
            .set_sound(PushoverSound::GAMELAN)
            .set_timestamp(now as u64)
            .build();

        // Was `.unwrap()`. A flaky notification provider should not be able to
        // kill a process that is holding open positions.
        send_pushover_request(message)
            .await
            .map_err(|e| anyhow::anyhow!("pushover request failed: {e:?}"))
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

    /// Counts the clock down on every open position.
    ///
    /// The old implementation did two things that combined badly:
    ///
    /// 1. It only decremented `timeout` while the token was **off** the top-N
    ///    list (`if !self.tokens.iter().any(...)`). A position that stayed in
    ///    the top list — which, having just been ranked into it, most of them
    ///    do — never aged, so `timeout = 240` was never enforced on winners.
    /// 2. It reset `timeout` to full whenever the last candle's change exceeded
    ///    `min_change`. At `min_change = 0.0` that is *any green minute*.
    ///
    /// And the second clause (`change == 0.0 && timeout <= 0`) refilled the
    /// clock of any position with exactly zero change — which includes every
    /// **unfilled** entry, because `buy_price` is 0.0 until the buy fills, so
    /// `get_percentage_diff` returns 0.0. Unfilled entries never expired. That
    /// is what let the re-bid loop in `buy_tokens` run forever.
    ///
    /// Time in a trade always runs. `timeout` now means what it says.
    pub fn update_timeouts(&mut self, mut tokens: Vec<Token>) -> Vec<Token> {
        for t in tokens.iter_mut() {
            if matches!(t.status, token::Status::Trading | token::Status::Selling) {
                t.timeout = t.timeout - self.time.elapsed;
            }
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

        let get_candles_query = self
            .db_session
            .prepare(
                "SELECT instid, ts, change, close, high, low, open, range, volume \
                 FROM candle1m WHERE instid=? AND ts <= ? LIMIT ?",
            )
            .await?;

        let get_tickers_query = self
            .db_session
            .prepare(
                "SELECT last, lastsz, ts FROM tickers WHERE instid=? AND ts >= ? order by ts asc",
            )
            .await?;

        // `okx.tickers` has carried askpx/asksz/bidpx/bidsz since the producer
        // was written (see `exchange_observer::models::TickerRow`); the scheduler
        // only ever selected `last`, so every order in this system was priced off
        // the last *trade* and the book was invisible.
        //
        // The sizes matter as much as the prices. Every entry is priced at exactly
        // the ask, so an IOC never reaches past level 1 — which means `asksz`
        // alone decides whether the order fills in full. It is also the number
        // that tells you whether the `books` channel is worth subscribing to at
        // all: if `spendable` sits comfortably inside `askpx * asksz`, depth data
        // would tell you nothing you don't already have.
        let get_price_query = self
            .db_session
            .prepare(
                "SELECT last, askpx, asksz, bidpx, bidsz \
                 FROM tickers WHERE instid=? LIMIT 1",
            )
            .await?;

        stream::iter(tokens.into_iter().map(|mut token| {
            let get_candle_stmt = get_candles_query.clone();
            let get_ticker_stmt = get_tickers_query.clone();
            let get_price_stmt = get_price_query.clone();
            async move {
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

                let price_result = self
                    .db_session
                    .execute_unpaged(&get_price_stmt, (&token.instid,))
                    .await?;
                let price_rows = price_result.into_rows_result()?;
                let tickers: Vec<(f64, f64, f64, f64, f64)> = price_rows
                    .rows::<(f64, f64, f64, f64, f64)>()?
                    .filter_map(Result::ok)
                    .collect();
                if let Some(&(last, ask, ask_sz, bid, bid_sz)) = tickers.last() {
                    token.price = last;
                    token.ask = ask;
                    token.ask_sz = ask_sz;
                    token.bid = bid;
                    token.bid_sz = bid_sz;
                }

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
        // instid -> everything the order loop needs, captured at the moment the
        // decision was made. Sizing and diagnostics live next to the decision so
        // they cannot drift apart from it.
        let mut pending: HashMap<String, PendingEntry> = HashMap::new();

        for token in self.tokens.iter_mut() {
            if token.cooldown > Duration::milliseconds(0)
                || account.portfolio.iter().any(|p| token.instid == p.instid)
            {
                continue;
            }

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

            let view = token.entry_view(denied);
            match strategy.should_enter(&ctx, &view) {
                EnterDecision::Enter { size_quote } => {
                    let book = token.top_of_book();

                    // No book, no entry.
                    //
                    // `top_of_book()` falls back to a synthetic spread around
                    // `last` when the feed gave us no bid/ask. That fallback is
                    // there so *exits* always have a price — we must be able to get
                    // out. It has no business authorizing an entry: OKX sends
                    // `askPx: ""` whenever a side of the book is empty, and an
                    // instrument with nothing resting on the offer is not one you
                    // can buy. Inventing an ask for it would mean pricing a real
                    // order off a number we made up.
                    //
                    // Note this also closes a hole in the spread guard below, which
                    // returns `None` (and therefore skips itself) when there is no
                    // book to measure.
                    if book.synthetic {
                        log::debug!(
                            "[{}] entry skipped: no book (askpx/bidpx empty or missing)",
                            token.instid
                        );
                        continue;
                    }

                    // Spread guard (`strategy.max_spread_bps`; `None` disables).
                    //
                    // The strategy can't see the book — `TokenView` carries no
                    // bid/ask — so this lives here. It is the cheapest filter
                    // available and nothing in this system has ever applied it:
                    // a round trip costs `2 x taker_fee` (20bps at the default)
                    // *plus* the spread, twice, against a 1% take-profit.
                    if let (Some(max_spread), Some(spread)) =
                        (config.max_spread_bps, token.spread_bps())
                    {
                        if spread > f64::from(max_spread) {
                            log::debug!(
                                "[{}] entry skipped: spread {:.1}bps > max {:.1}bps",
                                token.instid,
                                spread,
                                max_spread
                            );
                            continue;
                        }
                    }
                    let meta = self
                        .instruments
                        .get(&token.instid)
                        .copied()
                        .unwrap_or(InstrumentMeta::UNKNOWN);
                    // Size against the price we will actually bid, not the last
                    // trade, and honor `min_sz` — which we fetch from OKX and,
                    // until now, never used. An order below `min_sz` is rejected
                    // by the exchange and comes back looking exactly like a
                    // missed fill, which is how it stayed invisible.
                    let limit_px = token.entry_limit(meta);
                    let Some(size) = meta.order_size(size_quote, limit_px) else {
                        log::warn!(
                            "[{}] entry skipped: {:.4} quote at {} is below min_sz {}",
                            token.instid,
                            size_quote,
                            limit_px,
                            meta.min_sz
                        );
                        continue;
                    };

                    // Depth guard. We bid exactly the ask, so an IOC never reaches
                    // past level 1: the order fills in full iff it fits inside
                    // `ask_sz`, and otherwise a real IOC takes what's there and
                    // cancels the rest — leaving a position smaller than we sized,
                    // which the rest of this state machine has no honest way to
                    // carry.
                    //
                    // So: don't take it. If this fires often, the instrument is too
                    // thin for `spendable` and no amount of order-book data will
                    // fix that; raise `min_vol` or cut the size.
                    if !book.ask_covers(size) {
                        log::debug!(
                            "[{}] entry skipped: size {:.8} exceeds best ask size {:.8} \
                             ({:.2} USDT resting at {})",
                            token.instid,
                            size,
                            book.ask_sz,
                            book.ask_notional(),
                            book.ask
                        );
                        continue;
                    }

                    // The conditions that justified this trade, recorded so the entry
                    // filters can eventually be tuned against outcomes rather than
                    // arguments. `min_dip` says what we *required*; `signal.dip`
                    // says what we actually *got*, and only the second one can tell
                    // us whether the requirement is set anywhere near right.
                    let entry = PendingEntry {
                        size,
                        signal: strategy.entry_signal(&ctx, &view),
                        std_deviation: f64::from(token.std_deviation),
                        spread_bps: token.spread_bps().unwrap_or(0.0),
                    };

                    let before = account.portfolio.len();
                    account.add_token(token, config);
                    // `add_token` silently no-ops when the portfolio is full or the
                    // balance is short; only record the entry if it took the position.
                    if account.portfolio.len() > before {
                        pending.insert(token.instid.clone(), entry);
                    }
                },
                EnterDecision::Skip(reason) => {
                    log::debug!(
                        "[{}] entry skipped by {}: {}",
                        token.instid,
                        strategy.name(),
                        reason
                    );
                },
            }
        }

        for t in account.portfolio.iter_mut() {
            // Only *fresh* entries get an order.
            //
            // This loop used to re-bid any position whose buy came back
            // Cancelled — `buy_orders = any(side == Buy && state != Cancelled)`
            // — with no fresh strategy decision (the old comment said so out
            // loud). Combined with an IOC priced *under* the market, that turned
            // a one-shot entry into "chase this token until it fills", and the
            // price it eventually filled at was, by construction, a price that
            // had fallen to meet us. Every retry was a worse entry than the one
            // the strategy actually asked for.
            //
            // A cancelled entry is now abandoned. `Account::clean_portfolio`
            // drops the token; it becomes a candidate again on its own merits
            // once its cooldown expires and it re-passes `should_enter`.
            if t.orders.is_some() {
                continue;
            }
            let Some(entry) = pending.remove(&t.instid) else {
                continue;
            };

            let meta = self
                .instruments
                .get(&t.instid)
                .copied()
                .unwrap_or(InstrumentMeta::UNKNOWN);

            t.balance.start = entry.size;
            t.configure(config);

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
                    .ok_or_else(|| anyhow::anyhow!("buy() produced no order"))?;

                order.save(&self.db_session).await?;
                let log_line = self.build_order_log(order);
                self.logs.push(log_line);
                self.round_id += 1;
            }

            t.report = Report::new(self.round_id, &config.hash, t, self.clock.now_utc());
            t.report.dip = entry.signal.dip;
            t.report.bounce = entry.signal.bounce;
            t.report.std_deviation = entry.std_deviation;
            t.report.spread_bps = entry.spread_bps;
        }

        // Safety net: a position with no order occupies a portfolio slot and can
        // never do anything. Should be unreachable now, but the old code's worst
        // bug was a slot held by a token in limbo.
        account.portfolio.retain(|t| t.orders.is_some());

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
                match &order.response {
                    Some(r) => r
                        .data
                        .first()
                        .map(|d| d.s_msg.clone())
                        .unwrap_or_else(|| "N/A".to_string()),
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
                .as_deref()
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
                .as_deref()
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
                        .ok_or_else(|| anyhow::anyhow!("sell() produced no order"))?;

                    order.save(&self.db_session).await?;
                    let log_line = self.build_order_log(order);
                    self.logs.push(log_line);
                }

                let denied = self
                    .deny_list
                    .iter()
                    .any(|i| format!("{}-USDT", i) == t.instid);

                if t.exit_reason == Some(ExitReason::Stoploss)
                    && config.avoid_after_stoploss
                    && !denied
                {
                    self.deny_list.push(t.instid.replace("-USDT", ""))
                };

                // OKX takes the taker fee on a spot BUY in the base currency: we pay
                // `size * buy_price` USDT and receive `size * (1 - f)` tokens.
                // `calculate_balance` already sets `balance.current` to the
                // fee-reduced token amount — so the entry fee is *inside* the
                // balance we are about to sell.
                //
                // The old line was `let total_cost = cost + entry_fee;`, which
                // charged it a second time. Every loss was overstated by exactly
                // one entry fee (0.1% of notional, $0.05 at $50), and every report
                // ever generated inherited it. The comment on that block said it
                // was fixing an understated loss; it overshot.
                //
                //   net = size * (1-f)^2 * sell  -  size * buy
                //
                // `fees` below is still the true round-trip cost in USDT and is
                // reported as such — it just isn't subtracted twice.
                let cost = t.balance.start * t.buy_price;
                let entry_fee = calculate_fees(cost, self.exchange.taker_fee);
                let proceeds = t.balance.current * t.price;
                let exit_fee = calculate_fees(proceeds, self.exchange.taker_fee);
                let net_proceeds = proceeds - exit_fee;
                let earnings = net_proceeds - cost;
                let fees = entry_fee + exit_fee;

                t.report.earnings = earnings;
                t.report.fees = fees;
                t.report.change = if t.buy_price > 0.0 {
                    (((t.price - t.buy_price) / t.buy_price) * 100.0) as f32
                } else {
                    0.0
                };
                t.report.buy_price = t.buy_price;
                t.report.sell_price = t.price;

                t.report.save(&self.db_session).await?;

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

    /// Keeps only tokens the strategy would enter, then ranks them.
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
        self.tokens.sort_by(|a, b| {
            let sa = strategy.rank_score(&ctx, &a.entry_view(false));
            let sb = strategy.rank_score(&ctx, &b.entry_view(false));
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        self
    }

    /// Ages every token's cooldown — including the ones no longer in the top-N.
    ///
    /// Cooldown means one thing now: **how long after exiting a position before we
    /// will trade that name again.** It is pinned while the position is open and
    /// drains once it closes. It is no longer a throttle on *evaluating* a
    /// candidate — `should_enter` is pure and cheap, and a token that starts
    /// passing the filters should be buyable immediately rather than serving a
    /// sentence for having recently been ranked ninth.
    ///
    /// See `App::cooldowns` for what this used to do instead, and why it meant the
    /// bot could only ever buy tokens in sustained downtrends.
    pub fn update_cooldowns(&mut self, portfolio: &[Token]) -> &mut Self {
        let reset = self.cooldown;
        let elapsed = self.time.elapsed;

        for (instid, cooldown) in self.cooldowns.iter_mut() {
            *cooldown = if portfolio.iter().any(|p| p.instid == *instid) {
                reset
            } else {
                *cooldown - elapsed
            };
        }

        // Newly opened positions start their clock. Held tokens are re-pinned above
        // on every subsequent cycle, so this only has to catch the first one.
        for p in portfolio.iter() {
            self.cooldowns.entry(p.instid.clone()).or_insert(reset);
        }

        // Expired and not held: forget it. Keeps the map the size of the tokens
        // actually cooling down rather than the whole universe.
        self.cooldowns.retain(|_, cd| cd.num_milliseconds() > 0);

        for t in self.tokens.iter_mut() {
            t.cooldown = self
                .cooldowns
                .get(&t.instid)
                .copied()
                .unwrap_or_else(Duration::zero);
        }
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
            // A token we are re-discovering after `clean_top` dropped it keeps
            // whatever cooldown it had left. Absent from the map means "not cooling
            // down" — zero, eligible now. It still has to clear every filter; the
            // cooldown's job is to stop us re-trading a name we just exited, not to
            // make a token serve 5 seconds for the crime of being newly seen.
            let remembered = self
                .cooldowns
                .get(&candle.instid)
                .copied()
                .unwrap_or_else(Duration::zero);

            if let Some(token) = self.tokens.iter_mut().find(|t| candle.instid == t.instid) {
                token.add_or_update_candle(candle);
            } else {
                let mut new_token = Token::new(&candle.instid);
                new_token.cooldown = remembered;
                new_token.add_or_update_candle(candle);
                self.tokens.push(new_token);
            }
        }
        Ok(self)
    }
}
