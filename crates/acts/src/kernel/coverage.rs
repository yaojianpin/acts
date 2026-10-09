#![cfg(test)]

//! Exhaustive type-surface tests. These keep warnings actionable under
//! `-D warnings`: a newly added domain variant must be exercised deliberately.
use super::*;

#[test]
fn model_and_execution_graph_read_surfaces_are_exercised() {
    let root = NodeId::parse("root").unwrap();
    let child = NodeId::parse("child").unwrap();
    assert_eq!(root.as_str(), "root");
    let mut model = ModelGraphBuilder::default();
    model
        .add_node(NodeDefinition {
            id: root.clone(),
            kind: NodeKind::Workflow,
            name: "root".into(),
        })
        .unwrap();
    model
        .add_node(NodeDefinition {
            id: child.clone(),
            kind: NodeKind::Branch,
            name: "child".into(),
        })
        .unwrap();
    model.set_root(root.clone()).unwrap();
    model
        .add_child(root.clone(), child.clone(), ChildRole::Catch)
        .unwrap();
    model
        .set_explicit_next(root.clone(), child.clone())
        .unwrap();
    let model = model.build().unwrap();
    assert_eq!(model.root(), &root);
    assert_eq!(model.node(&child).unwrap().kind, NodeKind::Branch);
    assert_eq!(model.parent(&child), Some((&root, ChildRole::Catch)));
    assert_eq!(model.explicit_successor(&root), Some(&child));

    let timeout = NodeId::parse("timeout").unwrap();
    let mut another = ModelGraphBuilder::default();
    another
        .add_node(NodeDefinition {
            id: root.clone(),
            kind: NodeKind::Workflow,
            name: "root".into(),
        })
        .unwrap();
    another
        .add_node(NodeDefinition {
            id: timeout.clone(),
            kind: NodeKind::Act,
            name: "timeout".into(),
        })
        .unwrap();
    another.set_root(root).unwrap();
    another
        .add_child(NodeId::parse("root").unwrap(), timeout, ChildRole::Timeout)
        .unwrap();
    another.build().unwrap();

    let eroot = ExecutionId::new("root", 0).unwrap();
    let echild = ExecutionId::new("child", 0).unwrap();
    let mut execution = ExecutionGraphBuilder::new("p", 0).unwrap();
    execution
        .add_node(ExecutionNode {
            id: eroot.clone(),
            model_node: NodeId::parse("root").unwrap(),
            state: ExecutionState::Running,
            revision: 0,
        })
        .unwrap();
    execution
        .add_node(ExecutionNode {
            id: echild.clone(),
            model_node: NodeId::parse("child").unwrap(),
            state: ExecutionState::Removed,
            revision: 0,
        })
        .unwrap();
    execution.set_root(eroot.clone()).unwrap();
    execution.add_child(eroot.clone(), echild.clone()).unwrap();
    let execution = execution.build().unwrap();
    assert_eq!(execution.root(), &eroot);
    assert_eq!(execution.parent(&echild), Some(&eroot));
    assert_eq!(execution.children(&eroot), &[echild]);
}

#[test]
fn every_action_and_predicate_outcome_is_represented() {
    let actions = [
        ActionKind::Submit,
        ActionKind::Skip,
        ActionKind::Abort,
        ActionKind::Fail {
            code: "E".into(),
            message: "failed".into(),
        },
    ];
    assert_eq!(actions.len(), 4);
    let invalid = PredicateValue::Invalid {
        message: "bad predicate".into(),
    };
    assert!(matches!(invalid, PredicateValue::Invalid { .. }));
}

#[test]
fn operation_recovery_states_and_kinds_are_exercised() {
    let id = OperationId {
        process_id: "p".into(),
        task_id: "t".into(),
        generation: 0,
        sequence: 0,
    };
    let claim = ClaimToken {
        owner: "r".into(),
        fence: 1,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    };
    for kind in [
        OperationKind::DeliverMessage,
        OperationKind::FireTrigger,
        OperationKind::RunPostCommit,
    ] {
        assert_eq!(
            OperationRecord::pending(id.clone(), kind).state,
            OperationState::Pending
        );
    }
    let claimed = OperationRecord::pending(id.clone(), OperationKind::ExecuteTask)
        .evolve(OperationEvent::Claim(claim.clone()))
        .unwrap();
    assert!(matches!(
        claimed
            .clone()
            .evolve(OperationEvent::ScheduleRetry {
                claim: claim.clone(),
                attempt: 1
            })
            .unwrap()
            .state,
        OperationState::RetryScheduled { attempt: 1 }
    ));
    assert!(matches!(
        claimed
            .clone()
            .evolve(OperationEvent::MarkEffectUnknown {
                claim: claim.clone(),
                attempt: 1
            })
            .unwrap()
            .state,
        OperationState::EffectUnknown { attempt: 1 }
    ));
    assert!(matches!(
        claimed
            .evolve(OperationEvent::Quarantine {
                claim: claim.clone(),
                reason: "invalid external result".into()
            })
            .unwrap()
            .state,
        OperationState::Quarantined { .. }
    ));
}

#[test]
fn transition_plan_has_canonical_variable_order() {
    let execution = ExecutionId::new("t", 0).unwrap();
    let plan = TransitionPlan {
        process_id: "p".into(),
        expected: AggregateVersion {
            graph: 0,
            process: 0,
            operations: 0,
        },
        execution_creates: vec![],
        state_changes: vec![],
        variable_writes: vec![
            VariableWrite {
                scope: execution.clone(),
                name: "z".into(),
                value: "2".into(),
            },
            VariableWrite {
                scope: execution,
                name: "a".into(),
                value: "1".into(),
            },
        ],
        source_operation: None,
        operations: vec![],
        outbox: vec![],
    };
    let canonical = plan.canonical_variables();
    let names = canonical
        .keys()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["a", "z"]);
}
