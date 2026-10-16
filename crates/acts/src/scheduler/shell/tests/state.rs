use crate::LegacyTaskState;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_state_is_finished() {
    let state = LegacyTaskState::Ready;
    assert!(!state.is_completed());

    let state = LegacyTaskState::Running;
    assert!(!state.is_completed());

    let state = LegacyTaskState::None;
    assert!(!state.is_completed());

    let state = LegacyTaskState::Completed;
    assert!(state.is_completed());

    let state = LegacyTaskState::Cancelled;
    assert!(state.is_completed());

    let state = LegacyTaskState::Backed;
    assert!(state.is_completed());

    let state = LegacyTaskState::Submitted;
    assert!(state.is_completed());

    let state = LegacyTaskState::Skipped;
    assert!(state.is_completed());

    let state = LegacyTaskState::Removed;
    assert!(state.is_completed());

    let state = LegacyTaskState::Error;
    assert!(state.is_completed());

    let state = LegacyTaskState::Aborted;
    assert!(state.is_completed());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_state_is_error() {
    let state = LegacyTaskState::Ready;
    assert!(!state.is_error());

    let state = LegacyTaskState::Running;
    assert!(!state.is_error());

    let state = LegacyTaskState::None;
    assert!(!state.is_error());

    let state = LegacyTaskState::Completed;
    assert!(!state.is_error());

    let state = LegacyTaskState::Submitted;
    assert!(!state.is_error());

    let state = LegacyTaskState::Cancelled;
    assert!(!state.is_error());

    let state = LegacyTaskState::Backed;
    assert!(!state.is_error());

    let state = LegacyTaskState::Skipped;
    assert!(!state.is_error());

    let state = LegacyTaskState::Error;
    assert!(state.is_error());

    let state = LegacyTaskState::Aborted;
    assert!(!state.is_error());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_state_to_string() {
    let state = LegacyTaskState::None;
    assert_eq!(state.to_string(), "none");

    let state = LegacyTaskState::Ready;
    assert_eq!(state.to_string(), "ready");

    let state = LegacyTaskState::Running;
    assert_eq!(state.to_string(), "running");

    let state = LegacyTaskState::Error;
    assert_eq!(state.to_string(), "error");

    let state = LegacyTaskState::Interrupt;
    assert_eq!(state.to_string(), "interrupted");

    let state = LegacyTaskState::Submitted;
    assert_eq!(state.to_string(), "submitted");

    let state = LegacyTaskState::Cancelled;
    assert_eq!(state.to_string(), "cancelled");

    let state = LegacyTaskState::Backed;
    assert_eq!(state.to_string(), "backed");

    let state = LegacyTaskState::Pending;
    assert_eq!(state.to_string(), "pending");

    let state = LegacyTaskState::Aborted;
    assert_eq!(state.to_string(), "aborted");

    let state = LegacyTaskState::Removed;
    assert_eq!(state.to_string(), "removed");

    let state = LegacyTaskState::Skipped;
    assert_eq!(state.to_string(), "skipped");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_state_from_string() {
    let state: LegacyTaskState = "none".into();
    assert_eq!(state, LegacyTaskState::None);

    let state: LegacyTaskState = "ready".into();
    assert_eq!(state, LegacyTaskState::Ready);

    let state: LegacyTaskState = "running".into();
    assert_eq!(state, LegacyTaskState::Running);

    let state: LegacyTaskState = "error".into();
    assert_eq!(state, LegacyTaskState::Error);

    let state: LegacyTaskState = "aborted".into();
    assert_eq!(state, LegacyTaskState::Aborted);

    let state: LegacyTaskState = "submitted".into();
    assert_eq!(state, LegacyTaskState::Submitted);

    let state: LegacyTaskState = "cancelled".into();
    assert_eq!(state, LegacyTaskState::Cancelled);

    let state: LegacyTaskState = "backed".into();
    assert_eq!(state, LegacyTaskState::Backed);

    let state: LegacyTaskState = "interrupted".into();
    assert_eq!(state, LegacyTaskState::Interrupt);

    let state: LegacyTaskState = "pending".into();
    assert_eq!(state, LegacyTaskState::Pending);

    let state: LegacyTaskState = "skipped".into();
    assert_eq!(state, LegacyTaskState::Skipped);

    let state: LegacyTaskState = "removed".into();
    assert_eq!(state, LegacyTaskState::Removed);
}
