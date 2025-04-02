use crate::TaskState;

#[tokio::test]
async fn sch_state_is_finished() {
    let state = TaskState::Ready;
    assert!(!state.is_completed());

    let state = TaskState::Running;
    assert!(!state.is_completed());

    let state = TaskState::None;
    assert!(!state.is_completed());

    let state = TaskState::Completed;
    assert!(state.is_completed());

    let state = TaskState::Cancelled;
    assert!(state.is_completed());

    let state = TaskState::Backed;
    assert!(state.is_completed());

    let state = TaskState::Submitted;
    assert!(state.is_completed());

    let state = TaskState::Skipped;
    assert!(state.is_completed());

    let state = TaskState::Removed;
    assert!(state.is_completed());

    let state = TaskState::Error;
    assert!(state.is_completed());

    let state = TaskState::Aborted;
    assert!(state.is_completed());
}

#[tokio::test]
async fn sch_state_is_error() {
    let state = TaskState::Ready;
    assert!(!state.is_error());

    let state = TaskState::Running;
    assert!(!state.is_error());

    let state = TaskState::None;
    assert!(!state.is_error());

    let state = TaskState::Completed;
    assert!(!state.is_error());

    let state = TaskState::Submitted;
    assert!(!state.is_error());

    let state = TaskState::Cancelled;
    assert!(!state.is_error());

    let state = TaskState::Backed;
    assert!(!state.is_error());

    let state = TaskState::Skipped;
    assert!(!state.is_error());

    let state = TaskState::Error;
    assert!(state.is_error());

    let state = TaskState::Aborted;
    assert!(!state.is_error());
}

#[tokio::test]
async fn sch_state_to_string() {
    let state = TaskState::None;
    assert_eq!(state.to_string(), "none");

    let state = TaskState::Ready;
    assert_eq!(state.to_string(), "ready");

    let state = TaskState::Running;
    assert_eq!(state.to_string(), "running");

    let state = TaskState::Error;
    assert_eq!(state.to_string(), "error");

    let state = TaskState::Interrupt;
    assert_eq!(state.to_string(), "interrupted");

    let state = TaskState::Submitted;
    assert_eq!(state.to_string(), "submitted");

    let state = TaskState::Cancelled;
    assert_eq!(state.to_string(), "cancelled");

    let state = TaskState::Backed;
    assert_eq!(state.to_string(), "backed");

    let state = TaskState::Pending;
    assert_eq!(state.to_string(), "pending");

    let state = TaskState::Aborted;
    assert_eq!(state.to_string(), "aborted");

    let state = TaskState::Removed;
    assert_eq!(state.to_string(), "removed");

    let state = TaskState::Skipped;
    assert_eq!(state.to_string(), "skipped");
}

#[tokio::test]
async fn sch_state_from_string() {
    let state: TaskState = "none".into();
    assert_eq!(state, TaskState::None);

    let state: TaskState = "ready".into();
    assert_eq!(state, TaskState::Ready);

    let state: TaskState = "running".into();
    assert_eq!(state, TaskState::Running);

    let state: TaskState = "error".into();
    assert_eq!(state, TaskState::Error);

    let state: TaskState = "aborted".into();
    assert_eq!(state, TaskState::Aborted);

    let state: TaskState = "submitted".into();
    assert_eq!(state, TaskState::Submitted);

    let state: TaskState = "cancelled".into();
    assert_eq!(state, TaskState::Cancelled);

    let state: TaskState = "backed".into();
    assert_eq!(state, TaskState::Backed);

    let state: TaskState = "interrupted".into();
    assert_eq!(state, TaskState::Interrupt);

    let state: TaskState = "pending".into();
    assert_eq!(state, TaskState::Pending);

    let state: TaskState = "skipped".into();
    assert_eq!(state, TaskState::Skipped);

    let state: TaskState = "removed".into();
    assert_eq!(state, TaskState::Removed);
}
