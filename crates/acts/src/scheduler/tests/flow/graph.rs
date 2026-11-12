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
    MessageState, Principal, Step, Vars, Workflow,
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
    // The wait act's own record folded onto its node: created, the owner
    // fields empty (no assignment declared here).
    assert_eq!(graph.nodes[1].messages.len(), 1);
    assert_eq!(graph.nodes[1].messages[0].r#type, "step");
    assert_eq!(graph.nodes[1].messages[0].state, "created");
    assert_eq!(graph.nodes[1].messages[0].uses.as_deref(), Some(USES_IRQ));
    assert_eq!(graph.nodes[1].messages[0].owner, None);
    // The flow behind carries no record yet.
    assert_eq!(graph.nodes[2].messages.len(), 0);
    fixture.close().await;
}

/// The graph is the same materialized model the messages name: a container
/// step's inline acts appear as their kernel children (`<step>#acts`, the
/// act's own id) with their own states, and every announced node folds onto
/// the graph by `nid`. That is the join that makes the message stream a trace
/// over the graph. The materialized container itself is the kernel's own
/// virtual node (the declaration declares no branch): kind `block` on the
/// graph, no line of its own in the tree, and silent — its acts fold into the
/// parent step, which is where the declared flow puts them.
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
    // node's declared word — a `uses` step's act payload says `step` too.
    // The notices (`notice`, `assignment`) announce no level.
    for message in fixture.messages() {
        let level = message.r#type.as_str();
        if !matches!(level, "workflow" | "branch" | "step" | "block") {
            continue;
        }
        let expected = node(&message.nid);
        assert_eq!(
            expected.kind.as_str(),
            level,
            "{} announced as {} but the graph's level is {}",
            message.nid,
            message.r#type,
            expected.kind.as_str()
        );
    }
    // The container's own act is announced too (its node is the step).
    assert!(
        fixture
            .messages()
            .iter()
            .any(|message| message.nid == "step1")
    );
    // The kernel's own wrappers — the virtual branch here — are not declared
    // nodes of the flow and announce nothing; they stay graph-only.
    assert!(
        fixture
            .messages()
            .iter()
            .all(|message| message.nid != "step1#acts" && message.r#type != "block"),
        "a virtual wrapper must not announce: {:?}",
        fixture.messages()
    );

    assert_eq!(
        graph.tree_output(),
        concat!(
            "workflow id:$ name=Graph Block  uses=nil state=running next=nil\n",
            "└── step id:step1 name=step1  uses=acts.core.block state=running next=nil\n",
            "    ├── step id:m1 name=  uses=acts.core.msg state=completed next=nil notice=1\n",
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
             \u{20}   │   └── step id:{leaf_nid} name=  uses=acts.core.msg state=completed next=nil notice=1\n\
             \u{20}   └── step id:fan#item name=  uses=acts.core.block state=completed next=nil\n\
             \u{20}       └── step id:{leaf_nid} name=  uses=acts.core.msg state=completed next=nil notice=1\n"
        ),
        "the two items stay visible"
    );
    fixture.close().await;
}

/// A `Back` from inside a candidates fan-out re-enters the head step as a new
/// generation. The graph keeps the original hierarchy in both rounds: the
/// closed countersign and its item/vote subtree stay under the backed
/// generation, and the re-created countersign renders the same item/vote
/// subtree under the re-entered head. Losing the virtual `<step>#in` bridge
/// (the kernel never admits it as an execution) used to drop the whole
/// template subtree and flatten both generations into bare lines.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_across_a_rewind() {
    let workflow = Workflow::new()
        .with_id("flow-graph-rewind")
        .with_step(|step| step.with_id("prepare").with_uses(USES_IRQ, Vars::new()))
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new()
                        .with("in", "${{ candidates }}")
                        .with("join", "all")
                        .with(
                            "steps",
                            vec![
                                Step::new()
                                    .with_id("vote")
                                    .with_uses(USES_IRQ, Vars::new().with("key", "pm_vote"))
                                    .with_owner("${{ $item.value }}"),
                            ],
                        ),
                )
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "reviewers".to_string(),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let prepare = fixture.wait_created("prepare").await;
    fixture
        .complete(&prepare.pid, &prepare.tid, Vars::new())
        .await;

    // The countersign asks its pool, then fans the template out to one vote,
    // which starts at once (the container's pool is never re-asked under it).
    let answers = Vars::new()
        .with(
            "participants",
            json!([{ "id": "u1", "code": "alice", "name": "Alice" }]),
        )
        .with("ids", json!(["u1"]))
        .with("count", 1);
    let request = fixture.wait_created("approve#cand1").await;
    fixture.complete(&request.pid, &request.tid, answers).await;
    let vote = fixture.wait_created("vote").await;

    // Back the waiting vote to the head: the old round closes `backed` and a
    // fresh `prepare`/`approve` generation is created.
    fixture.back(&vote.pid, &vote.tid, "prepare").await.unwrap();
    fixture.wait_created_except("prepare", &prepare.tid).await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    // The hierarchy is the executions' own edges, not the rendering: each
    // generation owns its template/vote chain, so a rewind never reparents
    // the closed round's subtree onto the re-created one.
    let node = |nid: &str, parent_tid: Option<&str>| {
        graph
            .nodes
            .iter()
            .find(|node| node.nid == nid && node.parent_tid.as_deref() == parent_tid)
            .unwrap_or_else(|| {
                panic!(
                    "the graph has no {nid} under {parent_tid:?}: {}",
                    graph.tree_output()
                )
            })
    };
    let root = node("$", None);
    assert!(root.tid.is_some(), "the root execution");

    // The closed head and the re-entered one are both root executions.
    let closed_head = graph
        .nodes
        .iter()
        .find(|node| {
            node.nid == "prepare"
                && node.parent_tid == root.tid
                && node.state.as_deref() == Some("completed")
        })
        .expect("the closed head completed before the rewind");
    let redo_head = graph
        .nodes
        .iter()
        .find(|node| {
            node.nid == "prepare"
                && node.parent_tid == root.tid
                && node.state.as_deref() == Some("interrupted")
        })
        .expect("the re-entered head waits for its decision");
    assert_ne!(redo_head.tid, closed_head.tid);

    let approvals: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.nid == "approve" && node.parent_tid == root.tid)
        .collect();
    assert_eq!(approvals.len(), 2, "the closed and the re-created round");
    let closed = approvals
        .iter()
        .find(|node| node.state.as_deref() == Some("backed"))
        .expect("the closed generation is backed");
    let redo = approvals
        .iter()
        .find(|node| node.state.is_none())
        .expect("the re-created generation is the flow ahead");
    assert_ne!(closed.tid, redo.tid);

    for (generation, closed_round) in [(closed, true), (redo, false)] {
        // The model's virtual bridge (`<step>#in`) has no execution of its
        // own; it must still be emitted under each generation, or its real
        // `<step>#item` children are never walked.
        let bridge = node("approve#in", generation.tid.as_deref());
        assert_eq!(bridge.tid, None);
        assert_eq!(bridge.kind, "block");
        let item = node("approve#item", generation.tid.as_deref());
        assert_eq!(item.parent.as_deref(), Some("approve#in"));
        let branch = node("approve#item#act0", item.tid.as_deref());
        assert_eq!(branch.parent.as_deref(), Some("approve#item"));
        let vote = node("vote", branch.tid.as_deref());
        assert_eq!(vote.parent.as_deref(), Some("approve#item#act0"));
        if closed_round {
            assert_eq!(item.state.as_deref(), Some("backed"));
            assert_eq!(vote.state.as_deref(), Some("backed"));
        } else {
            assert_eq!(item.state, None, "a seeded item is still the flow ahead");
            assert_eq!(vote.state, None);
        }
    }
    // The closed round's pool request stays under the closed generation; the
    // re-created round has not asked yet.
    let request = node("approve#cand1", closed.tid.as_deref());
    assert_eq!(request.parent.as_deref(), Some("approve"));
    assert_eq!(request.state.as_deref(), Some("completed"));
    assert!(
        graph
            .nodes
            .iter()
            .all(|node| node.nid != "approve#cand1" || node.parent_tid != redo.tid),
        "the re-created round asks its pool only when it runs"
    );
    fixture.close().await;
}

/// The action matrix's shape: one container step owning the act every client
/// action decides or assigns. The container is the parent edge the graph must
/// keep whatever the action does.
fn action_workflow(id: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow
        .steps
        .push(Step::new().with_id("approval").with_uses(
            USES_BLOCK,
            Vars::new().with("mode", "sequence").with(
                "steps",
                vec![
                    Step::new()
                        .with_id("s1")
                        .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                        .with_owner("alice"),
                ],
            ),
        ));
    workflow
}

/// The act under one container execution, through the kernel's virtual
/// `<step>#acts` branch: the branch is a graph node of its own (the execution
/// authority is `parent_tid`), and the act is its child. The tree folds the
/// branch's line, not the edge.
fn act_under<'a>(
    graph: &'a crate::ProcessGraph,
    container: &crate::GraphNode,
) -> (&'a crate::GraphNode, &'a crate::GraphNode) {
    let branch = graph
        .nodes
        .iter()
        .find(|node| node.nid == "approval#acts" && node.parent_tid == container.tid)
        .unwrap_or_else(|| panic!("the container's virtual branch: {}", graph.tree_output()));
    let act = graph
        .nodes
        .iter()
        .find(|node| node.nid == "s1" && node.parent_tid == branch.tid)
        .unwrap_or_else(|| panic!("the act under the virtual branch: {}", graph.tree_output()));
    (branch, act)
}

/// The hierarchy every client action must leave intact: the container hangs
/// off the root, and the act stays under the container's own virtual branch.
fn assert_action_hierarchy(graph: &crate::ProcessGraph) {
    let root = graph
        .nodes
        .iter()
        .find(|node| node.nid == "$" && node.tid.is_some())
        .expect("the run's root execution");
    let approval = graph
        .nodes
        .iter()
        .find(|node| node.nid == "approval" && node.tid.is_some())
        .expect("the container execution");
    assert_eq!(
        approval.parent_tid.as_deref(),
        root.tid.as_deref(),
        "the container stays under the root"
    );
    let (branch, act) = act_under(graph, approval);
    assert!(branch.tid.is_some(), "the container branch's execution");
    assert_eq!(branch.kind, "block", "the kernel's virtual container");
    assert_eq!(branch.parent.as_deref(), Some("approval"));
    assert_eq!(act.parent.as_deref(), Some("approval#acts"));
}

/// `Next`/`skip`/`error`/`abort` only change the act's state: the graph keeps
/// the container's and act's executions and their parent edges.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_through_terminal_actions() {
    for (action, state) in [
        ("complete", TaskState::Completed),
        ("skip", TaskState::Skipped),
        ("error", TaskState::Failed),
        ("abort", TaskState::Aborted),
    ] {
        let workflow = action_workflow(&format!("flow-graph-action-{action}"));
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let created = fixture.wait_created("s1").await;
        match action {
            "complete" => {
                fixture
                    .complete(&created.pid, &created.tid, Vars::new())
                    .await
            }
            "skip" => fixture.skip(&created.pid, &created.tid).await.unwrap(),
            "error" => fixture
                .fail(&created.pid, &created.tid, "rejected", "not approved")
                .await
                .unwrap(),
            "abort" => fixture.abort(&created.pid, &created.tid).await.unwrap(),
            other => panic!("unexpected action {other}"),
        }
        fixture.wait_state("s1", state).await;

        let graph = fixture
            .engine()
            .runtime()
            .graph(fixture.pid())
            .await
            .unwrap();
        assert_action_hierarchy(&graph);
        assert_eq!(fixture.state("s1").await, state, "{action}: act state");
        fixture.close().await;
    }
}

/// `transfer`/`delegate` only move the assignment (owner/assignee); the act's
/// execution and the graph's parent edges are untouched.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_through_assignment_actions() {
    for action in ["transfer", "delegate"] {
        let workflow = action_workflow(&format!("flow-graph-assign-{action}"));
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let created = fixture.wait_created("s1").await;
        let decider = fixture.engine().executor(&Principal::unrestricted());
        match action {
            "transfer" => decider
                .act()
                .transfer(&created.pid, &created.tid, Vars::new().with("to", "bob"))
                .await
                .unwrap(),
            "delegate" => decider
                .act()
                .delegate(
                    &created.pid,
                    &created.tid,
                    Vars::new().with("delegatee", "bob"),
                )
                .await
                .unwrap(),
            other => panic!("unexpected action {other}"),
        }
        assert_eq!(fixture.state("s1").await, TaskState::Waiting);

        let graph = fixture
            .engine()
            .runtime()
            .graph(fixture.pid())
            .await
            .unwrap();
        assert_action_hierarchy(&graph);
        fixture.close().await;
    }
}

/// `Push` creates a dynamic act under the act that created it, `Remove`
/// decides that instance `removed`, and `Remind` is a record on the owner's
/// act: none of them reparents the container's act or its children.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_through_push_remove_and_remind() {
    let workflow = action_workflow("flow-graph-push-remind");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    // Push an act the model does not declare: it becomes a child execution of
    // the act that created it.
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

    // A non-owner reminds the assigned act: the record folds onto the node.
    fixture
        .engine()
        .executor(&super::act::approver("carol"))
        .act()
        .remind(
            fixture.pid(),
            &created.tid,
            Vars::new().with("message", "hurry"),
        )
        .await
        .unwrap();

    // Remove the pushed act: the execution stays in the graph as `removed`.
    fixture
        .act(&pushed.pid, &pushed.tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();
    fixture.wait_state("s1#push1", TaskState::Removed).await;
    fixture
        .wait_message("s1#push1", MessageState::Removed)
        .await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    assert_action_hierarchy(&graph);

    let act = graph
        .nodes
        .iter()
        .find(|node| node.nid == "s1" && node.tid.is_some())
        .expect("the assigned act's execution");
    let pushed = graph
        .nodes
        .iter()
        .find(|node| node.nid == "s1#push1")
        .expect("the pushed act's execution");
    assert_eq!(pushed.parent.as_deref(), Some("s1"));
    assert_eq!(pushed.parent_tid, act.tid);
    assert_eq!(pushed.state.as_deref(), Some("removed"));
    let remind = act
        .messages
        .iter()
        .find(|message| {
            message.r#type == "notice" && message.uses.as_deref() != Some("acts.core.msg")
        })
        .expect("the remind record folded onto the assigned act");
    assert_eq!(remind.owner.as_deref(), Some("alice"));
    assert_eq!(remind.state, "created");
    fixture.close().await;
}

/// A single-owner `push` builds a sign chain in either mode: `before` runs
/// the added owner's leaf first, `after` runs the target owner's first. The
/// leaves are child executions of the act in both orders — the mode decides
/// which leaf is admitted first, not the hierarchy.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_through_both_push_modes() {
    for (mode, first_owner, second_owner) in [("before", "bob", "alice"), ("after", "alice", "bob")]
    {
        let workflow = action_workflow(&format!("flow-graph-push-{mode}"));
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let act = fixture.wait_created("s1").await;

        fixture
            .act(
                &act.pid,
                &act.tid,
                EventAction::Push,
                Vars::new()
                    .with("owners", json!(["bob"]).to_string())
                    .with("mode", mode),
            )
            .await
            .unwrap();
        let first = fixture.wait_created("s1#push1").await;

        let graph = fixture
            .engine()
            .runtime()
            .graph(fixture.pid())
            .await
            .unwrap();
        assert_action_hierarchy(&graph);
        let target = graph
            .nodes
            .iter()
            .find(|node| node.nid == "s1" && node.tid.is_some())
            .expect("the assigned act's execution");
        let first_leaf = graph
            .nodes
            .iter()
            .find(|node| node.nid == "s1#push1")
            .expect("the first sign leaf");
        assert_eq!(first_leaf.parent.as_deref(), Some("s1"));
        assert_eq!(first_leaf.parent_tid, target.tid);
        assert_eq!(
            first_leaf
                .messages
                .first()
                .and_then(|message| message.owner.as_deref()),
            Some(first_owner),
            "{mode}: the first leaf runs {first_owner} first"
        );

        // Removing the first leaf admits the next one under the same parent.
        fixture
            .act(&first.pid, &first.tid, EventAction::Remove, Vars::new())
            .await
            .unwrap();
        fixture.wait_state("s1#push1", TaskState::Removed).await;
        fixture.wait_created("s1#push2").await;

        let graph = fixture
            .engine()
            .runtime()
            .graph(fixture.pid())
            .await
            .unwrap();
        let second_leaf = graph
            .nodes
            .iter()
            .find(|node| node.nid == "s1#push2")
            .expect("the second sign leaf");
        assert_eq!(second_leaf.parent.as_deref(), Some("s1"));
        assert_eq!(second_leaf.parent_tid, target.tid);
        assert_eq!(
            second_leaf
                .messages
                .first()
                .and_then(|message| message.owner.as_deref()),
            Some(second_owner),
            "{mode}: the second leaf runs {second_owner}"
        );
        fixture.close().await;
    }
}

/// `Cancel` is the other rewind: it closes the finished container `Backed`
/// and re-enters it as a new generation. Both generations keep the container
/// and its act under their own execution — the closed round as it ran, the
/// re-entered one with the act's fresh instance behind it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_keeps_the_hierarchy_across_a_cancel() {
    let mut workflow = action_workflow("flow-graph-cancel");
    workflow.steps.push(
        Step::new()
            .with_id("tail")
            .with_uses(USES_IRQ, Vars::new().with("key", "tail")),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let act = fixture.wait_created("s1").await;

    // The container's execution is the cancel target once it has finished.
    let before = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    let approval = before
        .nodes
        .iter()
        .find(|node| node.nid == "approval")
        .expect("the container execution")
        .tid
        .clone()
        .expect("the container's tid");
    fixture.complete(&act.pid, &act.tid, Vars::new()).await;
    fixture.wait_created("tail").await;

    fixture.cancel(fixture.pid(), &approval).await.unwrap();
    // The re-entered container re-runs its act as a fresh instance.
    fixture.wait_created_except("s1", &act.tid).await;

    let graph = fixture
        .engine()
        .runtime()
        .graph(fixture.pid())
        .await
        .unwrap();
    let approvals: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.nid == "approval")
        .collect();
    assert_eq!(approvals.len(), 2, "the closed and the re-entered round");
    let closed = approvals
        .iter()
        .find(|node| node.state.as_deref() == Some("backed"))
        .expect("the cancelled generation is backed");
    let redo = approvals
        .iter()
        .find(|node| node.state.as_deref() != Some("backed"))
        .expect("the re-entered generation");
    let (closed_branch, closed_act) = act_under(&graph, closed);
    let (redo_branch, redo_act) = act_under(&graph, redo);
    assert_ne!(closed_branch.tid, redo_branch.tid);
    // The closed round kept the act's completed instance; the redo waits.
    assert_eq!(closed_act.state.as_deref(), Some("completed"));
    assert_eq!(redo_act.state.as_deref(), Some("interrupted"));
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

/// The message records fold onto their nodes: a sign chain's leaves carry
/// their own created/completed records by instance, the parked target reads
/// its absorbed answer, the remind record and the transferred task's
/// `acts.core.msg` announcement are on the node too.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_graph_folds_the_message_records_onto_their_nodes() {
    let mut workflow = Workflow::new()
        .with_id("flow-graph-fold")
        .with_name("Graph Fold")
        .with_var("reviewer", "alice");
    workflow.steps.push(
        Step::new()
            .with_id("s1")
            .with_name("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_owner("alice"),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    // `after` mode: the owner's leaf first, bob's behind it.
    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Push,
            Vars::new().with("owners", serde_json::json!(["bob"]).to_string()),
        )
        .await
        .unwrap();
    let first = fixture.wait_created("s1#push1").await;
    fixture.next(&first.pid, &first.tid).await.unwrap();
    let second = fixture.wait_created("s1#push2").await;

    // A non-owner reminds the parked act: the record lands on the node.
    fixture
        .engine()
        .executor(&super::act::approver("carol"))
        .act()
        .remind(
            fixture.pid(),
            &created.tid,
            Vars::new().with("message", "hurry"),
        )
        .await
        .unwrap();

    fixture.next(&second.pid, &second.tid).await.unwrap();
    fixture.wait_state("s1", TaskState::Completed).await;
    fixture.wait_completed().await;

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

    // The target: its created, the absorbed answer, the remind record
    // addressed to the owner, and the completion its join announces once the
    // pushed leaves settle.
    let s1 = node("s1");
    let types: Vec<&str> = s1.messages.iter().map(|m| m.r#type.as_str()).collect();
    assert_eq!(types, vec!["step", "step", "notice", "step"], "{s1:?}");
    assert_eq!(s1.messages[2].owner.as_deref(), Some("alice"));
    let remind = &s1.messages[2];
    assert_eq!(remind.state, "created");
    assert_eq!(s1.messages[3].state, "completed");

    // The chain's leaves read by instance, created before completed.
    let pushed = node("s1#push1");
    assert_eq!(
        pushed
            .messages
            .iter()
            .map(|m| m.state.as_str())
            .collect::<Vec<_>>(),
        vec!["created", "completed"]
    );
    assert_eq!(pushed.messages[0].owner.as_deref(), Some("alice"));

    // The tree annotates the notice counts on the node's own line; the act's
    // own lifecycle records are the step itself and get none.
    let tree = graph.tree_output();
    assert!(
        tree.contains("step id:s1 name=s1  uses=acts.core.irq state=completed next=nil notice=1"),
        "the node line carries its notice counts: {tree}"
    );
    assert!(
        tree.matches(" next=nil notice=1\n").count() == 1,
        "only the reminded node carries an annotation: {tree}"
    );
    fixture.close().await;
}
