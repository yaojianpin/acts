use crate::{ActError, TaskState};

#[tokio::test]
async fn sch_state_is_finished() {
    let state = TaskState::Running;
    assert!(!state.is_completed());

    let state = TaskState::None;
    assert!(!state.is_completed());

    let state = TaskState::Success;
    assert!(state.is_completed());

    let state = TaskState::Skip;
    assert!(state.is_completed());

    let state = TaskState::Fail(ActError::Runtime("test error".into()).into());
    assert!(state.is_completed());

    let state = TaskState::Abort;
    assert!(state.is_completed());
}

#[tokio::test]
async fn sch_state_is_error() {
    let state = TaskState::Running;
    assert!(!state.is_error());

    let state = TaskState::None;
    assert!(!state.is_error());

    let state = TaskState::Success;
    assert!(!state.is_error());

    let state = TaskState::Skip;
    assert!(!state.is_error());

    let state = TaskState::Fail(ActError::Runtime("test error".into()).into());
    assert!(state.is_error());

    let state = TaskState::Abort;
    assert!(!state.is_error());
}

#[tokio::test]
async fn sch_state_to_string() {
    let state = TaskState::Running;
    assert_eq!(state.to_string(), "running");

    let state = TaskState::Fail("err info".to_string());
    assert_eq!(state.to_string(), "fail(err info)");
}

#[tokio::test]
async fn sch_state_from_string() {
    let str = "running";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Running);

    let str = "fail(err info)";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Fail("err info".to_string()));

    let str = "abort";
    let state: TaskState = str.into();
    assert_eq!(state, TaskState::Abort);
}
