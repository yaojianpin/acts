use crate::{
    sch::tree::{NodeData, NodeTree},
    Workflow,
};

#[derive(Clone)]
struct Data(i32);

impl std::fmt::Display for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_string())
    }
}

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
    let data = NodeData::Workflow(workflow);
    let node = tr.make(data, 0).unwrap();
    tr.set_root(&node);
    assert!(tr.root.is_some());
    assert_eq!(tr.root.unwrap().id(), "1");
}

#[tokio::test]
async fn sch_tree_set_parent() {
    let tr = NodeTree::new();
    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeData::Workflow(workflow);
    let parent = tr.make(data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeData::Workflow(workflow);
    let node = tr.make(data, 1).unwrap();
    node.set_parent(&parent);

    assert!(parent.children().len() > 0);
    assert_eq!(node.parent().unwrap().id(), "1");
}

#[tokio::test]
async fn sch_tree_set_next() {
    let tr = NodeTree::new();

    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeData::Workflow(workflow);
    let prev = tr.make(data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeData::Workflow(workflow);
    let node = tr.make(data, 1).unwrap();
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
            .with_act(|act| act.with_id("act1"))
            .with_act(|act| act.with_id("act2"))
    });
    let tree = NodeTree::build(&mut workflow).unwrap();
    let step = tree.node("step1").unwrap();
    let act1 = tree.node("act1").unwrap();
    let act2 = tree.node("act2").unwrap();
    assert_eq!(step.children().len(), 2);
    assert_eq!(act1.parent().unwrap().id(), "step1");
    assert_eq!(act2.parent().unwrap().id(), "step1");
}
