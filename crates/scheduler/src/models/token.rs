use crate::prelude::*;

/// Half-spread assumed **only when the tickers feed gave us no book**, in basis
/// points.
///
/// `okx.tickers` carries `askpx`/`bidpx` (see `exchange_observer::models::Ticker`
/// and `TickerRow`) — the real touch is in the database and the scheduler simply
/// never selected it. Everything below prices against the real bid/ask; this
/// constant only covers the case where a row comes back without one.
///
/// It is deliberately *not* zero: a fallback of "no spread" is what the old
/// simulator effectively assumed, and it is the assumption that made paper
/// results look nothing like production.
const FALLBACK_HALF_SPREAD_BPS: f64 = 10.0;

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
    /// Best ask from the tickers feed. `0.0` when unknown.
    ///
    /// New: `update_candles` used to `SELECT last` and nothing else, so every
    /// order in this system was priced off the last *trade* and the book was
    /// invisible — on a strategy whose take-profit is 1%, while trading
    /// instruments thin enough that the spread can be a meaningful part of it.
    #[serde(default)]
    pub ask: f64,
    /// Size resting at the best ask, in base currency. `0.0` = unknown.
    ///
    /// This is the number that decides whether we need the `books` channel at
    /// all. An IOC priced at the ask fills in full iff `size <= ask_sz`; below
    /// that threshold, level 1 is not an approximation of the book, it *is* the
    /// book, and depth would tell us nothing. Above it, we're walking into level
    /// 2 and the fill is worse than we think.
    ///
    /// The producer has been writing `asksz` to `okx.tickers` all along.
    #[serde(default)]
    pub ask_sz: f64,
    /// Best bid from the tickers feed. `0.0` when unknown.
    #[serde(default)]
    pub bid: f64,
    /// Size resting at the best bid, in base currency. `0.0` = unknown.
    #[serde(default)]
    pub bid_sz: f64,
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
    #[scylla(rename = "volume")]
    pub vol: f64,
}

impl Candlestick {
    /// Blank candle at `now` (truncated to the minute).
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
            ask: 0.0,
            ask_sz: 0.0,
            bid: 0.0,
            bid_sz: 0.0,
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

    /// Level 1 of the book for this token.
    ///
    /// Falls back to `last` widened by [`FALLBACK_HALF_SPREAD_BPS`] when the feed
    /// gave us no book, with both sizes marked unknown. The fallback widens rather
    /// than collapsing to `last` on both sides, because a zero-spread assumption
    /// is exactly the thing that made the old simulator optimistic.
    #[must_use]
    pub fn top_of_book(&self) -> TopOfBook {
        if self.bid > 0.0
            && self.ask > 0.0
            && self.bid.is_finite()
            && self.ask.is_finite()
            && self.ask >= self.bid
        {
            return TopOfBook {
                bid: self.bid,
                bid_sz: self.bid_sz.max(0.0),
                ask: self.ask,
                ask_sz: self.ask_sz.max(0.0),
                synthetic: false,
            };
        }
        let half = FALLBACK_HALF_SPREAD_BPS / 10_000.0;
        TopOfBook {
            bid: self.price * (1.0 - half),
            bid_sz: 0.0,
            ask: self.price * (1.0 + half),
            ask_sz: 0.0,
            synthetic: true,
        }
    }

    /// Quoted spread in basis points, or `None` when there is no book.
    ///
    /// A round trip already costs `2 x taker_fee` (0.20% at the configured 0.1%);
    /// the spread is paid on top of that, twice, and against a 1% take-profit it
    /// is not a rounding error.
    #[must_use]
    pub fn spread_bps(&self) -> Option<f64> {
        if self.bid <= 0.0 || self.ask <= 0.0 || self.ask < self.bid {
            return None;
        }
        let mid = (self.ask + self.bid) / 2.0;
        (mid > 0.0).then(|| (self.ask - self.bid) / mid * 10_000.0)
    }

    /// The price we will actually bid to enter: the **ask**, rounded up to the
    /// instrument tick.
    ///
    /// Was `floor_to_step(self.price, meta.tick_sz)` — a bid floored *below* the
    /// last trade, sent IOC. An IOC only fills against resting liquidity at or
    /// better than our limit, so that order could only fill when a seller came
    /// down onto it. Every fill was a fill into weakness; the ones where the move
    /// continued never happened at all.
    ///
    /// Bidding exactly the ask has a second, useful property: we never reach past
    /// level 1, so `ask_sz` alone tells us the whole truth about the fill. No
    /// depth data required.
    #[must_use]
    pub fn entry_limit(&self, meta: InstrumentMeta) -> f64 {
        ceil_to_step(self.top_of_book().ask, meta.tick_sz)
    }

    /// The price we will offer to exit: the **bid**, floored to the tick.
    #[must_use]
    pub fn exit_limit(&self, meta: InstrumentMeta) -> f64 {
        floor_to_step(self.top_of_book().bid, meta.tick_sz)
    }

    pub async fn buy(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        config: &StrategyConfig,
        now: DateTime<Utc>,
        meta: InstrumentMeta,
    ) -> Result<&Self> {
        // Was: `floor_to_step(self.price, meta.tick_sz)` — a bid *below* the
        // last trade, sent IOC. See `entry_limit`.
        self.buy_price = self.entry_limit(meta);
        let mut order = trade::Order::new(
            &self.instid,
            meta.format_price(self.buy_price),
            meta.format_size(self.balance.start),
            Side::Buy,
            &config.order_type,
            &config.hash,
            now,
        );
        order.publish(trade_enabled, &auth).await?;
        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }

    /// Applies a strategy exit decision.
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
        };
        let sell_balance = floor_to_step(raw_sell_balance, meta.lot_sz);
        let sell_price = self.exit_limit(meta);

        // Count sell attempts and go to market if we've been hanging around.
        //
        // NOTE: this is the only thing standing between a stoploss decision and
        // an unbounded loss, and at `cooldown = 5` it lets ~30s elapse before
        // it fires. If you keep a hard stop, consider dropping the threshold to
        // 1-2, or attaching an exchange-side trigger order at entry so the stop
        // survives this process dying.
        let sell_count = self
            .orders
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|o| o.side == Side::Sell)
            .count();

        let ord_type = match sell_count {
            x if x <= 5 => &config.order_type,
            _ => "market",
        };

        let mut order = trade::Order::new(
            &self.instid,
            meta.format_price(sell_price),
            meta.format_size(sell_balance),
            Side::Sell,
            ord_type,
            &config.hash,
            now,
        );

        order.publish(trade_enabled, &auth).await?;

        if order
            .response
            .as_ref()
            .is_some_and(|response| response.code.parse::<i64>().map_or(true, |code| code != 0))
        {
            order.state = OrderState::Failed;
        }

        self.orders.get_or_insert_with(Vec::new).push(order);

        Ok(self)
    }

    /// Read-only snapshot of this open position for `Strategy::should_exit`.
    ///
    /// No longer takes `still_listed`. The exit rules have no business asking
    /// whether the token we are *holding* still looks like a fresh *entry* — see
    /// `Thresholds::exit_decision`. It carries the position's peak instead, which is
    /// what `sell_floor` was always really about.
    ///
    /// `report.highest` is maintained by `update_reports`, which the main loop calls
    /// immediately before `should_exit`, so this is current.
    pub fn position_view(&self) -> PositionView {
        PositionView {
            instid: self.instid.clone(),
            change: f64::from(self.change),
            highest: f64::from(self.report.highest),
            timeout: self.timeout,
            candles: self.candlesticks.iter().map(Candlestick::view).collect(),
        }
    }

    /// Sets this position's timeout and sell floor from the strategy config.
    ///
    /// Replaces `configure_from_report`, which derived both from history: it
    /// took the **standard deviation** of past `highest` (peak gain) and
    /// `highest_elapsed` values and used them as this token's sell floor and
    /// timeout. A standard deviation measures dispersion, not level — the
    /// numbers only looked plausible by accident.
    ///
    /// The real damage was to the reports. It meant a strategy hash described
    /// two different things: the first trades on a fresh hash ran your config,
    /// later trades ran values derived from those earlier trades. Nothing
    /// tagged with a hash was comparable to anything else tagged with the same
    /// hash, which is fatal if you're trying to A/B configs. It also put two
    /// `ALLOW FILTERING` queries on the hot path of every entry.
    pub fn configure(&mut self, config: &StrategyConfig) -> &Self {
        self.config.timeout = Duration::seconds(config.timeout);
        self.timeout = self.config.timeout;
        self.config.sell_floor = config.sell_floor.unwrap_or(0.0);
        self
    }

    /// Read-only snapshot of this candidate token for `Strategy::should_enter`.
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
        let (vol, change, range) = self.candlesticks.iter().fold(
            (0.0_f64, 0.0_f64, 0.0_f64),
            |(vol_acc, change_acc, range_acc), x| {
                (vol_acc + x.vol, change_acc + x.change, range_acc + x.range)
            },
        );
        self.vol = vol;
        if self.status == token::Status::Waiting {
            self.change = change as f32;
        }
        self.range = range as f32;

        let changes: Vec<f32> = self.candlesticks.iter().map(|x| x.change as f32).collect();
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
        let book = self.top_of_book();
        let instid = self.instid.clone();
        if let Some(orders) = &mut self.orders {
            for order in orders.iter_mut().filter(|o| {
                o.state == OrderState::Live
                    && o.prev_state != OrderState::Created
                    && o.state != OrderState::Filled
            }) {
                if enable_trading {
                    let got_state = order.get_state(auth).await?;
                    if order.state != got_state {
                        order.state = got_state;
                    }
                    // TODO: `OkxOrderDetails` already carries `avg_px`. Read it
                    // here and stamp it onto `order.px` so `buy_price` is the
                    // price we *paid*, not the limit we asked for.
                } else {
                    order.state = simulate_ioc_fill(&instid, order, book);
                    order.id = order.cl_ord_id.clone();
                }

                // The entry reference for P&L is whatever we actually paid.
                // In simulation `simulate_ioc_fill` stamps the touch back onto
                // `order.px`; in live it is still the limit until the TODO above
                // is done.
                if order.state == OrderState::Filled && order.side == Side::Buy {
                    if let Ok(px) = order.px.parse::<f64>() {
                        self.buy_price = px;
                    }
                }

                self.status = Status::from_order(order);
            }
        }
        Ok(self)
    }
}

/// Level 1 of the order book, as OKX's `tickers` channel delivers it.
///
/// This is not a proxy for the book — it *is* the book, truncated to its best
/// level. `askPx` is the cheapest resting sell order and `askSz` is how much of
/// it there is. The `books` channel would add levels 2..N and nothing else.
///
/// Because every entry is priced at exactly the ask (see [`Token::entry_limit`]),
/// an IOC never reaches past this level. `ask_sz` therefore answers the whole
/// question on its own: the order fills in full iff `size <= ask_sz`. Depth data
/// is only needed to price the fills we have decided not to take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopOfBook {
    pub bid: f64,
    /// Size resting at the bid. `0.0` means *unknown*, not *empty*.
    pub bid_sz: f64,
    pub ask: f64,
    /// Size resting at the ask. `0.0` means *unknown*, not *empty*.
    pub ask_sz: f64,
    /// True when bid/ask were synthesized from `last` because the feed gave us
    /// no book.
    pub synthetic: bool,
}

impl TopOfBook {
    /// Whether `size` (base currency) fits inside the best ask.
    ///
    /// An unknown `ask_sz` returns `true`: we do not gate on data we do not have.
    /// That is a deliberate optimism, and `synthetic` marks where it applies.
    #[must_use]
    pub fn ask_covers(&self, size: f64) -> bool {
        self.ask_sz <= 0.0 || size <= self.ask_sz
    }

    /// Whether `size` fits inside the best bid.
    #[must_use]
    pub fn bid_covers(&self, size: f64) -> bool {
        self.bid_sz <= 0.0 || size <= self.bid_sz
    }

    /// Quote-currency notional resting at the best ask — the number to compare
    /// `account.spendable` against when deciding whether an instrument is deep
    /// enough to trade at all.
    #[must_use]
    pub fn ask_notional(&self) -> f64 {
        self.ask * self.ask_sz
    }
}

/// Fill model for `exchange.enable_trading = false`.
///
/// The old model was:
///
/// ```ignore
/// let random_state = if rng.gen_bool(1.0 / 6.0) { Filled } else { Cancelled };
/// ```
///
/// A coin flip, independent of price, direction, spread, size and book. It filled
/// a buy into a rising market exactly as readily as into a falling one, at the
/// last trade price, with zero slippage. That is the assumption that hides adverse
/// selection: in production an IOC bid below the ask does not fill *at all*, and
/// the ones that do fill are the ones the market is running away from. Every
/// "we almost break even" number came through that function.
///
/// What decides an IOC's fate is two questions, and this models both:
///
/// ```text
///   1. does our limit cross?      buy: limit >= ask     sell: limit <= bid
///   2. is there enough there?     buy: size <= ask_sz   sell: size <= bid_sz
/// ```
///
/// Both come from `okx.tickers`, so this is the real quoted book on the real
/// instrument — no constants, no depth data needed. A crossed IOC pays the touch,
/// not our limit, so the effective price is written back onto the order and
/// `buy_price` becomes what we would actually have paid.
///
/// **Size that exceeds the touch is treated as a cancel, not a partial fill.**
/// A real IOC would fill `ask_sz` and cancel the rest, leaving a position smaller
/// than intended; the scheduler's state machine has no honest path for that
/// (`Status::from_order` maps a filled sell straight to `Exited`, so a partial
/// exit would strand the remainder). Refusing the trade is the conservative
/// reading and it matches the entry guard in `buy_tokens`, which skips these
/// before an order is ever built.
///
/// **The one place this still flatters us** is a `market` sell whose size exceeds
/// `bid_sz` — the fallback after five failed IOC exits. A real market order walks
/// down the book and fills at an average worse than the bid; level-1 data cannot
/// tell us how much worse. Rather than invent a number, it fills at the bid and
/// logs a warning. Grep for `sim fill exceeded top of book`: if that line is rare,
/// level 1 is enough and the `books` channel would buy you nothing. If it is
/// common, you are too large for the instruments you are trading, and depth data
/// would only measure the damage more precisely.
///
/// Still not modeled: queue position, latency (the book you read is as of the last
/// Scylla write, and a real order arrives against a book that has moved), and
/// partial fills. Treat the output as an upper bound on live performance.
fn simulate_ioc_fill(instid: &str, order: &mut Order, book: TopOfBook) -> OrderState {
    if !book.bid.is_finite() || !book.ask.is_finite() || book.bid <= 0.0 || book.ask <= 0.0 {
        return OrderState::Cancelled;
    }
    let Ok(size) = order.sz.parse::<f64>() else {
        return OrderState::Failed;
    };

    if order.ord_type.eq_ignore_ascii_case("market") {
        let (touch, covered) = match order.side {
            Side::Buy => (book.ask, book.ask_covers(size)),
            Side::Sell => (book.bid, book.bid_covers(size)),
        };
        if !covered {
            log::warn!(
                "[{}] sim fill exceeded top of book: {} {:.8} vs resting {:.8}. \
                 Filling at the touch anyway — a real market order would walk the \
                 book and do worse. This number is optimistic.",
                instid,
                order.side.to_string(),
                size,
                match order.side {
                    Side::Buy => book.ask_sz,
                    Side::Sell => book.bid_sz,
                }
            );
        }
        order.px = touch.to_string();
        return OrderState::Filled;
    }

    let Ok(limit) = order.px.parse::<f64>() else {
        return OrderState::Failed;
    };

    match order.side {
        Side::Buy if limit >= book.ask => {
            if !book.ask_covers(size) {
                log::debug!(
                    "[{}] sim buy cancelled: size {:.8} > best ask size {:.8}",
                    instid,
                    size,
                    book.ask_sz
                );
                return OrderState::Cancelled;
            }
            order.px = book.ask.to_string();
            OrderState::Filled
        }
        Side::Sell if limit <= book.bid => {
            if !book.bid_covers(size) {
                log::debug!(
                    "[{}] sim sell cancelled: size {:.8} > best bid size {:.8}",
                    instid,
                    size,
                    book.bid_sz
                );
                return OrderState::Cancelled;
            }
            order.px = book.bid.to_string();
            OrderState::Filled
        }
        _ => OrderState::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(side: Side, ord_type: &str, px: f64, sz: f64) -> Order {
        Order::new(
            "TEST-USDT",
            px.to_string(),
            sz.to_string(),
            side,
            ord_type,
            "hash",
            DateTime::from_timestamp(0, 0).expect("epoch"),
        )
    }

    /// A token quoting 99.95 x 500 / 100.05 x 500, last trade 100.00.
    fn quoted() -> Token {
        let mut t = Token::new("TEST-USDT");
        t.price = 100.0;
        t.bid = 99.95;
        t.bid_sz = 500.0;
        t.ask = 100.05;
        t.ask_sz = 500.0;
        t
    }

    fn book() -> TopOfBook {
        quoted().top_of_book()
    }

    #[test]
    fn entry_limit_should_take_the_ask_not_undercut_the_last_trade() {
        // The old code bid floor(100.00) and waited. This bids the offer.
        let limit = quoted().entry_limit(InstrumentMeta::UNKNOWN);
        assert!((limit - 100.05).abs() < 1e-9);
    }

    #[test]
    fn exit_limit_should_hit_the_bid() {
        let limit = quoted().exit_limit(InstrumentMeta::UNKNOWN);
        assert!((limit - 99.95).abs() < 1e-9);
    }

    #[test]
    fn spread_bps_should_report_the_quoted_spread() {
        let spread = quoted().spread_bps().expect("book present");
        assert!((spread - 10.0).abs() < 0.01);
    }

    #[test]
    fn top_of_book_should_widen_the_last_trade_when_the_book_is_missing() {
        let mut t = Token::new("TEST-USDT");
        t.price = 100.0;
        let b = t.top_of_book();
        assert!(b.synthetic && b.bid < 100.0 && b.ask > 100.0);
    }

    #[test]
    fn unknown_ask_size_should_not_block_the_trade() {
        // We do not gate on data we do not have; `synthetic` marks where this
        // optimism applies.
        let mut t = Token::new("TEST-USDT");
        t.price = 100.0;
        assert!(t.top_of_book().ask_covers(1_000_000.0));
    }

    #[test]
    fn ask_notional_should_report_the_quote_currency_resting_at_the_ask() {
        // 500 units at 100.05 — the number to compare `spendable` against.
        assert!((book().ask_notional() - 50_025.0).abs() < 1e-6);
    }

    #[test]
    fn sim_buy_at_the_last_trade_should_not_fill() {
        // Exactly what the old `floor_to_step(price, tick)` bid produced.
        let mut o = order(Side::Buy, "ioc", 100.0, 10.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Cancelled
        );
    }

    #[test]
    fn sim_buy_that_crosses_the_ask_should_fill() {
        let mut o = order(Side::Buy, "ioc", 100.05, 10.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Filled
        );
    }

    #[test]
    fn sim_buy_should_pay_the_ask_not_our_limit() {
        let mut o = order(Side::Buy, "ioc", 100.20, 10.0);
        simulate_ioc_fill("TEST-USDT", &mut o, book());
        let paid: f64 = o.px.parse().expect("numeric");
        assert!((paid - 100.05).abs() < 1e-9);
    }

    #[test]
    fn sim_buy_larger_than_the_best_ask_should_not_fill() {
        // 600 units against 500 resting. A real IOC would take 500 and cancel the
        // rest; we refuse rather than open a position we can't size honestly.
        let mut o = order(Side::Buy, "ioc", 100.05, 600.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Cancelled
        );
    }

    #[test]
    fn sim_sell_that_crosses_the_bid_should_fill() {
        let mut o = order(Side::Sell, "ioc", 99.95, 10.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Filled
        );
    }

    #[test]
    fn sim_sell_larger_than_the_best_bid_should_not_fill() {
        // Falls through to the sell-retry loop, and to `market` after 5 attempts.
        let mut o = order(Side::Sell, "ioc", 99.95, 600.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Cancelled
        );
    }

    #[test]
    fn sim_market_order_should_always_fill() {
        let mut o = order(Side::Buy, "market", 0.0, 10.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Filled
        );
    }

    #[test]
    fn sim_market_order_beyond_the_touch_still_fills_but_is_optimistic() {
        // The one place the simulator knowingly flatters itself. It warns.
        let mut o = order(Side::Sell, "market", 0.0, 600.0);
        assert_eq!(
            simulate_ioc_fill("TEST-USDT", &mut o, book()),
            OrderState::Filled
        );
    }
}
