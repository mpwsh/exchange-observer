//! `ReversionStrategy`: buy tokens that have dipped and are now bouncing.
//!
//! Companion to [`ThresholdStrategy`](crate::ThresholdStrategy), testing the
//! opposite premise. Where `ThresholdStrategy` chases sustained upward
//! momentum, `ReversionStrategy` looks for a **downtrend that just started to
//! recover**: a stretch of negative-change candles ending with one or more
//! positive candles.
//!
//! ## Windows
//!
//! Windows are indexed from the end of the *completed* candles (the list is
//! oldest-first):
//!
//! - `bounce_window`: the trailing completed candles used to confirm a bounce.
//! - `dip_window`: candles ending just before the bounce window, used to
//!   measure the dip.
//!
//! ## The in-progress candle: confirm on closed bars, veto on the live one
//!
//! The scheduler's `update_candles` builds the newest candle from the current
//! minute's tickers, so `candles.last()` is a **partial bar** — it may be three
//! seconds old and hold two prints. Reading a *bounce* off it means triggering on
//! whatever ticked in the last few seconds, which is noise, not a bar. So the dip
//! and bounce windows are taken over completed candles only ([`completed_len`]
//! drops the live bar).
//!
//! But that leaves a hole, and it is the hole that ate ALLO-USDT: the strategy
//! confirms a bounce on `M-2`/`M-1` and then buys at `M`'s ask — while minute `M`
//! is collapsing, in a bar it is not allowed to look at. The bounce was real. It
//! just wasn't *current*.
//!
//! The resolution is an asymmetry, and it is deliberate:
//!
//! > The live bar is too noisy to **confirm** a bounce, and plenty good enough to
//! > **veto** an entry.
//!
//! Those are claims held to different standards because the cost of being wrong
//! differs. "There is a bounce" is a positive claim we stake money on — it needs a
//! closed bar. "Price is collapsing right now" is a reason *not* to stake money,
//! and a partial bar clears that bar easily. A noisy signal is a bad reason to act
//! and a fine reason to wait.
//!
//! Concretely: `live_bar_new_low` — the live bar has undercut the dip's low. This
//! is nothing more than the falling-knife guard asked about *now* instead of about
//! a minute ago, so it rides on the same `avoid_falling_knives` flag. No new
//! setting: the check already existed, it was simply pointed at the wrong bar.
//!
//! The cost is real and worth stating: a token that wicks down at the start of a
//! minute and recovers within it is *exactly* the reversion we hunt, and from
//! inside the minute we cannot tell that wick from the first leg of a collapse. We
//! will skip some of those. Fewer entries, better ones.
//!
//! ## Exits
//!
//! Exit logic is delegated to [`Thresholds::exit_decision`] — exit reasons are
//! entry-agnostic.

use chrono::{DateTime, Utc};

use crate::{
    StrategyConfig, common_checks,
    strategy::{Context, EnterDecision, EntrySignal, ExitDecision, Strategy},
    threshold::Thresholds,
    views::{PositionView, TokenView},
};

/// Buy dips that have just started to recover.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReversionStrategy;

impl Strategy for ReversionStrategy {
    fn name(&self) -> &str {
        "reversion"
    }

    fn should_enter(&self, ctx: &Context<'_>, token: &TokenView) -> EnterDecision {
        ReversionThresholds::from(ctx.config).entry_decision_at(
            ctx.portfolio.spendable,
            token,
            Some(ctx.clock.now_utc()),
        )
    }

    fn should_exit(&self, ctx: &Context<'_>, position: &PositionView) -> ExitDecision {
        // Exit rules are entry-agnostic — reuse ThresholdStrategy's port.
        Thresholds::from(ctx.config).exit_decision(position)
    }

    /// The dip and bounce that actually fired, for `okx.reports`.
    ///
    /// `min_dip` and `min_bounce` are *thresholds*; these are the values that
    /// cleared them. Recording only the threshold tells you nothing — every entry
    /// cleared it by definition. Recording the value lets you ask whether a 0.5%
    /// dip and a 2.0% dip produce different outcomes, which is the only honest way
    /// to set the threshold.
    fn entry_signal(&self, ctx: &Context<'_>, token: &TokenView) -> EntrySignal {
        let cfg = ReversionThresholds::from(ctx.config);
        let end = completed_len(token, Some(ctx.clock.now_utc()));
        let Some((dip_start, bounce_start)) = cfg.window_bounds(end) else {
            return EntrySignal::default();
        };
        EntrySignal {
            dip: token.candles[dip_start..bounce_start]
                .iter()
                .map(|c| c.change)
                .sum(),
            bounce: token.candles[bounce_start..end]
                .iter()
                .map(|c| c.change)
                .sum(),
        }
    }

    /// Rank by how **unusual** the dip is for that token, not how large it is in
    /// percent: `score = -dip_sum / (sigma * sqrt(dip_window))`. Highest sigma wins.
    ///
    /// The old score was raw `-dip_sum`, and it was the last piece of the machine
    /// that funnelled a 250-token universe into PI-USDT. A volatile token has a
    /// deeper dip **by construction** — not because anything dislocated, but because
    /// that is what volatile means. So the eight names `clean_top` kept were, every
    /// cycle, simply the eight noisiest instruments available: exactly the ones with
    /// the widest spreads and the thinnest books, and the ones whose entire best bid
    /// your `spendable` exceeds.
    ///
    /// Normalising by the token's own volatility inverts that. A 4-sigma dip on SOL
    /// now outranks a 0.5-sigma wobble on PI, which is the ordering you want: rank by
    /// *dislocation*, and let the deep, cheap-to-trade instruments compete on merit.
    ///
    /// The bounce check still gates entry, so a deeply-dipped token that never
    /// recovers won't trade — it just wins the ranking race against tokens that
    /// aren't dipping at all.
    ///
    /// Falls back to raw `-dip_sum` when sigma is missing (a token with no volatility
    /// history cannot be normalised), and to `token.change` when the window
    /// arithmetic doesn't fit.
    fn rank_score(&self, ctx: &Context<'_>, token: &TokenView) -> f64 {
        let cfg = ReversionThresholds::from(ctx.config);
        let end = completed_len(token, Some(ctx.clock.now_utc()));
        let Some((dip_start, bounce_start)) = cfg.window_bounds(end) else {
            return token.change;
        };
        let dip_sum: f64 = token.candles[dip_start..bounce_start]
            .iter()
            .map(|c| c.change)
            .sum();

        let scale = window_scale(token.std_deviation, cfg.dip_window);
        if scale > 0.0 {
            -dip_sum / scale
        } else {
            -dip_sum
        }
    }
}

/// Index one past the newest **completed** candle.
///
/// When `now` falls inside the newest candle's minute, that candle is still
/// being built and is excluded. `None` (used by unit tests and by any caller
/// that already holds closed bars) treats every candle as complete.
fn completed_len(token: &TokenView, now: Option<DateTime<Utc>>) -> usize {
    let n = token.candles.len();
    match (token.candles.last(), now) {
        (Some(last), Some(now))
            if last.ts.timestamp().div_euclid(60) == now.timestamp().div_euclid(60) =>
        {
            n - 1
        },
        _ => n,
    }
}

/// Reversion-specific tunables.
#[derive(Debug, Clone, PartialEq)]
pub struct ReversionThresholds {
    /// Timeframe in minutes (also the expected candle count).
    pub timeframe: i64,
    /// Total volume floor (quote currency, over the timeframe).
    pub min_vol: Option<f64>,
    /// How many completed candles at the tail of the window form the "bounce".
    ///
    /// Must be >= 1. A value of 1 makes the entry signal a single 1-minute bar,
    /// which on these instruments is inside the noise band — prefer 2-3.
    pub bounce_window: usize,
    /// How many candles *before* the bounce form the "dip". Must be >= 1.
    pub dip_window: usize,
    /// Absolute floor on the dip magnitude (percent, positive). A **cost** test.
    ///
    /// A dip smaller than the round trip (2 x taker + spread, ~0.22%) is not a
    /// dislocation — it is the cost of trading. No amount of volatility scaling
    /// makes such a trade payable, which is why this floor survives alongside
    /// `min_dip_std`.
    pub min_dip: f64,
    /// Dip requirement in multiples of the token's own volatility. A
    /// **dislocation** test. `None` leaves `min_dip` as the only gate.
    pub min_dip_std: Option<f64>,
    /// Absolute floor on the bounce sum (percent).
    pub min_bounce: f64,
    /// Bounce requirement in multiples of the token's own volatility.
    pub min_bounce_std: Option<f64>,
    /// If true, skip tokens whose bounce-window low undercuts the dip window's
    /// low — those are still falling, not reverting.
    ///
    /// Also gates the `live_bar_new_low` veto: it is the same question ("is this
    /// still making new lows?") asked about the minute currently in progress.
    pub avoid_falling_knives: bool,
}

impl From<&StrategyConfig> for ReversionThresholds {
    fn from(config: &StrategyConfig) -> Self {
        Self {
            timeframe: config.timeframe,
            min_vol: config.min_vol,
            bounce_window: config.bounce_window.unwrap_or(2) as usize,
            dip_window: config.dip_window.unwrap_or(10) as usize,
            min_dip: f64::from(config.min_dip.unwrap_or(0.3)),
            min_dip_std: config.min_dip_std.map(f64::from),
            min_bounce: f64::from(config.min_bounce.unwrap_or(0.05)),
            min_bounce_std: config.min_bounce_std.map(f64::from),
            avoid_falling_knives: config.avoid_falling_knives.unwrap_or(true),
        }
    }
}

/// The move a random walk of per-candle volatility `sigma` covers over `n` bars:
/// `sigma * sqrt(n)`.
///
/// This is what makes a threshold portable across the universe. `min_dip = 0.5%`
/// over 3 candles is a **5.7 sigma** demand on SOL (sigma ~0.05%, so the 3-bar scale
/// is 0.087%) and a **0.58 sigma** demand on PI (sigma ~0.5%, scale 0.87%). The same
/// number, a ten-fold difference in what it asks for — which is how a 250-token
/// universe silently collapsed to whichever handful was noisiest. Expressed in
/// sigma, "a 2-sigma dislocation" means the same thing on both.
///
/// Returns `0.0` when sigma is absent or non-finite, which leaves the absolute floor
/// as the only gate — see `required()`.
fn window_scale(sigma: f64, window: usize) -> f64 {
    if !sigma.is_finite() || sigma <= 0.0 || window == 0 {
        return 0.0;
    }
    sigma * (window as f64).sqrt()
}

/// The threshold actually applied: the **larger** of the absolute floor and the
/// sigma multiple.
///
/// Both must be satisfied, and they ask different questions. The floor asks *is
/// there enough here to pay for the trade?* — a 0.17% dislocation cannot cover a
/// 0.22% round trip however unusual it is for that token. The sigma term asks *is
/// this unusual for this token at all?* — a 0.5% wobble on PI is half a standard
/// deviation, i.e. the token breathing, not a dislocation. Neither alone is
/// sufficient and the failure modes are opposite.
///
/// `f64::max` returns the non-NaN operand, so a NaN sigma degrades to the floor
/// rather than poisoning the comparison.
fn required(floor: f64, multiple: Option<f64>, sigma: f64, window: usize) -> f64 {
    match multiple {
        Some(k) => floor.max(k * window_scale(sigma, window)),
        None => floor,
    }
}

impl ReversionThresholds {
    /// `(dip_start, bounce_start)` for a candle list with `end` completed bars,
    /// or `None` when the windows don't fit.
    fn window_bounds(&self, end: usize) -> Option<(usize, usize)> {
        if self.bounce_window == 0 || self.dip_window == 0 {
            return None;
        }
        if end < self.bounce_window + self.dip_window {
            return None;
        }
        let bounce_start = end - self.bounce_window;
        let dip_start = bounce_start - self.dip_window;
        Some((dip_start, bounce_start))
    }

    /// Entry decision, treating every candle as complete. Convenience wrapper
    /// for tests and closed-bar callers; production goes through
    /// [`Self::entry_decision_at`].
    pub fn entry_decision(&self, spendable: f64, token: &TokenView) -> EnterDecision {
        self.entry_decision_at(spendable, token, None)
    }

    /// Entry decision as of `now`.
    ///
    /// `now` is what lets us tell a closed bar from the one currently being
    /// built. Pass `None` only when the candles are known to be closed.
    pub fn entry_decision_at(
        &self,
        spendable: f64,
        token: &TokenView,
        now: Option<DateTime<Utc>>,
    ) -> EnterDecision {
        // Shared gatekeepers first — cheap, fail-fast, strategy-agnostic.
        if let Some(skip) = common_checks::denied(token) {
            return skip;
        }
        if let Some(skip) = common_checks::missing_candles(token, self.timeframe as usize) {
            return skip;
        }
        if let Some(skip) = common_checks::volume_below_min(token, self.min_vol) {
            return skip;
        }

        let end = completed_len(token, now);

        // Liquidity check on the last *completed* candle.
        //
        // This used to read `candles.last()`, which in production is the
        // in-progress minute: a bar that may be seconds old and holds a fraction
        // of a minute's volume. The check was passing or failing on how far into
        // the current minute the scheduler happened to run.
        let last_vol = end
            .checked_sub(1)
            .and_then(|i| token.candles.get(i))
            .map_or(0.0, |c| c.vol);
        #[expect(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
        )]
        if !(last_vol >= spendable) {
            return EnterDecision::Skip("last_candle_volume_below_spendable");
        }

        if self.bounce_window == 0 || self.dip_window == 0 {
            return EnterDecision::Skip("window_size_zero");
        }
        let Some((dip_start, bounce_start)) = self.window_bounds(end) else {
            return EnterDecision::Skip("window_too_small");
        };

        let dip_slice = &token.candles[dip_start..bounce_start];
        let bounce_slice = &token.candles[bounce_start..end];

        // Dip check. The bar is the larger of the absolute floor and the sigma
        // multiple — see `required()`.
        let dip_required = required(
            self.min_dip,
            self.min_dip_std,
            token.std_deviation,
            self.dip_window,
        );
        let dip_sum: f64 = dip_slice.iter().map(|c| c.change).sum();
        #[expect(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
        )]
        if !(dip_sum <= -dip_required) {
            return EnterDecision::Skip("dip_too_shallow");
        }

        // Bounce check, same construction.
        let bounce_required = required(
            self.min_bounce,
            self.min_bounce_std,
            token.std_deviation,
            self.bounce_window,
        );
        let bounce_sum: f64 = bounce_slice.iter().map(|c| c.change).sum();
        #[expect(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
        )]
        if !(bounce_sum >= bounce_required) {
            return EnterDecision::Skip("bounce_too_weak");
        }

        // Falling-knife guard: if the bounce window's low is at or below the dip
        // window's low, the trend is still down and the "bounce" is a dead cat.
        let dip_low = min_f64(dip_slice.iter().map(|c| c.low));
        if self.avoid_falling_knives {
            let bounce_low = min_f64(bounce_slice.iter().map(|c| c.low));
            if bounce_low <= dip_low {
                return EnterDecision::Skip("still_falling");
            }
        }

        // Live-bar veto — the last thing checked, so a skip here means "the signal
        // was good and the present tense disagreed", which is the diagnostic worth
        // having in the logs.
        //
        // `candles[end]` is the in-progress minute when one exists. It won't for a
        // few seconds at the top of a minute (no ticks yet, and candle1m has no row
        // for it either), and in backtests over closed bars. No live bar, no veto.
        if let Some(live) = token.candles.get(end) {
            // The falling-knife guard, asked about *now*. The bounce confirmed on
            // M-2/M-1 that the low was holding; this asks whether it still is.
            // ALLO-USDT: dip 19:24-19:26, bounce 19:27-19:28 (clean, guard passed),
            // bought at 19:29:45 — into a minute that was taking out the low by a
            // full percent, in a bar the strategy was not allowed to see.
            #[expect(
                clippy::neg_cmp_op_on_partial_ord,
                reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
            )]
            if self.avoid_falling_knives && !(live.low > dip_low) {
                return EnterDecision::Skip("live_bar_new_low");
            }
        }

        EnterDecision::Enter {
            size_quote: spendable,
        }
    }
}

/// f64 iterator min that treats NaN as "not smaller" — prevents a single NaN
/// candle low from making every comparison NaN.
fn min_f64<I: IntoIterator<Item = f64>>(iter: I) -> f64 {
    iter.into_iter()
        .fold(f64::INFINITY, |acc, x| if x < acc { x } else { acc })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::*;
    use crate::views::Candle;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn candle(change: f64, vol: f64, low: f64) -> Candle {
        candle_at(ts(), change, vol, low)
    }

    fn candle_at(ts: DateTime<Utc>, change: f64, vol: f64, low: f64) -> Candle {
        Candle {
            ts,
            open: 1.0,
            high: 1.0,
            low,
            close: 1.0,
            change,
            range: 0.0,
            vol,
        }
    }

    fn thresholds() -> ReversionThresholds {
        ReversionThresholds {
            timeframe: 10,
            min_vol: Some(1000.0),
            bounce_window: 2,
            dip_window: 5,
            min_dip: 0.5,
            min_dip_std: None,
            min_bounce: 0.1,
            min_bounce_std: None,
            avoid_falling_knives: true,
        }
    }

    /// 10 candles with real per-minute timestamps, the last of which is the
    /// in-progress bar. With `end = 9`: dip = indices 2..7, bounce = 7..9, live = 9.
    ///
    /// dip low = 98.6, bounce low = 98.7 (holds), so the closed-bar signal passes
    /// and every veto test below varies only the live bar.
    fn live_bar_token() -> (TokenView, DateTime<Utc>) {
        let base = ts();
        let mut candles = Vec::new();
        for i in 0..2 {
            candles.push(candle_at(base + Duration::minutes(i), 0.0, 200.0, 100.0));
        }
        for i in 0..5 {
            candles.push(candle_at(
                base + Duration::minutes(2 + i),
                -0.2,
                200.0,
                99.0 - i as f64 * 0.1,
            ));
        }
        candles.push(candle_at(base + Duration::minutes(7), 0.15, 200.0, 98.7));
        candles.push(candle_at(base + Duration::minutes(8), 0.15, 200.0, 98.8));
        // The live bar: healthy by default.
        candles.push(candle_at(base + Duration::minutes(9), 0.05, 50.0, 98.9));

        let token = TokenView {
            instid: "TEST-USDT".into(),
            price: 1.0,
            change: -0.4,
            std_deviation: 0.5,
            vol: 2000.0,
            denied: false,
            candles,
        };
        // 20 seconds into the newest candle's minute.
        (token, base + Duration::minutes(9) + Duration::seconds(20))
    }

    /// 10 candles: 3 flat, 5 down (sum -1.0), 2 up (sum +0.3). Falling-knife
    /// safe (bounce low > dip low). Should Enter.
    fn passing_token() -> TokenView {
        let mut candles = Vec::new();
        for _ in 0..3 {
            candles.push(candle(0.0, 200.0, 100.0));
        }
        for i in 0..5 {
            candles.push(candle(-0.2, 200.0, 99.0 - i as f64 * 0.1));
        }
        candles.push(candle(0.15, 200.0, 98.7));
        candles.push(candle(0.15, 200.0, 98.8));

        TokenView {
            instid: "TEST-USDT".into(),
            price: 1.0,
            change: -0.4,
            std_deviation: 0.5,
            vol: 2000.0,
            denied: false,
            candles,
        }
    }

    /// Same shape, but with real per-minute timestamps so the in-progress-candle
    /// logic has something to bite on.
    fn timestamped_token() -> TokenView {
        let mut token = passing_token();
        let n = token.candles.len() as i64;
        for (i, c) in token.candles.iter_mut().enumerate() {
            c.ts = ts() + Duration::minutes(i as i64 - n + 1);
        }
        token
    }

    #[test]
    fn entry_enters_a_valid_dip_and_bounce() {
        assert_eq!(
            thresholds().entry_decision(100.0, &passing_token()),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    #[test]
    fn entry_skips_denied_tokens_before_reading_windows() {
        let mut token = passing_token();
        token.denied = true;
        token.candles.clear();
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("denied")
        );
    }

    #[test]
    fn entry_skips_when_dip_is_too_shallow() {
        let mut token = passing_token();
        for c in token.candles.iter_mut().skip(3).take(5) {
            c.change = -0.01;
        }
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("dip_too_shallow")
        );
    }

    #[test]
    fn entry_skips_when_bounce_is_too_weak() {
        let mut token = passing_token();
        for c in token.candles.iter_mut().rev().take(2) {
            c.change = 0.01;
        }
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("bounce_too_weak")
        );
    }

    #[test]
    fn entry_skips_falling_knives_when_bounce_low_undercuts_dip_low() {
        let mut token = passing_token();
        let last_idx = token.candles.len() - 1;
        token.candles[last_idx - 1].low = 90.0;
        token.candles[last_idx].low = 90.0;
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("still_falling")
        );
    }

    #[test]
    fn entry_ignores_falling_knife_when_guard_is_disabled() {
        let mut token = passing_token();
        let last_idx = token.candles.len() - 1;
        token.candles[last_idx - 1].low = 90.0;
        token.candles[last_idx].low = 90.0;

        let mut t = thresholds();
        t.avoid_falling_knives = false;
        assert_eq!(
            t.entry_decision(100.0, &token),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    #[test]
    fn entry_skips_when_window_configuration_exceeds_history() {
        let token = passing_token();
        let t = ReversionThresholds {
            dip_window: 20,
            bounce_window: 5,
            ..thresholds()
        };
        assert_eq!(
            t.entry_decision(100.0, &token),
            EnterDecision::Skip("window_too_small")
        );
    }

    #[test]
    fn entry_skips_on_zero_sized_windows() {
        let token = passing_token();
        let t = ReversionThresholds {
            dip_window: 0,
            ..thresholds()
        };
        assert_eq!(
            t.entry_decision(100.0, &token),
            EnterDecision::Skip("window_size_zero")
        );
    }

    #[test]
    fn entry_skips_when_last_candle_volume_below_spendable() {
        let mut token = passing_token();
        token.candles.last_mut().expect("non-empty").vol = 10.0;
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("last_candle_volume_below_spendable")
        );
    }

    #[test]
    fn entry_skips_when_total_volume_below_min() {
        let token = TokenView {
            vol: 500.0,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("volume_below_min")
        );
    }

    #[test]
    fn entry_treats_dip_nan_as_shallow_not_deep() {
        let mut token = passing_token();
        token.candles[3].change = f64::NAN;
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("dip_too_shallow")
        );
    }

    // -- in-progress candle --------------------------------------------------

    #[test]
    fn entry_excludes_the_in_progress_candle_from_the_windows() {
        // `now` sits inside the newest candle's minute, so it is a partial bar.
        // Dropping it leaves 9 completed candles, and 5 + 2 still fits — but the
        // bounce is now (dip candle #5, bounce candle #1), summing to -0.05, so
        // the bounce no longer clears `min_bounce`. Before this change, the
        // partial bar was treated as a closed one and the entry fired.
        let token = timestamped_token();
        let now = token.candles.last().expect("non-empty").ts;
        assert_eq!(
            thresholds().entry_decision_at(100.0, &token, Some(now)),
            EnterDecision::Skip("bounce_too_weak")
        );
    }

    #[test]
    fn entry_uses_the_newest_candle_once_its_minute_has_closed() {
        let token = timestamped_token();
        let now = token.candles.last().expect("non-empty").ts + Duration::minutes(1);
        assert_eq!(
            thresholds().entry_decision_at(100.0, &token, Some(now)),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    // -- live-bar veto -------------------------------------------------------

    #[test]
    fn entry_enters_when_the_live_bar_is_healthy() {
        let (token, now) = live_bar_token();
        assert_eq!(
            thresholds().entry_decision_at(100.0, &token, Some(now)),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    #[test]
    fn entry_vetoes_when_the_live_bar_takes_out_the_dip_low() {
        // ALLO-USDT. The closed bars show a clean dip and a clean bounce; the minute
        // in progress is a full percent below the dip's low. Every window check
        // passes and the trade is still wrong.
        let (mut token, now) = live_bar_token();
        token.candles.last_mut().expect("live bar").low = 98.5; // dip low is 98.6
        assert_eq!(
            thresholds().entry_decision_at(100.0, &token, Some(now)),
            EnterDecision::Skip("live_bar_new_low")
        );
    }

    #[test]
    fn entry_ignores_a_live_bar_new_low_when_the_knife_guard_is_disabled() {
        // The new-low veto rides on `avoid_falling_knives` — same question, asked
        // about the minute in progress.
        let (mut token, now) = live_bar_token();
        token.candles.last_mut().expect("live bar").low = 98.5;

        let t = ReversionThresholds {
            avoid_falling_knives: false,
            ..thresholds()
        };
        assert_eq!(
            t.entry_decision_at(100.0, &token, Some(now)),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    #[test]
    fn entry_does_not_veto_when_there_is_no_live_bar() {
        // Closed-bar callers (tests, backtests) pass `None`. There is then no bar at
        // `candles[end]` to read, so neither veto can fire — whatever the decision
        // turns out to be, it is not a live-bar skip.
        let (mut token, _) = live_bar_token();
        token.candles.last_mut().expect("live bar").low = 98.5;

        let decision = thresholds().entry_decision(100.0, &token);
        assert!(!matches!(decision, EnterDecision::Skip("live_bar_new_low")));
    }

    // -- volatility scaling --------------------------------------------------

    #[test]
    fn required_takes_the_larger_of_the_floor_and_the_sigma_multiple() {
        // SOL: sigma 0.05%, 3-candle scale 0.087%, 2 sigma = 0.173% -> the FLOOR binds.
        // A 0.173% dislocation cannot cover a 0.22% round trip however rare it is.
        assert!((required(0.35, Some(2.0), 0.05, 3) - 0.35).abs() < 1e-9);

        // PI: sigma 0.5%, 3-candle scale 0.866%, 2 sigma = 1.73% -> the SIGMA binds.
        // A 0.35% wobble on PI is the token breathing, not a dislocation.
        assert!((required(0.35, Some(2.0), 0.5, 3) - 1.732_050_8).abs() < 1e-6);
    }

    #[test]
    fn required_degrades_to_the_floor_without_a_sigma_multiple() {
        assert!((required(0.5, None, 0.5, 3) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn required_degrades_to_the_floor_on_a_nan_or_zero_sigma() {
        // A token with no volatility history cannot be normalised; do not poison the
        // comparison, just fall back to the absolute floor.
        assert!((required(0.5, Some(2.0), f64::NAN, 3) - 0.5).abs() < 1e-9);
        assert!((required(0.5, Some(2.0), 0.0, 3) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn entry_skips_a_dip_that_is_large_in_percent_but_small_in_sigma() {
        // The PI case. The dip sums to -1.0%, which clears a 0.5% floor easily — but
        // on a token whose sigma is 1.0% that is only 0.58 sigma over 3 candles. The
        // token is breathing, not dislocating.
        let mut token = passing_token();
        token.std_deviation = 1.0;
        let t = ReversionThresholds {
            dip_window: 5,
            min_dip: 0.5,
            min_dip_std: Some(2.0),
            ..thresholds()
        };
        assert_eq!(
            t.entry_decision(100.0, &token),
            EnterDecision::Skip("dip_too_shallow")
        );
    }

    #[test]
    fn entry_takes_the_same_dip_when_it_is_large_in_sigma() {
        // Identical candles. The only difference is that this token is quiet, so the
        // same -1.0% dip is a 2.2 sigma dislocation rather than noise.
        let mut token = passing_token();
        token.std_deviation = 0.2;
        let t = ReversionThresholds {
            dip_window: 5,
            min_dip: 0.5,
            min_dip_std: Some(2.0),
            min_bounce_std: None,
            ..thresholds()
        };
        assert_eq!(
            t.entry_decision(100.0, &token),
            EnterDecision::Enter { size_quote: 100.0 }
        );
    }

    #[test]
    fn rank_score_prefers_the_bigger_dislocation_not_the_bigger_percentage() {
        use crate::clock::TestClock;
        use crate::views::PortfolioView;

        let clock = TestClock::new(ts() + Duration::minutes(5));
        let mut config = StrategyConfig::default();
        config.timeframe = 10;
        config.min_vol = Some(1000.0);
        config.dip_window = Some(5);
        config.bounce_window = Some(2);
        config.min_dip = Some(0.5);
        config.min_bounce = Some(0.1);
        let portfolio = PortfolioView {
            available: 1000.0,
            spendable: 100.0,
            positions: 0,
        };
        let ctx = Context {
            clock: &clock,
            config: &config,
            portfolio: &portfolio,
        };

        // The noisy token dips TWICE as far in percent...
        let mut noisy = timestamped_token();
        noisy.std_deviation = 1.0;
        for c in noisy.candles.iter_mut().skip(3).take(5) {
            c.change = -0.4;
        }
        // ...but the quiet one's shallower dip is a far bigger event for it.
        let mut quiet = timestamped_token();
        quiet.std_deviation = 0.1;
        for c in quiet.candles.iter_mut().skip(3).take(5) {
            c.change = -0.2;
        }

        let strategy = ReversionStrategy;
        assert!(strategy.rank_score(&ctx, &quiet) > strategy.rank_score(&ctx, &noisy));
    }

    #[test]
    fn rank_score_prefers_deeper_dips() {
        use crate::StrategyConfig;
        use crate::clock::TestClock;
        use crate::views::PortfolioView;

        // A minute after the newest candle, so nothing is treated as in-progress.
        let clock = TestClock::new(ts() + Duration::minutes(5));
        let mut config = StrategyConfig::default();
        config.timeframe = 10;
        config.min_vol = Some(1000.0);
        config.dip_window = Some(5);
        config.bounce_window = Some(2);
        config.min_dip = Some(0.5);
        config.min_bounce = Some(0.1);
        let portfolio = PortfolioView {
            available: 1000.0,
            spendable: 100.0,
            positions: 0,
        };
        let ctx = Context {
            clock: &clock,
            config: &config,
            portfolio: &portfolio,
        };

        let mut shallow = timestamped_token();
        for c in shallow.candles.iter_mut().skip(3).take(5) {
            c.change = -0.1;
        }
        let mut deep = timestamped_token();
        for c in deep.candles.iter_mut().skip(3).take(5) {
            c.change = -0.4;
        }

        let strategy = ReversionStrategy;
        assert!(strategy.rank_score(&ctx, &deep) > strategy.rank_score(&ctx, &shallow));
    }
}
