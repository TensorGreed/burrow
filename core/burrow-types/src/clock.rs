//! An injected clock, and the deadline built on it.
//!
//! Time is a dependency, not an ambient fact. Operations take a [`Clock`] and consult it
//! at their checkpoints rather than calling `Instant::now()`, for two reasons recorded in
//! `docs/adr/0007-limit-enforcement-per-platform.md`:
//!
//! - **`Instant::now()` panics on `wasm32-unknown-unknown`.** The core forbids panics and
//!   the web is the first target, so a limit implemented with `Instant` would be a
//!   guaranteed crash in a browser worker — and on that target a panic is an
//!   uncatchable trap, so it would take the worker with it.
//! - **A timeout test that really waits is a test nobody runs.** [`ManualClock`] makes
//!   timeout behaviour a fast, deterministic unit test.
//!
//! Enforcement is **cooperative**: a deadline is only observed where something checks it,
//! so an operation can overshoot by as long as one engine call takes. That is stated in
//! [`Limits::max_duration_ms`](crate::Limits::max_duration_ms) too, and it is a real
//! limitation rather than an implementation detail to be fixed later.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{Limits, Result};

/// A monotonic source of milliseconds.
///
/// The epoch is unspecified and implementation-defined: only *differences* between two
/// readings are meaningful. Implementations must be monotonic and must never panic.
///
/// `Send + Sync` because operations hand a clock across the engine boundary, and on
/// native that boundary is another thread.
pub trait Clock: Send + Sync {
    /// Milliseconds elapsed since this clock's own, unspecified epoch.
    ///
    /// Must never panic. A clock that cannot advance should return a constant rather
    /// than fail; callers treat a non-advancing clock as "no time has passed", which is
    /// safe, if useless.
    fn now_ms(&self) -> u64;
}

/// The native clock, backed by [`std::time::Instant`].
///
/// Not available on wasm targets, deliberately: `Instant::now()` panics on
/// `wasm32-unknown-unknown`, so there must be no way to reach for it there by accident.
/// The web supplies its own clock over `performance.now()` through the binding layer.
#[cfg(not(target_family = "wasm"))]
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

#[cfg(not(target_family = "wasm"))]
impl SystemClock {
    /// Starts a clock whose epoch is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_family = "wasm"))]
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        // `Instant` is monotonic, so `elapsed` cannot be negative. The `u128` -> `u64`
        // conversion saturates rather than truncating: a process up for 584 million
        // years would otherwise wrap its deadline and report no time had passed.
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A clock that only moves when told to.
///
/// This is how timeout behaviour is tested — and fuzzed — without waiting. It is public
/// and always compiled, not `#[cfg(test)]`, because integration tests and the fuzz
/// targets live outside this crate and need it too.
#[derive(Debug, Default)]
pub struct ManualClock {
    now_ms: AtomicU64,
}

impl ManualClock {
    /// Starts a clock reading `start_ms`.
    #[must_use]
    pub const fn new(start_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(start_ms),
        }
    }

    /// Moves the clock forward by `ms`. Saturates rather than wrapping.
    pub fn advance(&self, ms: u64) {
        // A compare-exchange loop rather than `fetch_add`, because a wrap would move the
        // clock *backwards* and silently reset every live deadline.
        //
        // Written out rather than using `fetch_update`, which is deprecated on current
        // nightly (as `try_update`) and not yet renamed on the pinned stable toolchain.
        // The fuzz targets build on nightly and everything else on stable, so this has to
        // compile warning-free on both.
        let mut current = self.now_ms.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_add(ms);
            match self.now_ms.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// A time budget, captured once and checked repeatedly.
///
/// Constructed at the start of an operation and carried alongside whatever the operation
/// produces, so that later calls on the same document are still bounded by the budget the
/// caller originally asked for.
///
/// See the module docs: this is cooperative. A `Deadline` that has expired stops the
/// *next* checkpoint, not the call currently running inside an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    start_ms: u64,
    budget_ms: u64,
}

impl Deadline {
    /// Starts a budget of `limits.max_duration_ms`, reading `clock` once for the origin.
    #[must_use]
    pub fn start(clock: &dyn Clock, limits: &Limits) -> Self {
        Self {
            start_ms: clock.now_ms(),
            budget_ms: limits.max_duration_ms,
        }
    }

    /// Milliseconds elapsed since [`Deadline::start`], per `clock`.
    ///
    /// Saturating: a clock that goes backwards reports zero elapsed rather than
    /// underflowing.
    #[must_use]
    pub fn elapsed_ms(&self, clock: &dyn Clock) -> u64 {
        clock.now_ms().saturating_sub(self.start_ms)
    }

    /// Fails once the budget has been exceeded.
    ///
    /// Call this between engine calls and at page boundaries. It is the only thing that
    /// makes `max_duration_ms` mean anything.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LimitExceeded`](crate::Error::LimitExceeded) with
    /// `limit: "max_duration_ms"` once more than the budget has elapsed.
    pub fn checkpoint(&self, clock: &dyn Clock) -> Result<()> {
        Limits::check("max_duration_ms", self.elapsed_ms(clock), self.budget_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    fn limits_with_duration(ms: u64) -> Limits {
        Limits::with(|l| l.max_duration_ms = ms)
    }

    #[test]
    fn a_manual_clock_does_not_move_on_its_own() {
        let clock = ManualClock::new(0);
        assert_eq!(clock.now_ms(), 0);
        assert_eq!(clock.now_ms(), 0);
        clock.advance(5);
        assert_eq!(clock.now_ms(), 5);
    }

    #[test]
    fn advancing_saturates_instead_of_wrapping_backwards() {
        let clock = ManualClock::new(u64::MAX - 1);
        clock.advance(1000);
        assert_eq!(clock.now_ms(), u64::MAX);
    }

    #[test]
    fn a_deadline_permits_exactly_its_budget_and_fails_one_past_it() {
        let clock = ManualClock::new(1_000);
        let deadline = Deadline::start(&clock, &limits_with_duration(100));

        clock.advance(100);
        assert!(
            deadline.checkpoint(&clock).is_ok(),
            "the boundary is allowed"
        );

        clock.advance(1);
        match deadline.checkpoint(&clock) {
            Err(Error::LimitExceeded {
                limit,
                requested,
                allowed,
            }) => {
                assert_eq!(limit, "max_duration_ms");
                assert_eq!(requested, 101);
                assert_eq!(allowed, 100);
            }
            other => panic!("expected LimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn a_clock_that_goes_backwards_reports_zero_elapsed_rather_than_underflowing() {
        struct Backwards;
        impl Clock for Backwards {
            fn now_ms(&self) -> u64 {
                0
            }
        }
        let start = ManualClock::new(5_000);
        let deadline = Deadline::start(&start, &limits_with_duration(10));
        assert_eq!(deadline.elapsed_ms(&Backwards), 0);
        assert!(deadline.checkpoint(&Backwards).is_ok());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn the_system_clock_is_monotonic() {
        let clock = SystemClock::new();
        let a = clock.now_ms();
        let b = clock.now_ms();
        assert!(b >= a, "{b} < {a}");
    }

    #[test]
    fn clocks_are_send_and_sync_so_they_cross_the_engine_boundary() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ManualClock>();
        #[cfg(not(target_family = "wasm"))]
        assert_send_sync::<SystemClock>();
    }
}
