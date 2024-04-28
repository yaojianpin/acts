use crate::TaskState;

#[tokio::test]
async fn sch_state_is_finished() {
    let state = TaskState::Running;
    assert!(!state.is_completed());

    let state = TaskState::None;
    assert!(!state.is_completed());

    let state = TaskState::Completed;
    assert!(state.is_completed());

    let state = TaskState::Skipped;
    assert!(state.is_completed());

    let state = TaskState::Error;
    assert!(state.is_completed());

    let state = TaskState::Aborted;
    assert!(state.is_completed());
}

#[tokio::test]
async fn sch_state_is_error() {
    let state = TaskState::Running;
    assert!(!state.is_error());

    let state = TaskState::None;
    assert!(!state.is_error());

    let state = TaskState::Completed;
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
    let state = TaskState::Running;
    assert_eq!(state.to_string(), "running");

    let state = TaskState::Error;
    assert_eq!(state.to_string(), "fail");
}

#[tokio::test]
async fn sch_state_from_string() {
    let str = "running";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Running);

    let str = "fail";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Error);

    let str = "abort";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Aborted);
}
