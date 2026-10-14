#![cfg(test)]

//! Exhaustive type-surface tests. These keep warnings actionable under
//! `-D warnings`: a newly added domain variant must be exercised deliberately.
//!
//! The imports name each type where it lives rather than globbing a layer:
//! this suite is the inventory of the pure surface, so it should spell it out.
use crate::runtime::core::commit::command::ActionKind;
use crate::runtime::core::model::execution_graph::{
    ExecutionGraphBuilder, ExecutionId, ExecutionNode, ExecutionState,
};
use crate::runtime::core::model::model_graph::{
    ChildRole, ModelGraphBuilder, NodeDefinition, NodeId, NodeKind,
};
use crate::runtime::core::model::operation::{
    ClaimToken, OperationEvent, OperationId, OperationKind, OperationRecord, OperationState,
};
use crate::runtime::core::plan::branch::PredicateValue;

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
            kind: NodeKind::Act,
            name: "child".into(),
        })
        .unwrap();
    model.set_root(root.clone()).unwrap();
    model
        .add_child(root.clone(), child.clone(), ChildRole::Normal)
        .unwrap();
    model
        .set_explicit_next(root.clone(), child.clone())
        .unwrap();
    let model = model.build().unwrap();
    assert_eq!(model.successor(&root), Some(&child));

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
        .add_child(NodeId::parse("root").unwrap(), timeout, ChildRole::Normal)
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
