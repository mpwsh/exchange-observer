//! Shared entry checks used by every strategy.
//!
//! The three "gatekeeper" checks are semantically strategy-agnostic:
//!
//! - `denied`: token is on the deny-list (produced by `avoid_after_stoploss`
//!   or an explicit config list). Every strategy should honor this.
//! - `missing_candles`: strategies read `candles[..]` by index, so requiring
//!   the full timeframe's worth of history keeps them from silently reading
//!   partial windows and returning stale decisions.
//! - `min_vol`: without a minimum liquidity floor, we'd try to trade tokens
//!   whose bid/ask are too thin to fill.
//!
//! Everything else — momentum thresholds, deviation bands, dip/bounce
//! detection — is strategy-specific and lives in the strategy impl.
//!
//! Written as free functions returning `Option<Skip>` so callers can `?`-chain
//! them at the top of `entry_decision` without owning a `Thresholds` bag they
//! don't otherwise need.

use crate::{strategy::EnterDecision, views::TokenView};

/// Fails when the token is on the deny-list.
#[must_use]
pub fn denied(token: &TokenView) -> Option<EnterDecision> {
    if token.denied {
        Some(EnterDecision::Skip("denied"))
    } else {
        None
    }
}

/// Fails when the token's candle history is shorter than the strategy's window.
#[must_use]
pub fn missing_candles(token: &TokenView, required: usize) -> Option<EnterDecision> {
    if token.candles.len() < required {
        Some(EnterDecision::Skip("missing_candles"))
    } else {
        None
    }
}

/// Fails when the token's total volume over the window is below `min_vol`.
///
/// Panics if `min_vol` is `None` — matching `ThresholdStrategy`'s original
/// behavior of erroring loudly on missing config rather than silently
/// admitting every token.
#[must_use]
pub fn volume_below_min(token: &TokenView, min_vol: Option<f64>) -> Option<EnterDecision> {
    let min_vol = min_vol.expect("strategy config `min_vol` must be set to evaluate entries");
    // `!(pass)` so NaN inputs fail like the original `&&` chain would.
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "NaN must fail the check, matching the original comparison"
    )]
    if !(token.vol > min_vol) {
        Some(EnterDecision::Skip("volume_below_min"))
    } else {
        None
    }
}

/// Fails when the *last* candle's volume was below `spendable`.
///
/// Weaker than a full order-book check but enough to weed out tokens whose
/// most-recent minute had no trades — buying into that guarantees a wide
/// spread and a bad fill.
#[must_use]
pub fn last_candle_volume_below_spendable(
    token: &TokenView,
    spendable: f64,
) -> Option<EnterDecision> {
    let last_vol = token.candles.last().map_or(0.0, |c| c.vol);
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "NaN must fail the check, matching the original comparison"
    )]
    if !(last_vol >= spendable) {
        Some(EnterDecision::Skip("last_candle_volume_below_spendable"))
    } else {
        None
    }
}
