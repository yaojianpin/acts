//! `proc:graph` cells: the run's model graph — every declared node, executed
//! or not, in declaration order, expanded over the aggregate's execution
//! instances — with each execution's state and the tree rendering a client can
//! log. A fan-out node appears once per item; a dynamic execution the model
//! does not declare (a pushed act, a join request) appears under the execution
//! that created it.
//!
//! A message names one model node (`nid`) and one execution instance (`tid`);
//! the read model's rows exist only for executions. A node no execution
//! reached has no `tid` and no `state`, so the graph is where a client learns
//! the flow ahead: it renders the graph and applies the messages by `tid` (or
//! by `nid` when it wants every instance of a fan-out node).

use serde_json::json;
use serial_test::serial;

use crate::{
    MessageState, Step, Vars, Workflow,
    event::EventAction,
    scheduler::core::model::task_graph::TaskState,
    utils::test::{USES_IRQ, USES_MSG, USES_PARALLEL},
};

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
                .with_var("token", 7)
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
    // The root and the flow behind it are executions: the graph names the
    // instance the messages address, not only the model node.
    assert!(root.tid.is_some());
    assert_eq!(root.parent_tid, None);
    let wait = &graph.nodes[1];
    assert_eq!(wait.parent_tid.as_deref(), root.tid.as_deref());
    // The graph is the declaration: a `uses` step is still a step, `uses`
    // marks the act (the *message* level).
    assert_eq!(wait.kind, "step");
    assert_eq!(wait.uses.as_deref(), Some(USES_IRQ));
    assert_eq!(wait.params["key"], "hold");
    assert_eq!(wait.vars.len(), 1);
    assert_eq!(wait.vars[0].name, "token");
    assert_eq!(wait.vars[0].value, 7);
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
/// over the graph. The materialized container itself is the kernel's own
/// virtual node (the declaration declares no branch): kind `block` on the
/// graph and in its messages, and no line of its own in the tree — its acts
/// fold into the parent step, which is where the declared flow puts them.
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
                "steps",
                vec![
                    Step::new().with_uses(USES_MSG, Vars::new()).with_id("m1"),
                    Step::new()
                        .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                        .with_id("act1"),
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
    // The kernel's own container: the declaration declares no branch, so it
    // speaks `block` — not the declared `branch` level.
    assert_eq!(branch.kind, "block");
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
    // The virtual container announces as `block` — its graph kind.
    assert!(
        fixture
            .messages()
            .iter()
            .any(|message| message.nid == "step1#acts" && message.r#type == "block")
    );

    assert_eq!(
        graph.tree_output(),
        concat!(
            "workflow id:$ name=Graph Block  uses=nil state=running next=nil\n",
            "└── step id:step1 name=step1  uses=acts.core.block state=running next=nil\n",
            "    ├── step id:m1 name=  uses=acts.core.msg state=completed next=nil\n",
            "    └── step id:act1 name=  uses=acts.core.irq state=interrupted next=nil\n",
        )
    );
    fixture.close().await;
}

/// A `acts.core.parallel{ in }` step fans its template out once per item: the
/// items share one model node, so the graph shows one node per generation —
/// two `pm_vote`/`pm_notify` equivalents — instead of a single newest
/// generation. The virtual block levels fold away in the tree, leaving the
/// per-item acts at the container's level.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_expands_a_param_fanout_over_its_items() {
    let mut fan = Step::new();
    fan.id = "fan".to_string();
    fan.name = "fan".to_string();
    fan.uses = Some(USES_PARALLEL.to_string());
    fan.params = json!({
        "in": ["u1", "u2"],
        "steps": [
            Step::new().with_uses(USES_MSG, Vars::new().with("key", "notify")),
        ],
    });
    let mut workflow = Workflow::new()
        .with_id("flow-graph-fanout")
        .with_name("Graph Fanout");
    workflow.steps.push(fan);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_completed().await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    let of = |nid: &str| {
        graph
            .nodes
            .iter()
            .filter(|node| node.nid == nid)
            .collect::<Vec<_>>()
    };

    // One container, one branch, two template items.
    assert_eq!(of("fan").len(), 1);
    assert_eq!(of("fan#in").len(), 1);
    let templates = of("fan#item");
    assert_eq!(templates.len(), 2, "one template execution per in item");
    let mut generations: Vec<u64> = templates
        .iter()
        .filter_map(|node| node.generation)
        .collect();
    generations.sort_unstable();
    assert_eq!(generations, vec![0, 1]);
    assert!(
        templates
            .iter()
            .all(|node| node.state.as_deref() == Some("completed")),
        "both items completed: {templates:?}"
    );

    // Each item's own branch, and the leaf act under it, are separate
    // executions with distinct tids even though they share a model node.
    let branches = of("fan#item#act0");
    assert_eq!(branches.len(), 2);
    let template_tids: Vec<&str> = templates
        .iter()
        .filter_map(|node| node.tid.as_deref())
        .collect();
    let branch_tids: Vec<&str> = branches
        .iter()
        .filter_map(|node| node.tid.as_deref())
        .collect();
    for branch in &branches {
        assert_eq!(branch.parent.as_deref(), Some("fan#item"));
        assert!(
            template_tids.contains(&branch.parent_tid.as_deref().unwrap()),
            "a branch belongs to one template item: {branch:?}"
        );
    }
    let leaf_nid = graph
        .nodes
        .iter()
        .find(|node| node.uses.as_deref() == Some(USES_MSG))
        .expect("the fan-out's act is in the graph")
        .nid
        .clone();
    let leaves = of(&leaf_nid);
    assert_eq!(leaves.len(), 2, "one act execution per item");
    let mut tid_seen = Vec::new();
    for leaf in &leaves {
        assert_eq!(leaf.state.as_deref(), Some("completed"));
        assert!(
            branch_tids.contains(&leaf.parent_tid.as_deref().unwrap()),
            "an act belongs to its item's branch: {leaf:?}"
        );
        let tid = leaf.tid.as_deref().unwrap();
        assert!(!tid_seen.contains(&tid), "each item has its own instance");
        tid_seen.push(tid);
    }

    let tree = graph.tree_output();
    assert_eq!(
        tree,
        format!(
            "workflow id:$ name=Graph Fanout  uses=nil state=completed next=nil\n\
             └── step id:fan name=fan  uses=acts.core.parallel state=completed next=nil\n\
             \u{20}   ├── step id:fan#item name=  uses=acts.core.block state=completed next=nil\n\
             \u{20}   │   └── step id:{leaf_nid} name=  uses=acts.core.msg state=completed next=nil\n\
             \u{20}   └── step id:fan#item name=  uses=acts.core.block state=completed next=nil\n\
             \u{20}       └── step id:{leaf_nid} name=  uses=acts.core.msg state=completed next=nil\n"
        ),
        "the virtual blocks fold, the two items stay visible"
    );
    fixture.close().await;
}

/// A client's `Push` creates an act the model does not declare, and `Remove`
/// decides that execution removed. Both facts belong to the run's graph: the
/// pushed act appears under the execution that created it, with its own
/// `uses`, name and final `removed` state.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_shows_a_pushed_act_and_its_removal() {
    let workflow = crate::scheduler::tests::flow::irq_workflow("flow-graph-push", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Push,
            Vars::new()
                .with("name", "added approver")
                .with("uses", USES_IRQ)
                .with("params", Vars::new().with("key", "act2")),
        )
        .await
        .unwrap();
    let pushed = fixture.wait_created("s1#push1").await;

    // Answer the step, then drop the added act: the step continues once the
    // push is removed, exactly as in the approve example's countersign.
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_state("s1", TaskState::Running).await;
    fixture
        .act(&pushed.pid, &pushed.tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();
    fixture.wait_state("s1#push1", TaskState::Removed).await;
    fixture
        .wait_message("s1#push1", MessageState::Removed)
        .await;
    fixture.wait_state("s1", TaskState::Completed).await;
    fixture.wait_completed().await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    let step = graph
        .nodes
        .iter()
        .find(|node| node.nid == "s1")
        .expect("the answered step is in the graph");
    let dynamic = graph
        .nodes
        .iter()
        .find(|node| node.nid == "s1#push1")
        .expect("the pushed act the model does not declare is in the graph");
    assert_eq!(dynamic.name, "added approver");
    assert_eq!(dynamic.kind, "step");
    assert_eq!(dynamic.uses.as_deref(), Some(USES_IRQ));
    assert_eq!(dynamic.params["key"], "act2");
    assert_eq!(dynamic.parent.as_deref(), Some("s1"));
    assert_eq!(dynamic.parent_tid.as_deref(), step.tid.as_deref());
    assert_eq!(dynamic.tid.as_deref(), Some(pushed.tid.as_str()));
    assert_eq!(dynamic.state.as_deref(), Some("removed"));
    assert_eq!(dynamic.generation, Some(0));
    assert!(
        graph
            .tree_output()
            .contains("step id:s1#push1 name=added approver  uses=acts.core.irq state=removed"),
        "the pushed act and its removal render: {}",
        graph.tree_output()
    );
    fixture.close().await;
}
