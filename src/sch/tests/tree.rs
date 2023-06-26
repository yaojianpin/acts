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

#[tokio::test]
async fn sch_tree_from() {
    let text = include_str!("./models/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    let tr = NodeTree::build(&mut workflow);
    assert!(tr.root.is_some());
}

#[tokio::test]
async fn sch_tree_get() {
    let text = include_str!("./models/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();

    let tr = NodeTree::build(&mut workflow);

    let job = tr.node("job1");
    assert!(job.is_some());
}

#[tokio::test]
async fn sch_tree_new() {
    let mut tr = NodeTree::new();

    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeData::Workflow(workflow);
    let node = tr.make("", data, 0);
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
    let parent = tr.make("", data, 0);

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeData::Workflow(workflow);
    let node = tr.make("", data, 1);
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
    let prev = tr.make("", data, 0);

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeData::Workflow(workflow);
    let node = tr.make("", data, 1);
    prev.set_next(&node);

    assert_eq!(prev.next().upgrade().unwrap().id(), "2");
}
