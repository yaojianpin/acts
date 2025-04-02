use std::sync::Arc;

use crate::{
    scheduler::{
        tree::{NodeContent, NodeTree},
        Node,
    },
    Act, NodeKind, Workflow,
};

const SIMPLE_WORKFLOW: &str = r#"
id: m1
steps:
- name: step1
  id: step1
  run: print("step1")
    "#;

#[tokio::test]
async fn sch_tree_from() {
    let mut workflow = Workflow::from_yml(SIMPLE_WORKFLOW).unwrap();
    let tr = NodeTree::build(&mut workflow).unwrap();
    assert!(tr.root.is_some());
}

#[tokio::test]
async fn sch_tree_get() {
    let mut workflow = Workflow::from_yml(SIMPLE_WORKFLOW).unwrap();
    let tr = NodeTree::build(&mut workflow).unwrap();

    let step1 = tr.node("step1");
    assert!(step1.is_some());
}

#[tokio::test]
async fn sch_tree_new() {
    let mut tr = NodeTree::new();

    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeContent::Workflow(workflow);
    let node = tr.make(&data.id(), data, 0).unwrap();
    tr.set_root(&node);
    assert!(tr.root.is_some());
    assert_eq!(tr.root.unwrap().id(), "1");
}

#[tokio::test]
async fn sch_tree_set_parent() {
    let tr = NodeTree::new();
    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeContent::Workflow(workflow);
    let parent = tr.make(&data.id(), data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeContent::Workflow(workflow);
    let node = tr.make(&data.id(), data, 1).unwrap();
    node.set_parent(&parent);

    assert!(!parent.children().is_empty());
    assert_eq!(node.parent().unwrap().id(), "1");
}

#[tokio::test]
async fn sch_tree_set_next() {
    let tr = NodeTree::new();

    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeContent::Workflow(workflow);
    let prev = tr.make(&data.id(), data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeContent::Workflow(workflow);
    let node = tr.make(&data.id(), data, 1).unwrap();
    prev.set_next(&node, true);

    assert_eq!(prev.next().upgrade().unwrap().id(), "2");
}

#[tokio::test]
async fn sch_tree_steps() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|s| s.with_id("step1"))
        .with_step(|s| s.with_id("step2"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let root = tree.root().unwrap();

    let node1 = tree.node("step1").unwrap();
    let node2 = tree.node("step2").unwrap();
    assert_eq!(node1.parent().unwrap().id(), "w1");
    assert_eq!(node2.parent().unwrap().id(), "w1");
    assert_eq!(node1.next().upgrade().unwrap().id(), "step2");
    assert_eq!(root.children().len(), 1);
}

#[tokio::test]
async fn sch_tree_step_auto_next() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step1 = tree.node("step1").unwrap();
    let step2 = tree.node("step2").unwrap();
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step1.next().upgrade().unwrap().id(), "step2");
    assert_eq!(step2.next().upgrade().unwrap().id(), "step3");
    assert!(step3.next().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_step_next_value() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3").with_next("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step3.next().upgrade().unwrap().id(), "step1");
}

#[tokio::test]
async fn sch_tree_step_prev() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step1 = tree.node("step1").unwrap();
    let step2 = tree.node("step2").unwrap();
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step3.prev().upgrade().unwrap().id(), "step2");
    assert_eq!(step2.prev().upgrade().unwrap().id(), "step1");
    assert!(step1.prev().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_branches() {
    let mut workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1"))
            .with_branch(|b| b.with_id("b2"))
    });
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step = tree.node("step1").unwrap();
    let b1 = tree.node("b1").unwrap();
    let b2 = tree.node("b2").unwrap();
    assert_eq!(b1.parent().unwrap().id(), "step1");
    assert_eq!(b2.parent().unwrap().id(), "step1");
    assert_eq!(step.children().len(), 2);
}

#[tokio::test]
async fn sch_tree_branch_steps() {
    let mut workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| {
                b.with_id("b1")
                    .with_step(|step| step.with_id("step11"))
                    .with_step(|step| step.with_id("step12"))
            })
            .with_branch(|b| {
                b.with_id("b2")
                    .with_step(|step| step.with_id("step21"))
                    .with_step(|step| step.with_id("step22"))
            })
    });
    let tree = NodeTree::build(&mut workflow).unwrap();
    tree.print();
    let b1 = tree.node("b1").unwrap();
    let b2 = tree.node("b2").unwrap();
    assert_eq!(b1.children().len(), 1);
    assert_eq!(b2.children().len(), 1);

    let step11 = tree.node("step11").unwrap();
    let step12 = tree.node("step12").unwrap();
    assert_eq!(step11.parent().unwrap().id(), "b1");
    assert_eq!(step12.parent().unwrap().id(), "b1");
}

#[tokio::test]
async fn sch_tree_acts() {
    let mut workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::new().with_id("act1"))
            .with_act(Act::new().with_id("act2"))
    });

    let tree = NodeTree::build(&mut workflow).unwrap();
    let step = tree.node("step1").unwrap();
    let act1 = tree.node("act1").unwrap();
    let act2 = tree.node("act2").unwrap();
    assert_eq!(step.children().len(), 2);
    assert_eq!(act1.parent().unwrap().id(), "step1");
    assert_eq!(act2.parent().unwrap().id(), "step1");
}

#[tokio::test]
async fn sch_tree_node_workflow_ser_de() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let w = tree.node("w1").unwrap();

    let data = w.to_string();
    let w2 = Node::from_str(&data, &tree);
    assert_eq!(w2.children().len(), w.children().len());
    assert_eq!(w2.id, w.id);
    assert_eq!(w2.kind(), w.kind());
    assert_eq!(w2.level, w.level);
    assert!(w2.parent().is_none());
    assert!(w2.prev().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_node_step_ser_de() {
    let mut workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_step(|s| s.with_id("step2")))
            .with_branch(|b| b.with_id("b2").with_step(|s| s.with_id("step3")))
    });
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step1 = tree.node("step1").unwrap();

    let data = step1.to_string();
    let step = Node::from_str(&data, &tree);
    assert_eq!(step.children().len(), 2);
    assert_eq!(step.id, "step1");
    assert_eq!(step.kind(), NodeKind::Step);
    assert_eq!(step.level, step.level);
    assert_eq!(step.parent().unwrap().id, "w1");
    assert!(step.prev().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_node_branch_ser_de() {
    let mut workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_step(|s| s.with_id("step2")))
            .with_branch(|b| b.with_id("b2").with_step(|s| s.with_id("step3")))
    });
    let tree = NodeTree::build(&mut workflow).unwrap();
    let b1 = tree.node("b1").unwrap();
    let data = b1.to_string();

    let b = Node::from_str(&data, &tree);
    assert_eq!(b.children().len(), 1);
    assert_eq!(b.id, "b1");
    assert_eq!(b.kind(), NodeKind::Branch);
    assert_eq!(b.level, b1.level);
    assert_eq!(b.parent().unwrap().id, "step1");
}

#[tokio::test]
async fn sch_tree_node_act_ser_de() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step1 = tree.node("step1").unwrap();
    let act1 = Arc::new(Node::new(
        "act_id_1",
        NodeContent::Act(Act::irq(|r| r.with_key("act1"))),
        step1.level + 1,
    ));

    act1.set_parent(&step1);

    let data = act1.to_string();
    let act = Node::from_str(&data, &tree);
    assert_eq!(act.children().len(), 0);
    assert_eq!(act.id, "act_id_1");
    assert_eq!(act.kind(), NodeKind::Act);
    assert_eq!(act.level, step1.level + 1);

    // not care about the parent when deserialize
    assert!(act.parent().is_none());
}

#[tokio::test]
async fn sch_tree_node_act2_ser_de() {
    let mut workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step1 = tree.node("step1").unwrap();
    let act1 = Arc::new(Node::new(
        "act_id_1",
        NodeContent::Act(Act::irq(|r| r.with_key("act1"))),
        step1.level + 1,
    ));

    let act2 = Arc::new(Node::new(
        "act_id_2",
        NodeContent::Act(Act::irq(|r| r.with_key("act2"))),
        act1.level + 1,
    ));

    act2.set_parent(&act1);

    let data = act2.to_string();
    let act = Node::from_str(&data, &tree);
    assert_eq!(act.children().len(), 0);
    assert_eq!(act.id, "act_id_2");
    assert_eq!(act.kind(), NodeKind::Act);
    assert_eq!(act.level, act1.level + 1);
    // not care the parent for act when resume data from string
    assert!(act.parent().is_none());
}
