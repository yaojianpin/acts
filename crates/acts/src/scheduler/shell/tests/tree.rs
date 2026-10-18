use crate::Workflow;

#[tokio::test]
async fn sch_tree_next_error() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1").with_next("next1"));
    assert!(
        workflow.valid().is_err(),
        "next node 'next1' does not exist, should error"
    );
}

#[test]
fn sch_tree_node_data_round_trips() {
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_step(|s| s.with_id("step2")))
            .with_branch(|b| b.with_id("b2").with_step(|s| s.with_id("step3")))
    });

    for (id, parent, next, level) in [
        ("w1", None, None, 0usize),
        ("step1", Some("w1"), None, 1),
        ("b1", Some("step1"), None, 2),
        ("step2", Some("b1"), None, 3),
    ] {
        let data = workflow.node_data(id).unwrap();
        assert_eq!(data.id(), id);
        assert_eq!(data.content.id(), id);
        assert_eq!(data.level, level);
        assert_eq!(data.parent_id.as_deref(), parent);
        assert_eq!(data.next_id.as_deref(), next);
    }
}

#[test]
fn sch_tree_node_data_root_is_the_workflow_node() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("step1"));
    let root = workflow.node_data("$").unwrap();
    assert_eq!(root.id(), "w1");
    assert_eq!(root.level, 0);
    assert_eq!(root.next_id, None);
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
