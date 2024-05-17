use crate::{ActError, ActionResult};

#[test]
fn model_action_result_begin() {
    let state = ActionResult::begin();
    assert!(state.start_time > 0)
}

#[test]
fn model_action_result_end() {
    let state = ActionResult::begin();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let result = state.end();
    assert!(result.unwrap().cost() > 0)
}

#[test]
fn model_action_result_end_with_result_ok() {
    let state = ActionResult::begin();
    let result = state.end_with_result(Ok(5));
    assert!(result.is_ok());
    assert_eq!(result.unwrap().outputs().get::<i32>("data").unwrap(), 5);
}

#[test]
fn model_action_result_end_with_result_err() {
    let state = ActionResult::begin();
    let result = state.end_with_result::<()>(Err(ActError::Action("err1".to_string())));
    assert!(result.is_err());
    assert_eq!(result.err().unwrap().to_string(), "err1");
}

#[test]
fn model_action_result_end_with_data() {
    let state = ActionResult::begin();
    let result = state.end_with_data("a", "abc");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().outputs().get::<String>("a").unwrap(), "abc");
}
