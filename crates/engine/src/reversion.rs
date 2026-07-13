//! `ReversionStrategy`: buy tokens that have dipped and are now bouncing.
//!
//! Companion to [`ThresholdStrategy`], testing the opposite premise. Where
//! `ThresholdStrategy` chases sustained upward momentum, `ReversionStrategy`
//! looks for a **downtrend that just started to recover**: a stretch of
//! negative-change candles ending with one or more positive candles.
//!
//! ## The intuition
//!
//! Momentum-chasing on 1-minute crypto candles buys near local tops by
//! definition — the signal peaks when the move is most exhausted. If the
//! recorded reports show 60-90% of losses come from "bad entry" (position
//! never rose above +0.1%), the entry timing is the problem.
//!
//! Short-timeframe mean reversion inverts the timing:
//!
//! - Look for tokens whose trailing "dip window" is significantly negative.
//! - Confirm the recent bounce: the last N candles must be positive.
//! - Avoid catching falling knives: skip when the current low is at or below
//!   the window's minimum (a still-falling token).
//!
//! ## Windows
//!
//! Windows are indexed from the *end* of the candle list (oldest first upstream):
//!
//! - `bounce_window`: the trailing candles used to confirm a bounce.
//! - `dip_window`: candles *ending just before* the bounce window used to
//!   measure the dip.
//!
//! Given 20 candles with `dip_window = 10`, `bounce_window = 2`:
//! ```text
//!   [ . . . . . . . . D D D D D D D D D D B B ]
//!                     └── dip (10) ──────┘└─┘
//!                                        bounce (2)
//! ```
//!
//! When `dip_window + bounce_window > candles.len()`, entry is skipped
//! ("missing_candles") — same failure mode as the momentum path.
//!
//! ## Exits
//!
//! Exit logic is delegated to [`Thresholds::exit_decision`]. The exit reasons
//! (stoploss, cashout, sell_floor, timeout) are entry-agnostic — they close a
//! position based on its P&L trajectory, not how it was chosen.

use crate::{
    common_checks,
    strategy::{Context, EnterDecision, ExitDecision, Strategy},
    threshold::Thresholds,
    views::{Candle, PositionView, TokenView},
    StrategyConfig,
};

/// Buy dips that have just started to recover.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReversionStrategy;

impl Strategy for ReversionStrategy {
    fn name(&self) -> &str {
        "reversion"
    }

    fn should_enter(&self, ctx: &Context<'_>, token: &TokenView) -> EnterDecision {
        ReversionThresholds::from(ctx.config).entry_decision(ctx.portfolio.spendable, token)
    }

    fn should_exit(&self, ctx: &Context<'_>, position: &PositionView) -> ExitDecision {
        // Exit rules are entry-agnostic — reuse ThresholdStrategy's port.
        Thresholds::from(ctx.config).exit_decision(position)
    }
}

/// Reversion-specific tunables.
///
/// A separate bag from `Thresholds` so the two strategies' fields don't
/// entangle at the type level. Shared config fields (min_vol, timeframe,
/// exit thresholds) are read via `Thresholds` when exit_decision runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ReversionThresholds {
    /// Timeframe in minutes (also the expected candle count).
    pub timeframe: i64,
    /// Total volume floor (quote currency, over the timeframe).
    pub min_vol: Option<f64>,
    /// How many candles at the tail of the window form the "bounce".
    /// Must be >= 1. Typical value: 2-3.
    pub bounce_window: usize,
    /// How many candles *before* the bounce form the "dip".
    /// Must be >= 1. Typical value: 6-10.
    pub dip_window: usize,
    /// Minimum required *magnitude* of the dip sum (percent, positive).
    /// The dip candles' change sum must be <= -min_dip.
    /// Typical value: 0.3-0.5.
    pub min_dip: f64,
    /// Minimum required bounce sum (percent). The bounce candles' change
    /// sum must be >= min_bounce. Typical value: 0.05-0.15.
    pub min_bounce: f64,
    /// If true, skip tokens whose most-recent candle low is at/below the
    /// dip window's low — those are still falling, not reverting.
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
            min_bounce: f64::from(config.min_bounce.unwrap_or(0.05)),
            avoid_falling_knives: config.avoid_falling_knives.unwrap_or(true),
        }
    }
}

impl ReversionThresholds {
    /// Entry decision. Runs shared gatekeeper checks first, then the
    /// reversion-specific dip/bounce checks, then optional falling-knife
    /// filter. `Enter` size is the caller's spendable — same convention as
    /// `Thresholds::entry_decision`.
    pub fn entry_decision(&self, spendable: f64, token: &TokenView) -> EnterDecision {
        // Shared gatekeepers first — cheap, fail-fast, strategy-agnostic.
        if let Some(skip) = common_checks::denied(token) {
            return skip;
        }
        if let Some(skip) = common_checks::missing_candles(token, self.timeframe as usize) {
            return skip;
        }
        if let Some(skip) = common_checks::last_candle_volume_below_spendable(token, spendable) {
            return skip;
        }
        if let Some(skip) = common_checks::volume_below_min(token, self.min_vol) {
            return skip;
        }

        // Window arithmetic. `dip_window + bounce_window` must fit within
        // `candles.len()`; otherwise the strategy would read overlapping
        // ranges and confuse itself.
        let n = token.candles.len();
        if self.bounce_window == 0 || self.dip_window == 0 {
            return EnterDecision::Skip("window_size_zero");
        }
        if n < self.bounce_window + self.dip_window {
            return EnterDecision::Skip("window_too_small");
        }

        // `candles` is oldest→newest; bounce is the tail, dip is the slice
        // immediately before it.
        let bounce_start = n - self.bounce_window;
        let dip_start = bounce_start.saturating_sub(self.dip_window);
        let dip_slice = &token.candles[dip_start..bounce_start];
        let bounce_slice = &token.candles[bounce_start..];

        // Dip check: cumulative change over the dip window must be negative
        // by at least `min_dip`. `-min_dip` because `min_dip` is stored as a
        // positive magnitude for readability in configs.
        let dip_sum: f64 = dip_slice.iter().map(|c| c.change).sum();
        #[expect(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
        )]
        if !(dip_sum <= -self.min_dip) {
            return EnterDecision::Skip("dip_too_shallow");
        }

        // Bounce check: cumulative change over the bounce window must be
        // positive by at least `min_bounce`.
        let bounce_sum: f64 = bounce_slice.iter().map(|c| c.change).sum();
        #[expect(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN inputs must fail the check, matching the threshold-strategy convention"
        )]
        if !(bounce_sum >= self.min_bounce) {
            return EnterDecision::Skip("bounce_too_weak");
        }

        // Falling-knife guard: if the newest candle's low is at or below
        // the dip window's low, the trend is still down and the "bounce"
        // is likely a dead-cat. Compare with a tiny margin so exact-tie
        // ties still skip.
        if self.avoid_falling_knives {
            let dip_low = min_f64(dip_slice.iter().map(|c| c.low));
            let bounce_low = min_f64(bounce_slice.iter().map(|c| c.low));
            // Use <= so an exact retest of the low still skips.
            if bounce_low <= dip_low {
                return EnterDecision::Skip("still_falling");
            }
        }

        EnterDecision::Enter {
            size_quote: spendable,
        }
    }
}

/// f64 iterator min that handles NaN by treating it as "not smaller" —
/// prevents a single NaN candle low from making every comparison NaN.
fn min_f64<I: IntoIterator<Item = f64>>(iter: I) -> f64 {
    iter.into_iter()
        .fold(f64::INFINITY, |acc, x| if x < acc { x } else { acc })
}

// Kept out of the public interface: consumers hold `ReversionStrategy`, not
// `Candle`, when calling. Re-exported here purely so the doc comment above
// can link to it.
#[allow(unused_imports)]
use Candle as _;

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::views::Candle;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn candle(change: f64, vol: f64, low: f64) -> Candle {
        Candle {
            ts: ts(),
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
            min_bounce: 0.1,
            avoid_falling_knives: true,
        }
    }

    /// 10 candles: 3 flat, 5 down (sum -1.0), 2 up (sum +0.3). Falling-knife
    /// safe (bounce low > dip low). Should Enter.
    fn passing_token() -> TokenView {
        let mut candles = Vec::new();
        for _ in 0..3 {
            candles.push(candle(0.0, 200.0, 100.0));
        }
        // dip: 5 x -0.2, low walks down
        for i in 0..5 {
            candles.push(candle(-0.2, 200.0, 99.0 - i as f64 * 0.1));
        }
        // bounce: 2 x +0.15, low rebounds above dip's minimum
        candles.push(candle(0.15, 200.0, 98.7));
        candles.push(candle(0.15, 200.0, 98.8));

        TokenView {
            instid: "TEST-USDT".into(),
            price: 1.0,
            change: -0.4, // ignored by reversion; kept for completeness
            std_deviation: 0.5,
            vol: 2000.0,
            denied: false,
            candles,
        }
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
        // Even with candles = [] the shared check should short-circuit.
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
        // Force the bounce candles' lows below the dip's low.
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
        token.candles.last_mut().unwrap().vol = 10.0;
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
        // NaN in the dip should not accidentally pass the -min_dip threshold.
        token.candles[3].change = f64::NAN;
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("dip_too_shallow")
        );
    }
}
