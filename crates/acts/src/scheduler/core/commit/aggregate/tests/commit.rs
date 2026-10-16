use super::*;

#[test]
fn commit_is_atomic_across_state_vars_operations_and_outbox() {
    let mut repo = repository();
    let receipt = repo.compare_and_commit(&plan()).unwrap();
    let snapshot = repo.load("p1").unwrap().unwrap();
    assert_eq!(snapshot.tasks[&id()].state, TaskState::Completed);
    assert_eq!(snapshot.variables[&(id(), "result".into())], "42");
    assert_eq!(snapshot.operations.len(), 1);
    assert!(
        snapshot
            .outbox
            .keys()
            .any(|id| &id.key == "complete:p1:t1:0")
    );
    assert_eq!(
        receipt.current,
        ProcessVersion {
            graph: 0,
            process: 1,
            operations: 1
        }
    );
}

#[test]
fn conflict_changes_nothing() {
    let mut repo = repository();
    let mut invalid = plan();
    invalid.state_changes[0].expected_revision = 1;
    assert!(matches!(
        repo.compare_and_commit(&invalid),
        Err(CommitError::TaskConflict { .. })
    ));
    assert_eq!(
        repo.load("p1").unwrap().unwrap(),
        repository().load("p1").unwrap().unwrap()
    );
}

#[test]
fn retrying_same_plan_is_rejected_without_duplicate_effects() {
    let mut repo = repository();
    repo.compare_and_commit(&plan()).unwrap();
    assert!(matches!(
        repo.compare_and_commit(&plan()),
        Err(CommitError::VersionConflict { .. })
    ));
    let snapshot = repo.load("p1").unwrap().unwrap();
    assert_eq!(snapshot.operations.len(), 1);
    assert_eq!(snapshot.outbox.len(), 1);
}
#[test]
fn source_operation_is_committed_and_closed_in_same_atomic_batch() {
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::operation::OperationEvent;
    use crate::scheduler::core::model::transition_plan::OperationCommit;
    let claim = ClaimToken {
        owner: "runtime-a".into(),
        fence: 9,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    let source_id = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
        sequence: 0,
    };
    let mut repo = repository();
    let mut snapshot = repo.load("p1").unwrap().unwrap();
    let prepared = OperationRecord::pending(source_id.clone(), OperationKind::ApplyAction)
        .evolve(OperationEvent::Claim(claim.clone()))
        .unwrap()
        .evolve(OperationEvent::PrepareCommit {
            claim: claim.clone(),
        })
        .unwrap();
    snapshot.operations.insert(source_id.clone(), prepared);
    repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();

    let mut p = plan();
    p.source_operation = Some(OperationCommit {
        id: source_id.clone(),
        claim,
    });
    let receipt = repo.compare_and_commit(&p).unwrap();
    let snapshot = repo.load("p1").unwrap().unwrap();
    // The operation's own commit and its close are one transition of the
    // batch: the hop leaves `Completed`, never an intermediate `Committed`
    // (nothing would observe it, and a second commit for it bought a second
    // whole-aggregate round trip).
    assert_eq!(
        snapshot.operations[&source_id].state,
        OperationState::Completed(receipt.token)
    );
    assert_eq!(snapshot.tasks[&id()].state, TaskState::Completed);
}

#[test]
fn stale_source_claim_rejects_whole_batch() {
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::operation::OperationEvent;
    use crate::scheduler::core::model::transition_plan::OperationCommit;
    let active = ClaimToken {
        owner: "runtime-a".into(),
        fence: 9,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    let stale = ClaimToken {
        owner: "runtime-b".into(),
        fence: 8,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    let source_id = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
        sequence: 0,
    };
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let prepared = OperationRecord::pending(source_id.clone(), OperationKind::ApplyAction)
        .evolve(OperationEvent::Claim(active.clone()))
        .unwrap()
        .evolve(OperationEvent::PrepareCommit { claim: active })
        .unwrap();
    snapshot.operations.insert(source_id.clone(), prepared);
    let before = snapshot.clone();
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();
    let mut p = plan();
    p.source_operation = Some(OperationCommit {
        id: source_id,
        claim: stale,
    });
    assert!(matches!(
        repo.compare_and_commit(&p),
        Err(CommitError::InvalidOperationState(_))
    ));
    assert_eq!(repo.load("p1").unwrap().unwrap(), before);
}

#[test]
fn durable_claim_prepare_commit_has_one_revision_chain() {
    use crate::scheduler::core::model::operation::ClaimToken;
    let id = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
        sequence: 0,
    };
    let claim = ClaimToken {
        owner: "runtime-a".into(),
        fence: 3,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    snapshot.operations.insert(
        id.clone(),
        OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
    );
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();

    let claimed = repo.claim_operation(&id, 0, claim.clone()).unwrap();
    let prepared = repo
        .prepare_operation(&id, claimed.revision, claim.clone())
        .unwrap();
    assert_eq!(
        prepared.state,
        OperationState::CommitPrepared(claim.clone())
    );

    let mut p = plan();
    p.expected = repo.load("p1").unwrap().unwrap().version;
    p.source_operation = Some(
        crate::scheduler::core::model::transition_plan::OperationCommit {
            id: id.clone(),
            claim,
        },
    );
    let receipt = repo.compare_and_commit(&p).unwrap();
    let committed = repo.load("p1").unwrap().unwrap().operations[&id].clone();
    // The effect commit carries the close: the operation reaches `Completed`
    // on the commit's own token, one revision past `CommitPrepared`.
    assert_eq!(
        committed.state,
        OperationState::Completed(receipt.token.clone()),
        "the commit closes its own source operation"
    );
    // The commit applies both lifecycle steps in the one batch — `Commit`
    // then `Complete` — so the revision advances by two, exactly as it did
    // across the two commits the fold replaces.
    assert_eq!(committed.revision, prepared.revision + 2);
    // A second close is refused: the effect commit already closed the
    // operation, so nothing may close it again.
    assert!(
        repo.complete_operation(&id, committed.revision, receipt.token)
            .is_err()
    );
}

#[test]
fn operation_revision_conflict_does_not_advance_state() {
    use crate::scheduler::core::model::operation::ClaimToken;
    let id = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
        sequence: 0,
    };
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    snapshot.operations.insert(
        id.clone(),
        OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
    );
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();
    let error = repo
        .claim_operation(
            &id,
            9,
            ClaimToken {
                owner: "r".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        CommitError::OperationRevisionConflict { .. }
    ));
    assert_eq!(
        repo.load("p1").unwrap().unwrap().operations[&id].state,
        OperationState::Pending
    );
}

#[test]
fn task_creation_advances_graph_and_process_atomically() {
    use crate::scheduler::core::model::transition_plan::TaskCreate;
    let mut repo = repository();
    let mut p = plan();
    p.task_creates.push(TaskCreate {
        task: TaskId::new("t1", 1).unwrap(),
        model_node: NodeId::parse("t1").unwrap(),
        parent: None,
        predecessors: vec![id()],
        initial_state: TaskState::Ready,
    });
    let receipt = repo.compare_and_commit(&p).unwrap();
    assert_eq!(receipt.current.graph, 1);
    assert_eq!(receipt.current.process, 1);
    let snapshot = repo.load("p1").unwrap().unwrap();
    assert_eq!(
        snapshot.tasks[&TaskId::new("t1", 1).unwrap()].state,
        TaskState::Ready
    );
}

#[test]
fn generation_must_move_forward_without_overwriting_history() {
    use crate::scheduler::core::model::transition_plan::TaskCreate;
    let mut repo = repository();
    let mut p = plan();
    p.task_creates.push(TaskCreate {
        task: id(),
        model_node: NodeId::parse("t1").unwrap(),
        parent: None,
        predecessors: vec![],
        initial_state: TaskState::Ready,
    });
    assert!(matches!(
        repo.compare_and_commit(&p),
        Err(CommitError::TaskAlreadyExists(_))
    ));
    assert_eq!(repo.load("p1").unwrap().unwrap().tasks.len(), 1);
}

#[test]
fn repository_heartbeat_uses_operation_revision_cas() {
    let id = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
        sequence: 0,
    };
    let current = ClaimToken {
        owner: "runtime-a".into(),
        fence: 4,
        lease_until_ms: 100,
        heartbeat_revision: 0,
    };
    let renewed = ClaimToken {
        owner: current.owner.clone(),
        fence: current.fence,
        lease_until_ms: 200,
        heartbeat_revision: 1,
    };
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    snapshot.operations.insert(
        id.clone(),
        OperationRecord::pending(id.clone(), OperationKind::ExecuteTask),
    );
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();
    let claimed = repo.claim_operation(&id, 0, current.clone()).unwrap();
    let heartbeat = repo
        .renew_operation(&id, claimed.revision, current.clone(), renewed.clone())
        .unwrap();
    assert_eq!(heartbeat.state, OperationState::Claimed(renewed));
    assert!(matches!(
        repo.renew_operation(&id, claimed.revision, current.clone(), current),
        Err(CommitError::OperationRevisionConflict { .. })
    ));
}
