//! Runtime abstractions ("ports") for the trading engine.
//!
//! This crate decouples *decision* logic from *execution*:
//!
//! - [`Clock`] — the single sanctioned source of time. `LiveClock` in
//!   production, `TestClock` for deterministic tests and future backtests.
//! - [`Strategy`] — pure entry/exit decisions over read-only view types
//!   ([`TokenView`], [`PositionView`], [`PortfolioView`]). The scheduler owns
//!   execution; strategies only decide.
//! - [`ThresholdStrategy`] — a behavior-identical port of the scheduler's
//!   original inline threshold logic.
//!
//! Mental model: ports & adapters (hexagonal) *inside* one process — this is
//! about dependency direction, not process boundaries. The trait shapes here
//! become the wire protocol when strategy runners split out over WebSocket
//! in a later refactor. Out of scope for now: `Executor`, event bus,
//! backtest harness.

#![deny(missing_docs)]

pub mod clock;
pub mod common_checks;
pub mod reversion;
pub mod strategy;
pub mod threshold;
pub mod views;

pub use clock::{Clock, LiveClock, TestClock};
pub use reversion::{ReversionStrategy, ReversionThresholds};
pub use strategy::{Context, EnterDecision, EntrySignal, ExitDecision, ExitReason, Strategy};
pub use threshold::{Thresholds, ThresholdStrategy};
pub use views::{Candle, PortfolioView, PositionView, TokenView};

/// The existing threshold bag from `lib`, renamed in engine context to free
/// the `Strategy` name for the trait. Stays defined in `exchange-observer`
/// (shared data models + config); `engine` only aliases it.
pub use exchange_observer::Strategy as StrategyConfig;
