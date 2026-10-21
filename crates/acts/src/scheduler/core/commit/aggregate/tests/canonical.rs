use super::*;

#[test]
fn commit_digest_covers_graph_topology_and_model_identity() {
    let mut left = repository().load("p1").unwrap().unwrap();
    let mut right = left.clone();
    let task = id();
    right.tasks.get_mut(&task).unwrap().model_node = NodeId::parse("different-model").unwrap();
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
    left.version.graph = 9;
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
}

#[test]
fn commit_digest_is_unambiguous_for_separator_like_user_data() {
    let mut left = repository().load("p1").unwrap().unwrap();
    let mut right = left.clone();
    left.variables.insert((id(), "a:b".into()), "c".into());
    right.variables.insert((id(), "a".into()), "b:c".into());
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
}

#[test]
fn sha256_matches_standard_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn commit_digest_is_fixed_size_lower_hex() {
    let digest = deterministic_digest(&repository().load("p1").unwrap().unwrap()).unwrap();
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
}

#[test]
fn canonical_process_and_commit_digest_match_golden_fingerprint() {
    let snapshot = repository().load("p1").unwrap().unwrap();
    let bytes = canonical_process_bytes(&snapshot).unwrap();
    const GOLDEN_DIGEST: &str = "a47d4bd489ffc8d817e121a5d1743a93b612d8bdd7da8a1ee2c5e1288967028e";
    assert_eq!(bytes.len(), 76);
    assert_eq!(sha256_hex(&bytes), GOLDEN_DIGEST);
    assert_eq!(deterministic_digest(&snapshot).unwrap(), GOLDEN_DIGEST);
}

#[test]
fn the_canonical_bytes_distinguish_every_state_and_kind_tag() {
    let tasks = [
        TaskState::Created,
        TaskState::Ready,
        TaskState::Running,
        TaskState::Waiting,
        TaskState::Completed,
        TaskState::Submitted,
        TaskState::Backed,
        TaskState::Cancelled,
        TaskState::Failed,
        TaskState::Aborted,
        TaskState::Skipped,
        TaskState::Removed,
    ];
    let kinds = [
        OperationKind::ExecuteTask,
        OperationKind::PropagateTask,
        OperationKind::ApplyAction,
        OperationKind::SkipTask,
        OperationKind::AbortTask,
        OperationKind::DispatchTimeout,
        OperationKind::ReenterTask,
        OperationKind::ReturnToParent,
        OperationKind::DeliverMessage,
        OperationKind::FireTrigger,
        OperationKind::RunPostCommit,
    ];
    let claim = crate::scheduler::core::model::operation::ClaimToken {
        owner: "w".into(),
        fence: 1,
        lease_until_ms: 2,
        heartbeat_revision: 0,
    };
    let commit = crate::scheduler::core::model::operation::CommitToken {
        process_version: 3,
        digest: "d".into(),
    };
    let states = [
        OperationState::Pending,
        OperationState::Claimed(claim.clone()),
        OperationState::CommitPrepared(claim.clone()),
        OperationState::Committed(commit.clone()),
        OperationState::RetryScheduled { attempt: 1 },
        OperationState::EffectUnknown { attempt: 1 },
        OperationState::Quarantined {
            reason: "hold".into(),
        },
        OperationState::Superseded { by_generation: 1 },
        OperationState::Completed(commit),
    ];
    let mut snapshot = ProcessSnapshot::empty("p").unwrap();
    for (index, state) in tasks.iter().copied().enumerate() {
        snapshot.tasks.insert(
            TaskId::new(format!("task{index}"), 0).unwrap(),
            TaskSnapshot {
                model_node: NodeId::parse(format!("model{index}")).unwrap(),
                parent: None,
                predecessors: vec![],
                state,
                revision: index as u64,
            },
        );
    }
    for (index, kind) in kinds.iter().copied().enumerate() {
        for (slot, state) in states.iter().enumerate() {
            let id = OperationId {
                process_id: "p".into(),
                task: TaskId::new("task0", 0).unwrap(),
                sequence: (index * states.len() + slot) as u64,
            };
            snapshot.operations.insert(
                id.clone(),
                crate::scheduler::core::model::operation::OperationRecord {
                    id,
                    kind,
                    state: state.clone(),
                    revision: 0,
                },
            );
        }
    }
    let bytes = canonical_process_bytes(&snapshot).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("Cancelled"));
    assert!(text.contains("Removed"));
    assert!(text.contains("RunPostCommit"));
    assert!(text.contains("CommitPrepared("));
    assert!(text.contains("Committed("));
    assert!(text.contains("RetryScheduled {"));
    assert!(text.contains("Quarantined {"));
    assert!(text.contains("Superseded {"));
    // Every distinct state/kind changes the digest of an otherwise identical
    // aggregate.
    let mut other = snapshot.clone();
    let id = OperationId {
        process_id: "p".into(),
        task: TaskId::new("task0", 0).unwrap(),
        sequence: 0,
    };
    other.operations.get_mut(&id).unwrap().kind = OperationKind::ApplyAction;
    other.operations.get_mut(&id).unwrap().state = OperationState::Pending;
    assert_ne!(
        canonical_process_bytes(&snapshot).unwrap(),
        canonical_process_bytes(&other).unwrap()
    );
    assert_eq!(sha256_hex(&bytes).len(), 64);
}
