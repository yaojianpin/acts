use super::*;

#[test]
fn transition_advances_the_snapshot_it_is_given() {
    let repo = repository();
    let current = repo.load("p1").unwrap().unwrap();
    let before = current.clone();
    let mut next = current;
    let receipt = advance_transition(&mut next, &plan()).unwrap();
    assert_ne!(next, before);
    assert_eq!(receipt.previous, before.version);
    assert_eq!(receipt.current, next.version);
}

fn pending_operation_snapshot() -> (ProcessSnapshot, OperationId, ClaimToken) {
    let claim = ClaimToken {
        owner: "runtime".into(),
        fence: 1,
        lease_until_ms: 100,
        heartbeat_revision: 0,
    };
    let mut current = repository().load("p1").unwrap().unwrap();
    let operation_id = OperationId {
        process_id: "p1".into(),
        task: TaskId::new("t1", 0).unwrap(),
        sequence: 9,
    };
    current.operations.insert(
        operation_id.clone(),
        OperationRecord::pending(operation_id.clone(), OperationKind::ExecuteTask),
    );
    (current, operation_id, claim)
}

#[test]
fn operation_event_advances_the_snapshot_it_is_given() {
    let (current, operation_id, claim) = pending_operation_snapshot();
    let before = current.clone();
    let mut next = current;
    let operation = advance_operation_event(
        &mut next,
        &operation_id,
        0,
        OperationEvent::Claim(claim.clone()),
    )
    .unwrap();
    assert_eq!(operation.state, OperationState::Claimed(claim));
    assert_eq!(operation.revision, 1);
    assert_eq!(next.version.operations, before.version.operations + 1);
    assert_eq!(next.operations[&operation_id], operation);
}

#[test]
fn rejected_operation_event_leaves_its_snapshot_untouched() {
    // The reachable rejection runs before the first mutation, which is
    // what lets the durable repository hand its own snapshot over: an operation
    // that loses the revision race must leave the aggregate as it was.
    let (current, operation_id, claim) = pending_operation_snapshot();
    let mut next = current.clone();
    let outcome =
        advance_operation_event(&mut next, &operation_id, 7, OperationEvent::Claim(claim));
    assert!(outcome.is_err());
    assert_eq!(next, current);
}
