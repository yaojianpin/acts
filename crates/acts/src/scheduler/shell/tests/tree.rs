use crate::{
    Act, Workflow,
    scheduler::core::model::view::{Node, NodeContent, NodeData, NodeTree},
};
use std::sync::Arc;

fn try_tree_of(workflow: &Workflow) -> crate::Result<NodeTree> {
    let mut tree = NodeTree::new();
    tree.load(workflow)?;
    Ok(tree)
}

fn tree_of(workflow: &Workflow) -> NodeTree {
    try_tree_of(workflow).unwrap()
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
    let workflow = Workflow::from_yml(SIMPLE_WORKFLOW).unwrap();
    let tr = tree_of(&workflow);
    assert!(tr.root.is_some());
}

#[tokio::test]
async fn sch_tree_shares_workflow_model() {
    let workflow = Workflow::from_yml(SIMPLE_WORKFLOW).unwrap();
    let tr = tree_of(&workflow);

    let NodeContent::Workflow(model) = &tr.root.unwrap().content else {
        panic!("workflow root has invalid content");
    };
    assert!(Arc::ptr_eq(model, &tr.model));
}

#[tokio::test]
async fn sch_tree_get() {
    let workflow = Workflow::from_yml(SIMPLE_WORKFLOW).unwrap();
    let tr = tree_of(&workflow);

    let step1 = tr.node("step1");
    assert!(step1.is_some());
}

#[tokio::test]
async fn sch_tree_new() {
    let mut tr = NodeTree::new();

    let mut workflow = Workflow::default();
    workflow.set_id("1");
    let data = NodeContent::Workflow(Arc::new(workflow));
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
    let data = NodeContent::Workflow(Arc::new(workflow));
    let parent = tr.make(&data.id(), data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeContent::Workflow(Arc::new(workflow));
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
    let data = NodeContent::Workflow(Arc::new(workflow));
    let prev = tr.make(&data.id(), data, 0).unwrap();

    let mut workflow = Workflow::default();
    workflow.set_id("2");
    let data = NodeContent::Workflow(Arc::new(workflow));
    let node = tr.make(&data.id(), data, 1).unwrap();
    prev.set_next(&node, true);

    assert_eq!(prev.next().upgrade().unwrap().id(), "2");
}

#[tokio::test]
async fn sch_tree_steps() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|s| s.with_id("step1"))
        .with_step(|s| s.with_id("step2"));
    let tree = tree_of(&workflow);
    let root = tree.root().unwrap();

    let node1 = tree.node("step1").unwrap();
    let node2 = tree.node("step2").unwrap();
    assert_eq!(node1.parent().unwrap().id(), "w1");
    assert_eq!(node2.parent().unwrap().id(), "w1");
    assert_eq!(node1.next().upgrade().unwrap().id(), "step2");
    assert_eq!(root.children().len(), 1);
}

/// `NodeTree::load` and the kernel import share one id pass (`view::assign_ids`):
/// an unnamed root, step, branch, catch handler and timeout branch all get a
/// non-empty id the model graph can address. The caller's model is untouched —
/// the tree carries the assigned copy.
#[tokio::test]
async fn sch_tree_assigns_ids_to_unnamed_nodes() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_catch(|catch| catch.with_branch(|branch| branch.with_step(|inner| inner)))
        })
        .with_step(|step| step.with_timeout(|timeout| timeout));
    let tree = tree_of(&workflow);

    let legal = |id: &str| {
        assert!(!id.trim().is_empty(), "an assigned id must not be empty");
        crate::scheduler::core::model::model_graph::NodeId::parse(id)
            .unwrap_or_else(|_| panic!("'{id}' must parse as a node id"));
    };
    legal(&tree.model.id);
    let root = tree.root().unwrap();
    assert_eq!(root.id(), tree.model.id, "the root node is the workflow");
    legal(root.id());

    let branching = &tree.model.steps[0];
    let catch = &branching.catches[0];
    legal(&catch.id);
    let branch = &catch.branches[0];
    legal(&branch.id);
    legal(&branch.steps[0].id);
    let timing = &tree.model.steps[1];
    legal(&timing.timeouts[0].id);

    assert!(tree.node(&branch.steps[0].id).is_some());
    assert!(
        workflow.id.is_empty()
            && workflow.steps[0].catches[0].id.is_empty()
            && workflow.steps[0].catches[0].branches[0].id.is_empty(),
        "the caller's model stays untouched"
    );
}

#[tokio::test]
async fn sch_tree_step_auto_next() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3"));
    let tree = tree_of(&workflow);
    let step1 = tree.node("step1").unwrap();
    let step2 = tree.node("step2").unwrap();
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step1.next().upgrade().unwrap().id(), "step2");
    assert_eq!(step2.next().upgrade().unwrap().id(), "step3");
    assert!(step3.next().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_step_next_value() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3").with_next("step1"));
    let tree = tree_of(&workflow);
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step3.next().upgrade().unwrap().id(), "step1");
}

#[tokio::test]
async fn sch_tree_step_prev() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"))
        .with_step(|step| step.with_id("step3"));
    let tree = tree_of(&workflow);
    let step1 = tree.node("step1").unwrap();
    let step2 = tree.node("step2").unwrap();
    let step3 = tree.node("step3").unwrap();
    assert_eq!(step3.prev().upgrade().unwrap().id(), "step2");
    assert_eq!(step2.prev().upgrade().unwrap().id(), "step1");
    assert!(step1.prev().upgrade().is_none());
}

#[tokio::test]
async fn sch_tree_branches() {
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1"))
            .with_branch(|b| b.with_id("b2"))
    });
    let tree = tree_of(&workflow);
    let step = tree.node("step1").unwrap();
    let b1 = tree.node("b1").unwrap();
    let b2 = tree.node("b2").unwrap();
    assert_eq!(b1.parent().unwrap().id(), "step1");
    assert_eq!(b2.parent().unwrap().id(), "step1");
    assert_eq!(step.children().len(), 2);
}

#[tokio::test]
async fn sch_tree_branch_steps() {
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
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
    let tree = tree_of(&workflow);
    let b1 = tree.node("b1").unwrap();
    let b2 = tree.node("b2").unwrap();
    assert_eq!(b1.children().len(), 1);
    assert_eq!(b2.children().len(), 1);

    let step11 = tree.node("step11").unwrap();
    let step12 = tree.node("step12").unwrap();
    assert_eq!(step11.parent().unwrap().id(), "b1");
    assert_eq!(step12.parent().unwrap().id(), "b1");
}

#[test]
fn sch_tree_node_data_round_trips() {
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_step(|s| s.with_id("step2")))
            .with_branch(|b| b.with_id("b2").with_step(|s| s.with_id("step3")))
    });
    let tree = tree_of(&workflow);

    for (id, parent, next, level) in [
        ("w1", None, None, 0usize),
        ("step1", Some("w1"), None, 1),
        ("b1", Some("step1"), None, 2),
        ("step2", Some("b1"), None, 3),
    ] {
        let node = tree.node(id).unwrap();
        let data: NodeData = serde_json::from_str(&node.to_string().unwrap()).unwrap();
        assert_eq!(data.id, id);
        assert_eq!(data.content.id(), id);
        assert_eq!(data.level, level);
        assert_eq!(data.parent_id.as_deref(), parent);
        assert_eq!(data.next_id.as_deref(), next);
    }
}

#[test]
fn sch_tree_act_node_data_round_trips() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let tree = tree_of(&workflow);
    let step1 = tree.node("step1").unwrap();
    let act1 = Arc::new(Node::new(
        "act_id_1",
        NodeContent::Act(
            Act::irq(|r| r.with_params_vars(|v| v.with("key", "act1"))).with_id("act_id_1"),
        ),
        step1.level + 1,
    ));
    let act2 = Arc::new(Node::new(
        "act_id_2",
        NodeContent::Act(
            Act::irq(|r| r.with_params_vars(|v| v.with("key", "act2"))).with_id("act_id_2"),
        ),
        act1.level + 1,
    ));

    act1.set_parent(&step1);
    act2.set_parent(&act1);

    let data: NodeData = serde_json::from_str(&act2.to_string().unwrap()).unwrap();
    assert_eq!(data.id, "act_id_2");
    assert_eq!(data.content.id(), "act_id_2");
    assert!(matches!(data.content, NodeContent::Act(_)));
    assert_eq!(data.level, act1.level + 1);
    assert_eq!(data.parent_id.as_deref(), Some("act_id_1"));
    assert_eq!(data.prev_id, None);
    assert_eq!(data.next_id, None);
}
#[tokio::test]
async fn sch_tree_dyn_acts_restore_chain() {
    // build a dynamic act chain: the first act hangs off the step, the rest
    // follow it one by one
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let tree = tree_of(&workflow);
    let step1 = tree.node("step1").unwrap();

    let mut acts = [
        Act::irq(|r| r.with_params_vars(|v| v.with("key", "act1"))),
        Act::irq(|r| r.with_params_vars(|v| v.with("key", "act2"))),
        Act::irq(|r| r.with_params_vars(|v| v.with("key", "act3"))),
    ];
    let mut prev = step1.clone();
    for act in acts.iter_mut() {
        act.id = crate::utils::shortid();
        let node = tree
            .make(&act.id, NodeContent::Act(act.clone()), step1.level + 1)
            .unwrap();
        if node.level == prev.level {
            prev.set_next(&node, true);
        } else {
            node.set_parent(&step1);
        }
        prev = node;
    }

    // dynamic nodes are registered in the tree map
    let act_ids = acts.iter().map(|a| a.id.clone()).collect::<Vec<_>>();
    for id in &act_ids {
        assert!(tree.node(id).is_some(), "dynamic node {id} in tree map");
    }
    // chain: act1 -> act2 -> act3, act1 is the child of step1
    assert_eq!(step1.children().len(), 1);
    let act1 = tree.node(&act_ids[0]).unwrap();
    let act2 = tree.node(&act_ids[1]).unwrap();
    assert_eq!(act1.next().upgrade().unwrap().id(), act2.id());

    // serialize every node the way a task row stores it
    let datas = acts
        .iter()
        .map(|a| {
            let node = tree.node(&a.id).unwrap();
            serde_json::from_str::<NodeData>(&node.to_string().unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(datas[0].parent_id.is_some());
    assert_eq!(datas[0].next_id.as_deref(), Some(act_ids[1].as_str()));

    // restore into a fresh tree (mirrors store::load_proc + load_tasks:
    // register all dynamic nodes first, then rebuild the links)
    let tree2 = tree_of(&workflow);
    let mut dyn_nodes = Vec::new();
    for data in datas.iter() {
        let node = tree2
            .get_or_make(&data.id, data.content.clone(), data.level)
            .unwrap();
        dyn_nodes.push((node, data));
    }
    for (node, data) in dyn_nodes {
        node.restore_links(data, &tree2);
    }

    // links are rebuilt: prev/next chain, parent and children
    let a1 = tree2.node(&act_ids[0]).unwrap();
    let a2 = tree2.node(&act_ids[1]).unwrap();
    let a3 = tree2.node(&act_ids[2]).unwrap();
    assert_eq!(a1.next().upgrade().unwrap().id(), a2.id());
    assert_eq!(a2.prev().upgrade().unwrap().id(), a1.id());
    assert_eq!(a2.next().upgrade().unwrap().id(), a3.id());
    assert!(a3.next().upgrade().is_none());
    assert_eq!(a1.parent().unwrap().id(), "step1");
    // children restored as the inverse of the parent link
    let step = tree2.node("step1").unwrap();
    assert_eq!(step.children().len(), 1);
    assert_eq!(step.children()[0].id(), a1.id());
}

#[tokio::test]
async fn sch_tree_next_error() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1").with_next("next1"));
    let tree = try_tree_of(&workflow);
    assert!(
        tree.is_err(),
        "next node 'next1' does not exist, should error"
    );
}

const TREE_OUTPUT_MODEL: &str = r#"
name: tree output
id: c8a-tree
ver: 0.1.0
steps:
  - name: first
    id: first
    branches:
      - name: left
        id: left
        if: "true"
        steps:
          - name: left-1
            id: left-1
          - name: left-2
            id: left-2
      - name: right
        id: right
        if: "false"
        steps:
          - name: right-1
            id: right-1
          - name: right-2
            id: right-2
  - name: second
    id: second
    catches:
      - name: caught
        id: caught
    timeouts:
      - name: timed-out
        id: timed-out
  - name: third
    id: third
"#;

const TREE_OUTPUT: &str = r#"workflow id:c8a-tree name=tree output  next=nil
├── step id:first name=first  next=second
│   ├── branch id:left name=left  next=nil
│   │   ├── step id:left-1 name=left-1  next=left-2
│   │   └── step id:left-2 name=left-2  next=nil
│   └── branch id:right name=right  next=nil
│       ├── step id:right-1 name=right-1  next=right-2
│       └── step id:right-2 name=right-2  next=nil
├── step id:second name=second  next=third
│   └── step id:caught name=caught  next=nil
│   └── step id:timed-out name=timed-out  next=nil
└── step id:third name=third  next=nil
"#;

#[test]
fn sch_tree_output_is_byte_stable() {
    let workflow = Workflow::from_yml(TREE_OUTPUT_MODEL).unwrap();
    assert_eq!(workflow.tree_output(), TREE_OUTPUT);
}
