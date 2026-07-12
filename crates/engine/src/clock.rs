//! The `Clock` port: the single sanctioned source of time.
//!
//! Everything downstream of the scheduler's decision loop receives time
//! either as a `&dyn Clock` (at the edges) or as plain `DateTime<Utc>`
//! values (inside pure logic). Ambient time calls (`Utc::now()`,
//! `Instant::now()`) are denied via `clippy::disallowed_methods` so they
//! cannot sneak back in; `LiveClock` below holds the only exemptions.

use std::{
    sync::Mutex,
    time::{Duration as StdDuration, Instant},
};

use chrono::{DateTime, Utc};

/// Source of wall-clock and monotonic time.
///
/// Object safe on purpose: the scheduler holds an `Arc<dyn Clock>` so tests
/// and (later) backtests can substitute a [`TestClock`]. The virtual call is
/// irrelevant at scheduler decision cadence; switch to generics only if a
/// profile ever says otherwise.
pub trait Clock: std::fmt::Debug + Send + Sync {
    /// Current wall-clock time in UTC.
    fn now_utc(&self) -> DateTime<Utc>;

    /// Monotonic time elapsed since an arbitrary fixed origin (e.g. clock
    /// creation). Replaces `Instant::now()` + `Instant::elapsed()` pairs:
    /// callers measure spans as `clock.monotonic() - earlier_reading`.
    fn monotonic(&self) -> StdDuration;
}

/// Production clock backed by the operating system.
#[derive(Debug)]
pub struct LiveClock {
    origin: Instant,
}

impl Default for LiveClock {
    fn default() -> Self {
        Self {
            // The one place in the workspace allowed to read ambient time.
            #[expect(
                clippy::disallowed_methods,
                reason = "LiveClock is the sanctioned adapter over ambient time"
            )]
            origin: Instant::now(),
        }
    }
}

impl Clock for LiveClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "LiveClock is the sanctioned adapter over ambient time"
    )]
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn monotonic(&self) -> StdDuration {
        self.origin.elapsed()
    }
}

/// Deterministic clock for tests and (later) backtests.
///
/// Time only moves when [`TestClock::advance`] or [`TestClock::set`] is
/// called, making decision logic fully reproducible.
#[derive(Debug)]
pub struct TestClock {
    state: Mutex<TestClockState>,
}

#[derive(Debug)]
struct TestClockState {
    now: DateTime<Utc>,
    mono: StdDuration,
}

impl TestClock {
    /// Creates a clock frozen at `start` with a monotonic origin of zero.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            state: Mutex::new(TestClockState {
                now: start,
                mono: StdDuration::ZERO,
            }),
        }
    }

    /// Moves both wall-clock and monotonic time forward by `delta`.
    pub fn advance(&self, delta: StdDuration) {
        let mut state = self.state.lock().expect("TestClock mutex poisoned");
        state.now += chrono::Duration::from_std(delta)
            .expect("TestClock advance delta out of chrono range");
        state.mono += delta;
    }

    /// Sets wall-clock time without touching monotonic time.
    pub fn set(&self, now: DateTime<Utc>) {
        self.state.lock().expect("TestClock mutex poisoned").now = now;
    }
}

impl Clock for TestClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.state.lock().expect("TestClock mutex poisoned").now
    }

    fn monotonic(&self) -> StdDuration {
        self.state.lock().expect("TestClock mutex poisoned").mono
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn test_clock_now_should_return_start_time_before_any_advance() {
        let clock = TestClock::new(start());
        assert_eq!(clock.now_utc(), start());
    }

    #[test]
    fn test_clock_advance_should_move_wall_clock_forward() {
        let clock = TestClock::new(start());
        clock.advance(StdDuration::from_secs(90));
        assert_eq!(clock.now_utc(), start() + chrono::Duration::seconds(90));
    }

    #[test]
    fn test_clock_advance_should_move_monotonic_time_forward() {
        let clock = TestClock::new(start());
        clock.advance(StdDuration::from_millis(1500));
        assert_eq!(clock.monotonic(), StdDuration::from_millis(1500));
    }

    #[test]
    fn test_clock_set_should_not_move_monotonic_time() {
        let clock = TestClock::new(start());
        clock.set(start() + chrono::Duration::days(1));
        assert_eq!(clock.monotonic(), StdDuration::ZERO);
    }

    #[test]
    fn live_clock_monotonic_should_never_decrease() {
        let clock = LiveClock::default();
        let first = clock.monotonic();
        assert!(clock.monotonic() >= first);
    }
}
