//! `proc:graph` cells: the run's declared model graph — every node, executed
//! or not, in declaration order — with each node's newest execution state and
//! the tree rendering a client can log.
//!
//! A message names one model node (`nid`) and one execution instance (`tid`);
//! the read model's rows exist only for executions. A node no execution
//! reached has no `state`, so the graph is where a client learns the flow
//! ahead: it renders the graph and applies the messages by `nid`.

use serial_test::serial;

use crate::{Act, MessageState, Vars, Workflow, utils::test::USES_IRQ};

use super::FlowFixture;

/// The container package whose inline `acts` the kernel materializes.
const USES_BLOCK: &str = "acts.core.block";

/// The graph answers a waiting run's whole declaration with the live states:
/// the node the client is answering is `interrupt`, the step behind it is
/// still `pending`, and the declared ids come in declaration order — the tree
/// rendering pins the same facts in one string.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_answers_every_declared_node_with_its_state() {
    let workflow = Workflow::new()
        .with_id("flow-graph-nodes")
        .with_name("Graph Nodes")
        .with_step(|step| {
            step.with_id("wait")
                .with_name("wait")
                .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
        })
        .with_step(|step| step.with_id("after").with_name("after"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_created("wait").await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    assert_eq!(graph.pid, fixture.pid());
    assert_eq!(graph.mid, "flow-graph-nodes");
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.nid.as_str())
            .collect::<Vec<_>>(),
        vec!["$", "wait", "after"]
    );

    let root = &graph.nodes[0];
    assert_eq!(root.kind, "workflow");
    assert_eq!(root.parent, None);
    assert_eq!(root.state.as_deref(), Some("running"));
    let wait = &graph.nodes[1];
    // The graph is the declaration: a `uses` step is still a step, `uses`
    // marks the act (the *message* level).
    assert_eq!(wait.kind, "step");
    assert_eq!(wait.uses.as_deref(), Some(USES_IRQ));
    assert_eq!(wait.parent.as_deref(), Some("$"));
    assert_eq!(wait.state.as_deref(), Some("interrupted"));
    assert_eq!(wait.generation, Some(0));
    assert_eq!(wait.next, None);
    // `after` is the flow ahead: its seeded execution is still `Created`, so
    // it has no state (the flow has not reached it) and no message.
    assert_eq!(graph.nodes[2].kind, "step");
    assert_eq!(graph.nodes[2].parent.as_deref(), Some("$"));
    assert_eq!(graph.nodes[2].state, None);
    assert_eq!(graph.nodes[2].generation, None);
    assert!(
        fixture.message("after", MessageState::Created).is_none(),
        "the step behind the waiting act has not been announced"
    );

    assert_eq!(
        graph.tree_output(),
        concat!(
            "workflow id:$ name=Graph Nodes  uses=nil state=running next=nil\n",
            "├── step id:wait name=wait  uses=acts.core.irq state=interrupted next=nil\n",
            "└── step id:after name=after  uses=nil state=nil next=nil\n",
        )
    );
    fixture.close().await;
}

/// The graph is the same materialized model the messages name: a container
/// step's inline acts appear as their kernel children (`<step>#acts`, the
/// act's own id) with their own states, and every announced node folds onto
/// the graph by `nid` — its message level is `act` exactly when the node
/// carries `uses`. That is the join that makes the message stream a trace
/// over the graph.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_names_the_materialized_nodes_the_messages_name() {
    let mut workflow = Workflow::new()
        .with_id("flow-graph-block")
        .with_name("Graph Block");
    workflow.steps.push(crate::model::Step {
        id: "step1".to_string(),
        name: "step1".to_string(),
        uses: Some(USES_BLOCK.to_string()),
        params: Vars::new()
            .with("mode", "sequence")
            .with(
                "acts",
                vec![
                    Act::msg(|act| act.with_id("m1")),
                    Act::irq(|act| {
                        act.with_id("act1")
                            .with_params_vars(|v| v.with("key", "act1"))
                    }),
                ],
            )
            .to_value(),
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_message("m1", MessageState::Completed).await;
    fixture.wait_created("act1").await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    let node = |nid: &str| {
        graph
            .nodes
            .iter()
            .find(|node| node.nid == nid)
            .unwrap_or_else(|| panic!("the graph names no node '{nid}'"))
    };
    // The materialized shape, each node with its own live state.
    let step = node("step1");
    assert_eq!(step.kind, "step");
    assert_eq!(step.uses.as_deref(), Some(USES_BLOCK));
    assert_eq!(step.state.as_deref(), Some("running"));
    let branch = node("step1#acts");
    assert_eq!(branch.kind, "branch");
    assert_eq!(branch.parent.as_deref(), Some("step1"));
    assert_eq!(branch.state.as_deref(), Some("running"));
    assert_eq!(node("m1").kind, "step");
    assert_eq!(node("m1").state.as_deref(), Some("completed"));
    let act = node("act1");
    assert_eq!(act.kind, "step");
    assert_eq!(act.uses.as_deref(), Some(USES_IRQ));
    assert_eq!(act.parent.as_deref(), Some("step1#acts"));
    assert_eq!(act.state.as_deref(), Some("interrupted"));

    // Every announced node is a graph node, and its message level is the
    // node's: `act` for a `uses` step, the declared word otherwise.
    for message in fixture.messages() {
        let expected = node(&message.nid);
        let level = if expected.uses.is_some() {
            "act"
        } else {
            expected.kind.as_str()
        };
        assert_eq!(
            level, message.r#type,
            "{} announced as {} but the graph's level is {}",
            message.nid, message.r#type, level
        );
    }
    // The container's own act is announced too (its node is the step).
    assert!(
        fixture
            .messages()
            .iter()
            .any(|message| message.nid == "step1")
    );

    assert_eq!(
        graph.tree_output(),
        concat!(
            "workflow id:$ name=Graph Block  uses=nil state=running next=nil\n",
            "└── step id:step1 name=step1  uses=acts.core.block state=running next=nil\n",
            "    └── branch id:step1#acts name=  uses=nil state=running next=nil\n",
            "        ├── step id:m1 name=  uses=acts.core.msg state=completed next=nil\n",
            "        └── step id:act1 name=  uses=acts.core.irq state=interrupted next=nil\n",
        )
    );
    fixture.close().await;
}
