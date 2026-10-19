use super::*;

#[test]
fn process_rejects_operation_key_value_identity_mismatch() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let key = OperationId {
        process_id: "p1".into(),
        task: TaskId::new("t1", 0).unwrap(),
        sequence: 10,
    };
    let record_id = OperationId {
        sequence: 11,
        ..key.clone()
    };
    snapshot.operations.insert(
        key.clone(),
        OperationRecord::pending(record_id.clone(), OperationKind::ExecuteTask),
    );
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::OperationIdentityMismatch {
            key,
            record: record_id
        })
    );
}

#[test]
fn process_rejects_foreign_operation_and_dangling_variable_scope() {
    let mut foreign = repository().load("p1").unwrap().unwrap();
    let foreign_id = OperationId {
        process_id: "other".into(),
        task: TaskId::new("t1", 0).unwrap(),
        sequence: 1,
    };
    foreign.operations.insert(
        foreign_id.clone(),
        OperationRecord::pending(foreign_id.clone(), OperationKind::ExecuteTask),
    );
    assert_eq!(
        foreign.validate(),
        Err(CommitError::ForeignOperation(foreign_id))
    );

    let mut dangling = repository().load("p1").unwrap().unwrap();
    let missing = TaskId::new("missing", 0).unwrap();
    dangling
        .variables
        .insert((missing.clone(), "value".into()), "1".into());
    assert_eq!(dangling.validate(), Err(CommitError::TaskNotFound(missing)));
}

#[test]
fn recovery_pages_are_stable_non_overlapping_and_skip_terminal_rows() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let claim = ClaimToken {
        owner: "r".into(),
        fence: 1,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    for sequence in 0..5 {
        let id = OperationId {
            process_id: "p1".into(),
            task: TaskId::new("t1", 0).unwrap(),
            sequence,
        };
        let mut record = OperationRecord::pending(id.clone(), OperationKind::ExecuteTask);
        if sequence == 2 {
            let commit = CommitToken {
                process_version: 1,
                digest: "d".into(),
            };
            record = record
                .evolve(OperationEvent::Claim(claim.clone()))
                .unwrap()
                .evolve(OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                })
                .unwrap()
                .evolve(OperationEvent::Commit {
                    claim: claim.clone(),
                    commit: commit.clone(),
                })
                .unwrap()
                .evolve(OperationEvent::Complete { commit })
                .unwrap();
        }
        snapshot.operations.insert(id, record);
    }
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();
    let first = repo.recoverable_page(None, 2).unwrap();
    let second = repo.recoverable_page(first.next.as_ref(), 2).unwrap();
    assert_eq!(
        first
            .operations
            .iter()
            .map(|candidate| candidate.operation.id.sequence)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        second
            .operations
            .iter()
            .map(|candidate| candidate.operation.id.sequence)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(second.next.is_none());
    assert_eq!(
        repo.recoverable_page(None, 0),
        Err(CommitError::InvalidPageLimit)
    );
}

#[test]
fn empty_process_ids_are_refused_at_construction_and_validation() {
    assert_eq!(
        ProcessSnapshot::empty(" ").unwrap_err(),
        CommitError::EmptyProcessId
    );
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    snapshot.process_id = "".into();
    assert_eq!(snapshot.validate(), Err(CommitError::EmptyProcessId));
}

#[test]
fn process_rejects_outbox_identity_and_foreign_records() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let record_id = OutboxId {
        process_id: "p1".into(),
        key: "k".into(),
    };
    let record = OutboxRecord::pending(record_id.clone(), "topic".into(), "{}".into(), 0).unwrap();
    let key = OutboxId {
        key: "other".into(),
        ..record_id.clone()
    };
    snapshot.outbox.insert(key.clone(), record.clone());
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::OutboxIdentityMismatch {
            key,
            record: record_id
        })
    );

    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let foreign_id = OutboxId {
        process_id: "other".into(),
        key: "k".into(),
    };
    snapshot.outbox.insert(
        foreign_id.clone(),
        OutboxRecord::pending(foreign_id.clone(), "topic".into(), "{}".into(), 0).unwrap(),
    );
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::ForeignOutbox(foreign_id))
    );
}

#[test]
fn process_rejects_invalid_outbox_records() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let id = OutboxId {
        process_id: "p1".into(),
        key: "k".into(),
    };
    let mut record = OutboxRecord::pending(id.clone(), "topic".into(), "{}".into(), 0).unwrap();
    record.attempt = 1;
    record.last_fence = 0;
    snapshot.outbox.insert(id, record);
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::InvalidOutbox(
            crate::scheduler::core::outbox::records::OutboxError::CounterInvariant
        ))
    );
}

#[test]
fn process_rejects_broken_task_topology() {
    // A parent that is not in the aggregate.
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let child = TaskId::new("child", 0).unwrap();
    snapshot.tasks.insert(
        child.clone(),
        TaskSnapshot {
            model_node: NodeId::parse("child").unwrap(),
            parent: Some(TaskId::new("ghost", 0).unwrap()),
            predecessors: vec![],
            state: TaskState::Created,
            revision: 0,
        },
    );
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::TaskNotFound(TaskId::new("ghost", 0).unwrap()))
    );

    // A predecessor that is not in the aggregate.
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let child = TaskId::new("child", 0).unwrap();
    snapshot.tasks.insert(
        child.clone(),
        TaskSnapshot {
            model_node: NodeId::parse("child").unwrap(),
            parent: None,
            predecessors: vec![TaskId::new("ghost", 0).unwrap()],
            state: TaskState::Created,
            revision: 0,
        },
    );
    assert_eq!(
        snapshot.validate(),
        Err(CommitError::TaskNotFound(TaskId::new("ghost", 0).unwrap()))
    );

    // A task that names itself, and a duplicate edge, are invalid topology.
    for predecessors in [vec![child.clone()], vec![id(), id()]] {
        let mut snapshot = repository().load("p1").unwrap().unwrap();
        snapshot.tasks.insert(
            child.clone(),
            TaskSnapshot {
                model_node: NodeId::parse("child").unwrap(),
                parent: None,
                predecessors,
                state: TaskState::Created,
                revision: 0,
            },
        );
        assert!(matches!(
            snapshot.validate(),
            Err(CommitError::InvalidTaskTopology { .. })
        ));
    }
}

#[test]
fn process_rejects_an_empty_variable_name() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    snapshot.variables.insert((id(), "  ".into()), "v".into());
    assert_eq!(snapshot.validate(), Err(CommitError::EmptyVariableName));
}

#[test]
fn the_repository_refuses_a_duplicate_process() {
    let mut repo = repository();
    let snapshot = repo.load("p1").unwrap().unwrap();
    assert_eq!(
        repo.insert(snapshot),
        Err(CommitError::ProcessAlreadyExists("p1".into()))
    );
}

#[test]
fn recovery_cursor_filters_lexicographically_then_positionally() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    for sequence in 0..3 {
        let id = OperationId {
            process_id: "p1".into(),
            task: TaskId::new("t1", 0).unwrap(),
            sequence,
        };
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ExecuteTask),
        );
    }
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();

    // A cursor before the process keeps every row (the process-key branch).
    let before = RecoveryCursor {
        process_key: "a".into(),
        operation: None,
    };
    assert_eq!(
        repo.recoverable_page(Some(&before), 10)
            .unwrap()
            .operations
            .len(),
        3
    );

    // A cursor inside the process with no positional part keeps every row.
    let process_only = RecoveryCursor {
        process_key: "p1".into(),
        operation: None,
    };
    assert_eq!(
        repo.recoverable_page(Some(&process_only), 10)
            .unwrap()
            .operations
            .len(),
        3
    );

    // A positional cursor resumes strictly after its operation.
    let first = OperationId {
        process_id: "p1".into(),
        task: TaskId::new("t1", 0).unwrap(),
        sequence: 0,
    };
    let after = RecoveryCursor {
        process_key: "p1".into(),
        operation: Some(first.clone()),
    };
    let page = repo.recoverable_page(Some(&after), 10).unwrap();
    assert_eq!(
        page.operations
            .iter()
            .map(|candidate| candidate.operation.id.clone())
            .collect::<Vec<_>>(),
        vec![
            OperationId {
                sequence: 1,
                ..first.clone()
            },
            OperationId {
                sequence: 2,
                ..first
            }
        ]
    );
}
