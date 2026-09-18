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
    scheduler::health::{DEGRADE_AFTER, HealthState, LoopGuard, MAX_BACKOFF_TICKS},
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
