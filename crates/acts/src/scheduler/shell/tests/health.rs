//! A store that stays down must degrade the loops that poll it.
//!
//! Both store-facing timers treat one failed tick as survivable: the error is
//! logged and the loop goes on. That is right for a transient fault and wrong
//! for a persistent one — the loop keeps querying a store that cannot answer,
//! logs one error per tick, and in every state it reports looks exactly like an
//! idle scheduler. `LoopGuard` gives each loop a failure streak, a backoff
//! window and a state an owner can read; the case here pins that state machine.
//! (The trigger timer's use of it under a store outage lives in
//! [`crate::scheduler::tests::reliability::timer`].)

use crate::{
    ActError,
    scheduler::shell::health::{DEGRADE_AFTER, HealthState, LoopGuard, MAX_BACKOFF_TICKS},
};

/// The guard's contract: nothing changes below the threshold, the streak that
/// crosses it degrades the loop and makes it skip the next tick, the window
/// doubles to a cap and no further, and one clean tick clears all of it.
#[test]
fn guard_degrades_widens_to_a_cap_and_recovers() {
    let guard = LoopGuard::new("test");
    let err = ActError::Store("backend unavailable".to_string());

    // a streak below the threshold is only counted: the loop keeps its cadence
    // and is still reported healthy (one transient failure is not an incident)
    for _ in 0..DEGRADE_AFTER - 1 {
        assert!(guard.attempt(), "a healthy loop attempts every tick");
        guard.failed(&err);
    }
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Healthy);
    assert_eq!(health.consecutive_failures, DEGRADE_AFTER - 1);
    assert_eq!(health.backoff_ticks, 0);

    // the failure that crosses the threshold degrades the loop and skips
    // exactly one tick: the first window must not already be the cap, or a
    // store that comes back would not be noticed until the cap had elapsed
    assert!(guard.attempt());
    guard.failed(&err);
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Degraded);
    assert_eq!(health.consecutive_failures, DEGRADE_AFTER);
    assert_eq!(health.backoff_ticks, 1);
    assert!(!guard.attempt(), "a degraded loop skips its next tick");
    assert!(guard.attempt(), "and attempts the one after it");

    // the window grows with the streak, up to the cap
    let mut widest = 0;
    for _ in 0..MAX_BACKOFF_TICKS + 2 {
        // sit out the window the previous failure set...
        for _ in 0..guard.snapshot().backoff_ticks {
            assert!(!guard.attempt(), "the loop skips its whole window");
        }
        // ...then attempt and fail again, which is what widens it
        assert!(guard.attempt());
        guard.failed(&err);
        widest = widest.max(guard.snapshot().backoff_ticks);
    }
    assert_eq!(
        widest, MAX_BACKOFF_TICKS,
        "the window must reach the cap, and stop there"
    );

    // one clean tick clears the streak and the window, so the loop is back to
    // attempting every tick
    for _ in 0..guard.snapshot().backoff_ticks {
        assert!(!guard.attempt(), "the loop skips its whole window");
    }
    assert!(guard.attempt());
    guard.recovered();
    let health = guard.snapshot();
    assert_eq!(health.state, HealthState::Healthy);
    assert_eq!(health.consecutive_failures, 0);
    assert_eq!(health.backoff_ticks, 0);
    assert!(
        guard.attempt(),
        "a recovered loop attempts every tick again"
    );
}

/// The state spellings are part of the log contract; both arms are read.
#[test]
fn health_state_names_are_stable() {
    assert_eq!(HealthState::Healthy.as_str(), "healthy");
    assert_eq!(HealthState::Degraded.as_str(), "degraded");
}

/// A clean tick below the threshold only clears a streak: it is a debug
/// event, not the recovery of a degraded loop, and the loop attempts again
/// immediately either way.
#[test]
fn a_short_streak_recovers_quietly() {
    let guard = LoopGuard::new("test-short");
    let err = ActError::Store("flaky".to_string());
    assert!(guard.attempt());
    guard.failed(&err);
    assert_eq!(guard.snapshot().consecutive_failures, 1);
    assert!(guard.attempt(), "below the threshold nothing is skipped");
    guard.recovered();
    assert_eq!(
        guard.snapshot(),
        crate::scheduler::shell::health::LoopHealth {
            state: HealthState::Healthy,
            consecutive_failures: 0,
            backoff_ticks: 0,
        }
    );

    // A clean tick with no failure at all is a no-op.
    guard.recovered();
    assert_eq!(guard.snapshot().consecutive_failures, 0);
}

/// A failure inside an unchanged backoff window does not re-log, and the
/// window still caps: repeated failures at the cap keep the same skip count.
#[test]
fn a_repeated_failure_inside_the_cap_does_not_widen_the_window() {
    let guard = LoopGuard::new("test-cap");
    let err = ActError::Store("down".to_string());
    for _ in 0..64 {
        guard.failed(&err);
    }
    let capped = guard.snapshot();
    assert_eq!(capped.state, HealthState::Degraded);
    assert_eq!(capped.backoff_ticks, MAX_BACKOFF_TICKS);
    // Every one of the capped ticks is skipped, then the loop attempts again.
    for _ in 0..MAX_BACKOFF_TICKS {
        assert!(!guard.attempt());
    }
    assert!(guard.attempt());
}
