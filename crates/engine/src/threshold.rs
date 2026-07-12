//! `ThresholdStrategy`: a pure port of the scheduler's inline threshold
//! logic (`Token::is_valid`, `Token::get_exit_reason`, and the sizing rule
//! from `App::buy_tokens`). Behavior is intentionally identical — including
//! check order, float-comparison semantics (every `f32` is widened exactly
//! to `f64`), the `min_vol` panic on entry, and the disabled `LowVolume`
//! exit. Improve the logic in a separate change, never in this port.

use chrono::Duration;

use crate::{
    StrategyConfig,
    strategy::{Context, EnterDecision, ExitDecision, ExitReason, Strategy},
    views::{PositionView, TokenView},
};

/// The original threshold-based buy/sell strategy.
///
/// Stateless: all tunables come from [`Context::config`], snapshotted into
/// [`Thresholds`] per decision so the decision core stays independent of the
/// config crate (and trivially testable).
#[derive(Debug, Default, Clone, Copy)]
pub struct ThresholdStrategy;

impl Strategy for ThresholdStrategy {
    fn name(&self) -> &str {
        "threshold"
    }

    fn should_enter(&self, ctx: &Context<'_>, token: &TokenView) -> EnterDecision {
        Thresholds::from(ctx.config).entry_decision(ctx.portfolio.spendable, token)
    }

    fn should_exit(&self, ctx: &Context<'_>, position: &PositionView) -> ExitDecision {
        Thresholds::from(ctx.config).exit_decision(position)
    }
}

/// Typed snapshot of the threshold values [`ThresholdStrategy`] consults.
///
/// Owning this here keeps the decision core decoupled from
/// `exchange_observer::Strategy`'s full shape and lets tests construct
/// configurations directly.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    /// Timeframe in minutes (also the expected candle count).
    pub timeframe: i64,
    /// Position lifetime in seconds.
    pub timeout: i64,
    /// Minimum summed change over the timeframe (percent).
    pub min_change: f32,
    /// Minimum change of the newest candle (percent).
    pub min_change_last_candle: f32,
    /// Minimum standard deviation of candle changes.
    pub min_deviation: f32,
    /// Maximum standard deviation of candle changes.
    pub max_deviation: f32,
    /// Exit when change drops to `-stoploss` (percent).
    pub stoploss: f32,
    /// Exit when change reaches `cashout` (percent).
    pub cashout: f32,
    /// Min rising candles to select a token
    pub min_rising_candles: Option<u32>,
    /// Exit floor once the token falls off the top list (percent).
    pub sell_floor: Option<f32>,
    /// Minimum total volume over the timeframe (quote currency).
    /// `None` preserves the original behavior: entry evaluation panics.
    pub min_vol: Option<f64>,
}

impl From<&StrategyConfig> for Thresholds {
    fn from(config: &StrategyConfig) -> Self {
        Self {
            timeframe: config.timeframe,
            timeout: config.timeout,
            min_change: config.min_change,
            min_rising_candles: config.min_rising_candles,
            min_change_last_candle: config.min_change_last_candle,
            min_deviation: config.min_deviation,
            max_deviation: config.max_deviation,
            stoploss: config.stoploss,
            cashout: config.cashout,
            sell_floor: config.sell_floor,
            min_vol: config.min_vol,
        }
    }
}

impl Thresholds {
    /// Entry decision — port of `Token::is_valid` plus the sizing rule from
    /// `App::buy_tokens` (`spendable` quote currency per position).
    ///
    /// Checks run in the original short-circuit order. Conditions are written
    /// as `if !(pass)` rather than the inverted comparison so NaN inputs fail
    /// the check exactly as they did before.
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "!(pass) is deliberate: NaN inputs must fail checks exactly \
                  as they did in the original `&&` chain"
    )]
    pub fn entry_decision(&self, spendable: f64, token: &TokenView) -> EnterDecision {
        let timeframe = self.timeframe as usize;

        if token.denied {
            return EnterDecision::Skip("denied");
        }
        // No missing candles in our data.
        if token.candles.len() < timeframe {
            return EnterDecision::Skip("missing_candles");
        }
        // At least half of the candles should have higher volume than our spendable.
        let candles_above_spendable = token.candles.iter().filter(|c| c.vol > spendable).count();
        if candles_above_spendable < timeframe / 2 {
            return EnterDecision::Skip("candle_volume_below_spendable");
        }
        let candles_with_change = token.candles.iter().filter(|c| c.change > 0.0).count();
        // Enough candles are rising. Configurable via `min_rising_candles`;
        // unset falls back to the original rule (at least half, integer div).
        let required_rising = self
            .min_rising_candles
            .map_or(timeframe / 2, |n| n as usize);
        if candles_with_change < required_rising {
            return EnterDecision::Skip("too_few_rising_candles");
        }
        if !(token.change >= f64::from(self.min_change)) {
            return EnterDecision::Skip("change_below_min");
        }
        if !(token.std_deviation >= f64::from(self.min_deviation)
            && token.std_deviation <= f64::from(self.max_deviation))
        {
            return EnterDecision::Skip("deviation_out_of_range");
        }
        // The original fell back to a blank candle (vol 0, change 0) when the
        // token had no candles at all; preserved for degenerate timeframes.
        let (last_vol, last_change) = token
            .candles
            .last()
            .map_or((0.0, 0.0), |c| (c.vol, c.change));
        if !(last_vol >= spendable) {
            return EnterDecision::Skip("last_candle_volume_below_spendable");
        }
        if !(last_change > f64::from(self.min_change_last_candle)) {
            return EnterDecision::Skip("last_candle_change_below_min");
        }
        // Panic preserved from the original `strategy.min_vol.unwrap()`:
        // reaching this check without min_vol configured was already fatal.
        let min_vol = self
            .min_vol
            .expect("strategy config `min_vol` must be set to evaluate entries");
        if !(token.vol > min_vol) {
            return EnterDecision::Skip("volume_below_min");
        }

        EnterDecision::Enter {
            size_quote: spendable,
        }
    }

    /// Exit decision — port of `Token::get_exit_reason`.
    pub fn exit_decision(&self, position: &PositionView) -> ExitDecision {
        let timeout_threshold = Duration::seconds(self.timeout - 5);
        let sell_floor = f64::from(self.sell_floor.unwrap_or(0.0));
        let volume_threshold =
            self.min_vol.unwrap_or((self.timeframe * 1600) as f64) / self.timeframe as f64;

        let low_volume = position
            .candles
            .iter()
            .filter(|c| c.vol < volume_threshold)
            .count();

        if position.timeout.num_seconds() <= 0 {
            return ExitDecision::Exit(ExitReason::Timeout);
        }

        if position.change <= -f64::from(self.stoploss) {
            return ExitDecision::Exit(ExitReason::Stoploss);
        }

        if position.change >= f64::from(self.cashout) {
            return ExitDecision::Exit(ExitReason::Cashout);
        }

        if position.change >= sell_floor
            && position.timeout < timeout_threshold
            && !position.still_listed
        {
            return ExitDecision::Exit(ExitReason::FloorReached);
        }

        if low_volume as i64 >= self.timeframe / 2 {
            // Half of the candles in the selected timeframe show volume lower
            // than our spendable. Disabled in the original — kept for parity
            // until LowVolume exits are deliberately reinstated.
            // return ExitDecision::Exit(ExitReason::LowVolume);
        }

        ExitDecision::Hold
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn candle(change: f64, vol: f64) -> crate::views::Candle {
        crate::views::Candle {
            ts: ts(),
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            change,
            range: 0.0,
            vol,
        }
    }

    fn thresholds() -> Thresholds {
        Thresholds {
            timeframe: 4,
            timeout: 60,
            min_change: 0.5,
            min_change_last_candle: 0.1,
            min_deviation: 0.05,
            max_deviation: 5.0,
            stoploss: 1.0,
            cashout: 2.0,
            sell_floor: Some(0.3),
            min_vol: Some(1000.0),
        }
    }

    /// A token that passes every entry check for [`thresholds`] with
    /// `spendable = 100.0`.
    fn passing_token() -> TokenView {
        TokenView {
            instid: "BTC-USDT".to_string(),
            price: 10.0,
            change: 1.0,
            std_deviation: 0.2,
            vol: 2000.0,
            denied: false,
            candles: vec![
                candle(0.2, 500.0),
                candle(0.3, 500.0),
                candle(0.2, 500.0),
                candle(0.3, 500.0),
            ],
        }
    }

    fn holding_position() -> PositionView {
        PositionView {
            instid: "BTC-USDT".to_string(),
            change: 0.1,
            timeout: Duration::seconds(58),
            still_listed: true,
            candles: vec![candle(0.2, 500.0); 4],
        }
    }

    // -- entry ---------------------------------------------------------------

    #[test]
    fn entry_should_size_position_at_spendable_when_all_checks_pass() {
        let decision = thresholds().entry_decision(100.0, &passing_token());
        assert_eq!(decision, EnterDecision::Enter { size_quote: 100.0 });
    }

    #[test]
    fn entry_should_skip_denied_token() {
        let token = TokenView {
            denied: true,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("denied")
        );
    }

    #[test]
    fn entry_should_skip_token_with_missing_candles() {
        let mut token = passing_token();
        token.candles.pop();
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("missing_candles")
        );
    }

    #[test]
    fn entry_should_skip_when_most_candle_volumes_are_below_spendable() {
        let mut token = passing_token();
        for c in token.candles.iter_mut().take(3) {
            c.vol = 10.0;
        }
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("candle_volume_below_spendable")
        );
    }

    #[test]
    fn entry_should_skip_when_most_candles_are_not_rising() {
        let mut token = passing_token();
        for c in token.candles.iter_mut().take(3) {
            c.change = 0.0;
        }
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("too_few_rising_candles")
        );
    }

    #[test]
    fn entry_should_skip_when_change_is_below_min() {
        let token = TokenView {
            change: 0.4,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("change_below_min")
        );
    }

    #[test]
    fn entry_should_skip_when_change_is_nan() {
        let token = TokenView {
            change: f64::NAN,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("change_below_min")
        );
    }

    #[test]
    fn entry_should_skip_when_deviation_is_below_range() {
        let token = TokenView {
            std_deviation: 0.01,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("deviation_out_of_range")
        );
    }

    #[test]
    fn entry_should_skip_when_deviation_is_above_range() {
        let token = TokenView {
            std_deviation: 6.0,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("deviation_out_of_range")
        );
    }

    #[test]
    fn entry_should_skip_when_last_candle_volume_is_below_spendable() {
        let mut token = passing_token();
        token.candles[3].vol = 99.0;
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("last_candle_volume_below_spendable")
        );
    }

    #[test]
    fn entry_should_skip_when_last_candle_change_is_below_min() {
        let mut token = passing_token();
        token.candles[3].change = 0.1; // not strictly greater than 0.1
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("last_candle_change_below_min")
        );
    }

    #[test]
    fn entry_should_skip_when_total_volume_is_below_min_vol() {
        let token = TokenView {
            vol: 999.0,
            ..passing_token()
        };
        assert_eq!(
            thresholds().entry_decision(100.0, &token),
            EnterDecision::Skip("volume_below_min")
        );
    }

    #[test]
    #[should_panic(expected = "min_vol")]
    fn entry_should_panic_when_min_vol_is_unset_like_the_original() {
        let config = Thresholds {
            min_vol: None,
            ..thresholds()
        };
        let _ = config.entry_decision(100.0, &passing_token());
    }

    // -- exit ----------------------------------------------------------------

    #[test]
    fn exit_should_hold_a_healthy_listed_position() {
        let decision = thresholds().exit_decision(&holding_position());
        assert_eq!(decision, ExitDecision::Hold);
    }

    #[test]
    fn exit_should_trigger_timeout_when_time_runs_out() {
        let position = PositionView {
            timeout: Duration::seconds(0),
            ..holding_position()
        };
        assert_eq!(
            thresholds().exit_decision(&position),
            ExitDecision::Exit(ExitReason::Timeout)
        );
    }

    #[test]
    fn exit_should_trigger_stoploss_when_loss_reaches_threshold() {
        let position = PositionView {
            change: -1.0,
            ..holding_position()
        };
        assert_eq!(
            thresholds().exit_decision(&position),
            ExitDecision::Exit(ExitReason::Stoploss)
        );
    }

    #[test]
    fn exit_should_trigger_cashout_when_gain_reaches_threshold() {
        let position = PositionView {
            change: 2.0,
            ..holding_position()
        };
        assert_eq!(
            thresholds().exit_decision(&position),
            ExitDecision::Exit(ExitReason::Cashout)
        );
    }

    #[test]
    fn exit_should_trigger_floor_when_delisted_below_timeout_threshold() {
        // Boundary convention: `change` reaches the engine as an exactly
        // widened f32 (the scheduler stores it as f32). A raw 0.3_f64 would
        // sit *below* the widened 0.3_f32 sell floor and hold instead.
        let position = PositionView {
            change: f64::from(0.3_f32),
            timeout: Duration::seconds(54),
            still_listed: false,
            ..holding_position()
        };
        assert_eq!(
            thresholds().exit_decision(&position),
            ExitDecision::Exit(ExitReason::FloorReached)
        );
    }

    #[test]
    fn exit_should_hold_at_floor_while_token_is_still_listed() {
        let position = PositionView {
            change: f64::from(0.3_f32),
            timeout: Duration::seconds(54),
            still_listed: true,
            ..holding_position()
        };
        assert_eq!(thresholds().exit_decision(&position), ExitDecision::Hold);
    }

    #[test]
    fn exit_should_hold_at_floor_before_timeout_threshold_is_crossed() {
        // timeout_threshold = timeout - 5 = 55s; 58s remaining is above it.
        let position = PositionView {
            change: f64::from(0.3_f32),
            still_listed: false,
            ..holding_position()
        };
        assert_eq!(thresholds().exit_decision(&position), ExitDecision::Hold);
    }

    #[test]
    fn exit_should_hold_when_low_volume_because_the_reason_is_disabled() {
        let position = PositionView {
            candles: vec![candle(0.2, 1.0); 4],
            ..holding_position()
        };
        assert_eq!(thresholds().exit_decision(&position), ExitDecision::Hold);
    }

    #[test]
    fn exit_timeout_should_win_over_stoploss_like_the_original_order() {
        let position = PositionView {
            timeout: Duration::seconds(-1),
            change: -5.0,
            ..holding_position()
        };
        assert_eq!(
            thresholds().exit_decision(&position),
            ExitDecision::Exit(ExitReason::Timeout)
        );
    }
}
