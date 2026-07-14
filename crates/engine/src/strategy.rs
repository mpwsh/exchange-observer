//! The `Strategy` port: pure entry/exit decisions over view types.
//!
//! Implementations must be side-effect free — they read a [`Context`] plus a
//! view and return a typed decision. The scheduler owns execution (orders,
//! cooldowns, portfolio bookkeeping). This trait's shape is deliberately
//! serializable-at-the-edges: it becomes the wire protocol when strategy
//! runners are split into their own processes later.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{
    StrategyConfig,
    clock::Clock,
    views::{PortfolioView, PositionView, TokenView},
};

/// Everything a strategy may consult besides the token/position itself.
pub struct Context<'a> {
    /// Time source — strategies must not read ambient time.
    pub clock: &'a dyn Clock,
    /// The threshold bag from `lib` (`exchange_observer::Strategy`).
    pub config: &'a StrategyConfig,
    /// Account-level snapshot.
    pub portfolio: &'a PortfolioView,
}

/// A pluggable trading strategy.
///
/// Object safe (held as `&dyn Strategy` by the scheduler); `Send + Sync` so
/// future runners can evaluate strategies off the scheduler task.
pub trait Strategy: Send + Sync {
    /// Stable identifier for logs, reports and (later) routing.
    fn name(&self) -> &str;

    /// Decide whether to open a position in `token`.
    fn should_enter(&self, ctx: &Context<'_>, token: &TokenView) -> EnterDecision;

    /// Decide whether to close the open `position`.
    fn should_exit(&self, ctx: &Context<'_>, position: &PositionView) -> ExitDecision;

    /// Score a candidate token for ranking within the pool.
    ///
    /// The scheduler fetches a universal candidate pool each cycle and
    /// keeps only the top-N by this score (higher = better). Different
    /// strategies want different candidates:
    ///
    /// - Momentum wants tokens with the largest positive change (default).
    /// - Reversion wants tokens with the largest negative dip.
    ///
    /// Default returns `token.change`, preserving the pre-trait ranking of
    /// "sort by change descending" that `ThresholdStrategy` relied on.
    /// Strategies whose ideal candidate isn't "highest change" override.
    /// The signal values behind an entry, for the reports table.
    ///
    /// Called only when `should_enter` returned `Enter`, so recomputing the window
    /// sums here costs a handful of adds on at most `portfolio_size` tokens per
    /// cycle. Strategies with no such concept inherit zeros.
    fn entry_signal(&self, _ctx: &Context<'_>, _token: &TokenView) -> EntrySignal {
        EntrySignal::default()
    }

    /// Rank by how *deep* the recent dip is: score = `-dip_sum`, so the deepest
    /// dips get the highest score. The bounce check still gates entry, so a
    /// deeply-dipped token without a bounce won't trade — it just wins the
    /// ranking race against tokens that aren't dipping at all.
    ///
    /// Falls back to `token.change` when the window arithmetic doesn't fit.
    fn rank_score(&self, ctx: &Context<'_>, token: &TokenView) -> f64 {
        token.change
    }
}

/// Typed entry decision. `Skip` carries a reason so logs show *why* a
/// strategy didn't fire instead of a bare `false`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnterDecision {
    /// Open a position, spending `size_quote` in quote currency.
    Enter {
        /// Position size in quote currency (e.g. USDT).
        size_quote: f64,
    },
    /// Do not enter; the payload names the failed check.
    Skip(&'static str),
}

/// Diagnostic snapshot of the signal that fired an entry.
///
/// Written to `okx.reports` alongside the outcome, because without it the
/// reports record *what happened* and never *why we were there*. A question like
/// "do deeper dips produce better trades?" — the question that decides `min_dip`
/// — cannot be answered from the data at all, so the threshold ends up being set
/// by argument instead of evidence. That is exactly how `configure_from_report`
/// happened.
///
/// Deliberately **not** part of [`EnterDecision`]. The enum is the *decision*;
/// bolting strategy-specific diagnostics onto it would force every caller and
/// every test to carry fields they don't use. This is a separate, optional query,
/// asked only of entries that actually fired.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EntrySignal {
    /// Cumulative change over the dip window, in percent. Negative for a real
    /// dip. Zero for strategies that have no such concept.
    pub dip: f64,
    /// Cumulative change over the bounce window, in percent.
    pub bounce: f64,
}

/// Typed exit decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitDecision {
    /// Keep the position open.
    Hold,
    /// Close the position for the given reason.
    Exit(ExitReason),
}

/// Why a position was (or should be) closed.
///
/// Moved verbatim from `scheduler::models::trade` — serde representation
/// (unit variant names) and the snake_case display strings are unchanged, so
/// the console wire format and the `reports.reason` column are unaffected.
#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Clone)]
pub enum ExitReason {
    /// Loss exceeded the configured stoploss.
    Stoploss,
    /// Volume dropped below the configured floor (currently disabled).
    LowVolume,
    /// Change stayed flat for too long (currently disabled).
    LowChange,
    /// Change reached the sell floor while the token fell off the top list.
    FloorReached,
    /// Position ran out of time.
    Timeout,
    /// Gain reached the configured cashout target.
    Cashout,
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Stoploss => "stoploss",
            Self::LowVolume => "low_volume",
            Self::LowChange => "low_change",
            Self::FloorReached => "floor_reached",
            Self::Timeout => "timeout",
            Self::Cashout => "cashout",
        };
        f.write_str(s)
    }
}

impl FromStr for ExitReason {
    type Err = ();

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lower = input.to_lowercase();
        match lower.as_ref() {
            "stoploss" => Ok(Self::Stoploss),
            "low_volume" => Ok(Self::LowVolume),
            "low_change" => Ok(Self::LowChange),
            "floor_reached" => Ok(Self::FloorReached),
            "timeout" => Ok(Self::Timeout),
            "cashout" => Ok(Self::Cashout),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_reason_display_should_match_legacy_wire_strings() {
        let rendered: Vec<String> = [
            ExitReason::Stoploss,
            ExitReason::LowVolume,
            ExitReason::LowChange,
            ExitReason::FloorReached,
            ExitReason::Timeout,
            ExitReason::Cashout,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            rendered,
            [
                "stoploss",
                "low_volume",
                "low_change",
                "floor_reached",
                "timeout",
                "cashout"
            ]
        );
    }

    #[test]
    fn exit_reason_from_str_should_round_trip_display() {
        let parsed = ExitReason::from_str("floor_reached");
        assert_eq!(parsed, Ok(ExitReason::FloorReached));
    }

    #[test]
    fn exit_reason_serde_should_use_variant_names() {
        let json = serde_json::to_string(&ExitReason::Stoploss).expect("serializes");
        assert_eq!(json, "\"Stoploss\"");
    }
}
