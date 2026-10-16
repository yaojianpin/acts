use super::*;

#[test]
fn process_rejects_operation_key_value_identity_mismatch() {
    let mut snapshot = repository().load("p1").unwrap().unwrap();
    let key = OperationId {
        process_id: "p1".into(),
        node: "t1".into(),
        generation: 0,
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
        node: "t1".into(),
        generation: 0,
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
            node: "t1".into(),
            generation: 0,
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
            .map(|op| op.id.sequence)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        second
            .operations
            .iter()
            .map(|op| op.id.sequence)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(second.next.is_none());
    assert_eq!(
        repo.recoverable_page(None, 0),
        Err(CommitError::InvalidPageLimit)
    );
}
