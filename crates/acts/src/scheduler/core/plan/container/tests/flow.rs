use super::*;

#[test]
fn the_flow_fact_is_one_axis_with_two_values() {
    let mut snapshot = snapshot(vec![task("step1", 0, TaskState::Running)]);
    let step1 = id("step1", 0);
    assert_eq!(task_flow(&snapshot, &step1), None);
    assert!(!parked_for_children(&snapshot, &step1));
    assert!(!deciding_catches(&snapshot, &step1));

    snapshot.variables.insert(
        (step1.clone(), FLOW_VAR.to_string()),
        FLOW_CHILDREN.to_string(),
    );
    assert_eq!(task_flow(&snapshot, &step1), Some(TaskFlow::Children));
    assert!(parked_for_children(&snapshot, &step1));
    assert!(!deciding_catches(&snapshot, &step1));

    snapshot.variables.insert(
        (step1.clone(), FLOW_VAR.to_string()),
        FLOW_CATCHES.to_string(),
    );
    assert_eq!(task_flow(&snapshot, &step1), Some(TaskFlow::Catches));
    assert!(!parked_for_children(&snapshot, &step1));
    assert!(deciding_catches(&snapshot, &step1));

    // An unknown value is "no flow" rather than a crash: only the two
    // values this module writes are meaningful.
    snapshot
        .variables
        .insert((step1.clone(), FLOW_VAR.to_string()), "?".to_string());
    assert_eq!(task_flow(&snapshot, &step1), None);
}
