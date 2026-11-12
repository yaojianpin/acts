//! `assign.candidates` cells: the candidate pool is business data, computed
//! locally when every atom of the expression resolves in the kernel, and
//! otherwise by the spec's `uses` package (default `acts.core.users`)
//! over one IRQ request the declaring act pushes under itself. The act's
//! start waits for that answer — the fan-out (`in: "${{ candidates }}"`) and
//! an owner that references the pool resolve on the re-entered start.

use super::*;
use crate::{MessageState, Principal};
use crate::{
    Step, Vars, Workflow, scheduler::core::model::task_graph::TaskState, utils::consts,
    utils::test::USES_BLOCK, utils::test::USES_IRQ,
};
use serde_json::json;
use serial_test::serial;

const USES_USERS: &str = "acts.core.users";
const USES_PARALLEL: &str = "acts.core.parallel";

/// A container speaks at the pool wait: its act `Created` rides the wait
/// itself, carrying the raw declared params (the pool has not resolved), and
/// the answer never re-announces — the fan-out and the join proceed
/// unchanged. Every declared node announces this way; there is no opt-out.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_announce_speaks_at_the_pool_wait() {
    let workflow =
        Workflow::new()
            .with_id("sch-candidates-announce")
            .with_step(|step| {
                step.with_id("approve")
                    .with_uses(
                        USES_PARALLEL,
                        Vars::new().with("in", "${{ candidates }}").with(
                            "steps",
                            vec![Step::new().with_id("vote").with_uses(
                                USES_IRQ,
                                Vars::new().with("value", "${{ $item.value }}"),
                            )],
                        ),
                    )
                    .with_candidates(crate::model::CandidateExpr::Role {
                        name: "reviewers".to_string(),
                    })
            });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The container announces Created while its pool is still pending: the
    // payload carries the raw declaration, not a resolution of it.
    let container = fixture.wait_created("approve").await;
    let params = container.params().expect("the declared params");
    assert_eq!(
        params.get::<String>("in").as_deref(),
        Some("${{ candidates }}")
    );

    // The pool resolves, one vote fans out, and the act completes; the
    // announcement stays a single Created.
    let request = fixture.wait_created("approve#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([{ "id": "u1", "code": "alice", "name": "Alice" }]),
                )
                .with("ids", json!(["u1"]))
                .with("count", 1),
        )
        .await;
    let vote = fixture.wait_created_uses(USES_IRQ, 1).await.remove(0);
    fixture.complete(&vote.pid, &vote.tid, Vars::new()).await;
    fixture.wait_state("approve", TaskState::Completed).await;
    fixture.wait_completed().await;
    assert_eq!(
        fixture.announced("approve"),
        vec![MessageState::Created, MessageState::Completed],
        "the announce step speaks once at the wait, once at its completion"
    );
    fixture.close().await;
}

/// The approve demo's `department_manager_approval` shape: the client sees
/// exactly the declared stream — the container `Created`, its pool request
/// `Created`/`Completed`, the vote `Created` — with no kernel wrapper
/// (`<step>#item`, its virtual branches) announcing and no container
/// `Completed` before the vote settles it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_announce_container_speaks_only_the_declared_shape() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-declared-shape")
        .with_step(|step| {
            step.with_id("department_manager_approval")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new()
                        .with("in", "${{ $assign.candidates.ids }}")
                        .with("join", "any")
                        .with(
                            "steps",
                            vec![
                                Step::new()
                                    .with_id("department_manager_vote")
                                    .with_uses(
                                        USES_IRQ,
                                        Vars::new().with("key", "department_manager_vote"),
                                    )
                                    .with_owner("${{ $item.value }}"),
                            ],
                        ),
                )
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "manager".to_string(),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The container announces at the wait; the pool request is answered and
    // fans the template out to one vote.
    fixture.wait_created("department_manager_approval").await;
    let request = fixture
        .wait_created("department_manager_approval#cand1")
        .await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([{ "id": "101", "code": "101", "name": "Eve Chen" }]),
                )
                .with("ids", json!(["101"]))
                .with("count", 1),
        )
        .await;
    fixture
        .wait_message("department_manager_approval#cand1", MessageState::Completed)
        .await;
    fixture.wait_created("department_manager_vote").await;

    // Only the declared nodes spoke: the container and its pool request and
    // the vote. The materialized template (`#item`) and its virtual branches
    // (`#in`, `#item#act0`) are kernel-internal and silent, and the container
    // is still running for its vote.
    let trail: Vec<(String, MessageState)> = fixture
        .messages()
        .into_iter()
        .filter(|message| {
            message.nid == "department_manager_approval"
                || message.nid.starts_with("department_manager_approval#")
                || message.nid == "department_manager_vote"
        })
        .map(|message| (message.nid, message.state))
        .collect();
    assert_eq!(
        trail,
        vec![
            (
                "department_manager_approval".to_string(),
                MessageState::Created
            ),
            (
                "department_manager_approval#cand1".to_string(),
                MessageState::Created
            ),
            (
                "department_manager_approval#cand1".to_string(),
                MessageState::Completed
            ),
            ("department_manager_vote".to_string(), MessageState::Created),
        ],
        "only the declared nodes announce"
    );
    assert!(
        fixture
            .messages()
            .iter()
            .all(|message| message.r#type != "block"),
        "a virtual wrapper must not announce: {:?}",
        fixture.messages()
    );
    assert_eq!(
        fixture.announced("department_manager_approval"),
        vec![MessageState::Created],
        "the container is running for its vote, not completed"
    );

    // The vote settles the join; the container's completion speaks now.
    let vote = fixture.wait_created("department_manager_vote").await;
    fixture.complete(&vote.pid, &vote.tid, Vars::new()).await;
    fixture
        .wait_state("department_manager_approval", TaskState::Completed)
        .await;
    fixture.wait_completed().await;
    assert_eq!(
        fixture.announced("department_manager_approval"),
        vec![MessageState::Created, MessageState::Completed]
    );
    fixture.close().await;
}

/// The example's shape: the container announces at the pool wait and the
/// client records its tid; the pool answers, the fan-out runs and the vote
/// starts at once — it inherits the pool's computed result through the scope
/// chain, so no `#cand` request rides under it — and joins the pushed act
/// before the container finishes.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_push_at_the_announce_wait_fans_out_after_the_answer() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-push-wait")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new()
                        .with("in", "${{ $assign.candidates.ids }}")
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

    // The announcement rides the wait: the step speaks before its pool
    // resolves, and nothing announces the vote yet.
    let container = fixture.wait_created("approve").await;
    assert!(fixture.message("vote", MessageState::Created).is_none());

    // The pool answers; the fan-out runs and the vote starts at once — the
    // container's pool is never re-asked under the vote, whose owner
    // resolves from the item on the scope chain.
    let request = fixture.wait_created("approve#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([{ "id": "u1", "code": "alice", "name": "Alice" }]),
                )
                .with("ids", json!(["u1"]))
                .with("count", 1),
        )
        .await;
    let vote = fixture.wait_created_uses(USES_IRQ, 1).await.remove(0);

    // Every vote is in: the classic push lands on the step's recorded tid,
    // and the join: all waits for the added approver as much as for the
    // vote.
    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .act()
        .push(
            fixture.pid(),
            &container.tid,
            Vars::new()
                .with("uses", USES_IRQ)
                .with("params", Vars::new().with("key", "added")),
        )
        .await
        .expect("the classic push signs the container after the fan-out");
    let added = fixture.wait_created("approve#push1").await;
    fixture.complete(&vote.pid, &vote.tid, Vars::new()).await;
    fixture.complete(&added.pid, &added.tid, Vars::new()).await;
    fixture.wait_state("approve", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// The container's pool is asked exactly once, under the container. The
/// fan-out items inherit the pool's *computed result* through the scope
/// chain, so an owned vote starts the moment the pool answers — its owner
/// resolves from its item — and no `#cand` request ever rides under a vote.
/// (The regression for the per-item re-ask the approve demo showed: every
/// `pm_vote` minted its own `<vote>#cand<N>` because the child inherited the
/// container's declaration instead of its answer.)
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_container_pool_is_asked_once_and_the_item_votes_never_re_ask() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-single-ask")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new()
                        .with("in", "${{ $assign.candidates.ids }}")
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

    // The container asks its pool; the answer fans the template out.
    let request = fixture.wait_created("approve#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([
                        { "id": "u1", "code": "alice", "name": "Alice" },
                        { "id": "u2", "code": "bob", "name": "Bob" }
                    ]),
                )
                .with("ids", json!(["u1", "u2"]))
                .with("count", 2),
        )
        .await;

    // Both votes start at once, each owned by its item's value, and no
    // `#cand` request exists under either — the pool's answer is inherited,
    // not re-asked.
    let votes = fixture.wait_created_uses(USES_IRQ, 2).await;
    let mut owners: Vec<_> = votes.iter().map(|vote| vote.owner.clone()).collect();
    owners.sort();
    assert_eq!(
        owners,
        vec![Some("u1".to_string()), Some("u2".to_string())],
        "each vote's owner is its item value, resolved without a pool ask"
    );
    for vote in &votes {
        assert_eq!(vote.nid, "vote");
        assert!(
            fixture
                .message("vote#cand1", MessageState::Created)
                .is_none()
                && fixture
                    .message("vote#cand2", MessageState::Created)
                    .is_none(),
            "the container's pool is never re-asked under a vote"
        );
    }
    fixture.close().await;
}

/// The declaring container waits for the pool, then fans it out: one owner
/// act per candidate, the request answered by the business system.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_pool_fans_out_parallel_owners() {
    let workflow =
        Workflow::new()
            .with_id("sch-candidates-parallel")
            .with_step(|step| {
                step.with_id("approve")
                    .with_uses(
                        USES_PARALLEL,
                        Vars::new().with("in", "${{ candidates }}").with(
                            "steps",
                            vec![Step::new().with_id("vote").with_uses(
                                USES_IRQ,
                                Vars::new().with("value", "${{ $item.value }}"),
                            )],
                        ),
                    )
                    .with_candidates(crate::model::CandidateExpr::Role {
                        name: "reviewers".to_string(),
                    })
            });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The pool request rides the uses package, under the declaring act, and
    // carries the expression document.
    let request = fixture.wait_created("approve#cand1").await;
    assert_eq!(request.uses.as_deref(), Some(USES_USERS));
    let params = request.params().expect("the candidate expression");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("role"));
    assert_eq!(params.get::<String>("name").as_deref(), Some("reviewers"));

    // The business system answers with its own directory's result.
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([
                        { "id": "u1", "code": "alice", "name": "Alice" },
                        { "id": "u2", "code": "bob", "name": "Bob" }
                    ]),
                )
                .with("ids", json!(["u1", "u2"]))
                .with("count", 2),
        )
        .await;

    // The container fans the pool out: one owner act per candidate.
    let mut values: Vec<String> = fixture
        .wait_created_uses(USES_IRQ, 2)
        .await
        .into_iter()
        .filter_map(|message| {
            message
                .params()
                .and_then(|params| params.get::<String>("value"))
        })
        .collect();
    values.sort();
    assert_eq!(values, vec!["u1".to_string(), "u2".to_string()]);
    fixture.close().await;
}

/// The package's local half computes in the kernel: a context atom the engine
/// owns (`initiator`) never produces a request — the pool lands locally and
/// the owner reads it without a round trip.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_context_atoms_compute_locally() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-context")
        .with_rn("sch-candidates-context")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_owner("${{ candidates[0] }}")
                .with_candidates(crate::model::CandidateExpr::Context {
                    var: crate::model::ContextVar::Initiator,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let starter = crate::Principal::from_policy(&crate::UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["sch-candidates-context".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture
        .start_as(&starter, Vars::new().with("actor", "starter"))
        .await
        .unwrap();

    // The act starts directly — no `#cand` request, no client answer — and
    // the owner read the locally computed pool.
    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner.as_deref(), Some("starter"));
    fixture.close().await;
}

/// A candidates expression whose every atom is local computes in the kernel:
/// no request execution, no client answer — the pool lands and the container
/// fans it out.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_local_expression_computes_without_a_request() {
    let workflow =
        Workflow::new()
            .with_id("sch-candidates-local")
            .with_step(|step| {
                step.with_id("approve")
                    .with_uses(
                        USES_PARALLEL,
                        Vars::new().with("in", "${{ candidates }}").with(
                            "steps",
                            vec![Step::new().with_id("vote").with_uses(
                                USES_IRQ,
                                Vars::new().with("value", "${{ $item.value }}"),
                            )],
                        ),
                    )
                    .with_candidates(crate::model::CandidateExpr::AnyOf {
                        operands: vec![
                            crate::model::CandidateExpr::User {
                                value: "u1".to_string(),
                            },
                            crate::model::CandidateExpr::User {
                                value: "u2".to_string(),
                            },
                        ],
                    })
            });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // No client answered anything (the votes' `value` can only come from a
    // locally computed pool): the kernel computed the pool itself.
    let mut values: Vec<String> = fixture
        .wait_created_uses(USES_IRQ, 2)
        .await
        .into_iter()
        .filter_map(|message| {
            message
                .params()
                .and_then(|params| params.get::<String>("value"))
        })
        .collect();
    values.sort();
    assert_eq!(values, vec!["u1".to_string(), "u2".to_string()]);
    fixture.close().await;
}

/// A business atom defers to the spec's `uses` package — and the request it
/// sends carries the locally computed parts: the `${{ … }}` string operand
/// arrives as the users it resolved to, while the business atom stays for the
/// client.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_prepares_expression_strings_for_the_business_system() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-strings")
        .with_rn("sch-candidates-strings")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_candidates(crate::model::CandidateExpr::AnyOf {
                    operands: vec![
                        crate::model::CandidateExpr::Value("${{ $initiator }}".to_string()),
                        crate::model::CandidateExpr::Role {
                            name: "reviewers".to_string(),
                        },
                    ],
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let starter = crate::Principal::from_policy(&crate::UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["sch-candidates-strings".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture
        .start_as(&starter, Vars::new().with("actor", "starter"))
        .await
        .unwrap();

    let request = fixture.wait_created("s1#cand1").await;
    let params = request.params().expect("the prepared expression");
    assert_eq!(
        params.get::<serde_json::Value>("operands"),
        Some(json!([
            { "kind": "user", "name": "starter" },
            { "kind": "role", "name": "reviewers" }
        ])),
        "the string operand computes locally; the business atom stays"
    );
    fixture.close().await;
}

/// An all-local candidates expression the scope cannot answer fails the act
/// at start: the engine already knows the value is absent, so it invents
/// neither an empty pool nor a request, and the failure names the expression.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_unresolvable_expression_fails_the_act() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-unresolved")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_candidates(crate::model::CandidateExpr::Value(
                    "${{ no_such_var }}".to_string(),
                ))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let failed = fixture.wait_message("s1", MessageState::Error).await;
    let message = failed
        .outputs
        .get::<String>(consts::ACT_ERR_MESSAGE)
        .unwrap_or_default();
    assert!(
        message.contains("no_such_var"),
        "the failure must name the expression: {message}"
    );
    fixture.wait_state("s1", TaskState::Failed).await;
    fixture.wait_error().await;
    fixture.close().await;
}

/// A step that computes its pool through `step.uses =
/// acts.core.users` — no `assign.candidates` — is a plain IRQ act: the
/// engine sends the params with the `${{ … }}` string already computed and
/// the answer lands as the act's own outputs, never as a locally folded pool.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_step_uses_package_answers_without_local_folding() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-step-uses")
        .with_step(|step| {
            let step = step.with_id("pool").with_uses(USES_USERS, Vars::new());
            let mut step = step;
            step.params = json!("${{ owner_var }}");
            step
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new().with("owner_var", "u9")).await;

    // The act itself is the request; the params carry the computed user.
    let created = fixture.wait_created("pool").await;
    let params = created.params().expect("the prepared params");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("user"));
    assert_eq!(params.get::<String>("name").as_deref(), Some("u9"));

    // The answer is the act's own outputs — no local pool folded it.
    fixture
        .complete(
            &created.pid,
            &created.tid,
            Vars::new().with("ids", json!(["u9"])).with("count", 1),
        )
        .await;
    let completed = fixture.wait_message("pool", MessageState::Completed).await;
    assert_eq!(
        completed.outputs.get::<serde_json::Value>("ids"),
        Some(json!(["u9"]))
    );
    fixture.close().await;
}

/// An owner may reference the computed pool: the act resolves its single
/// owner from the answer on the re-entered start.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_owner_resolves_from_the_pool() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-owner")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_owner("${{ candidates[0] }}")
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "reviewers".to_string(),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let request = fixture.wait_created("s1#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new().with("ids", json!(["u9"])).with("count", 1),
        )
        .await;

    // The `Created` the client saw spoke at the pool wait — before the pool
    // resolved, so without the owner. The owner materializes with the
    // act's own completion (the re-entered start resolved and persisted it).
    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner, None);
    fixture
        .complete(&created.pid, &created.tid, Vars::new())
        .await;
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("u9"));
    fixture.close().await;
}

/// A generated child act reads the pool of the container that generated it:
/// `$assign.candidates.ids` resolves the nearest computed pool along the
/// scope chain, and `$assign.at("<step-id>")` names an execution explicitly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_child_reads_the_parent_pool_through_assign() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-assign")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new()
                        .with("in", "${{ $assign.candidates.ids }}")
                        .with(
                            "steps",
                            vec![
                                Step::new().with_id("vote").with_uses(
                                    USES_IRQ,
                                    Vars::new()
                                        .with("value", "${{ $item.value }}")
                                        .with("pool", "${{ $assign.candidates.ids }}")
                                        .with("explicit", "${{ $assign.at('approve').ids }}"),
                                ),
                            ],
                        ),
                )
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "reviewers".to_string(),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // `in: "${{ $assign.candidates.ids }}"` resolves the container's own pool.
    let request = fixture.wait_created("approve#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with("ids", json!(["u1", "u2"]))
                .with("count", 2),
        )
        .await;

    let created = fixture.wait_created_uses(USES_IRQ, 2).await;
    let mut values = Vec::new();
    for message in &created {
        let params = message.params().expect("the resolved child params");
        values.push(params.get::<String>("value").expect("the item value"));
        assert_eq!(
            params.get::<serde_json::Value>("pool"),
            Some(json!(["u1", "u2"])),
            "the child reads the pool that generated it"
        );
        assert_eq!(
            params.get::<serde_json::Value>("explicit"),
            Some(json!(["u1", "u2"])),
            "$assign.at names the parent execution"
        );
    }
    values.sort();
    assert_eq!(values, vec!["u1".to_string(), "u2".to_string()]);
    fixture.close().await;
}

/// A flow without any assign fact still answers `$assign`: null identities
/// and an empty pool, never an unknown-variable error.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_assign_without_facts_answers_an_empty_pool() {
    let workflow = Workflow::new()
        .with_id("sch-assign-empty")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                USES_IRQ,
                Vars::new()
                    .with("pool", "${{ $assign.candidates.ids }}")
                    .with("owner", "${{ $assign.owner }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    let params = created.params().expect("the resolved params");
    assert_eq!(params.get::<serde_json::Value>("pool"), Some(json!([])));
    assert_eq!(params.get::<serde_json::Value>("owner"), Some(json!(null)));
    fixture.close().await;
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_failure_fails_the_act() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-failure")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "reviewers".to_string(),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let request = fixture.wait_created("s1#cand1").await;
    fixture
        .fail(&request.pid, &request.tid, "directory_down", "unreachable")
        .await
        .unwrap();

    // The walk decides the waiting act Failed and carries the request's
    // error into its variables; the run ends errored.
    fixture.wait_state("s1", TaskState::Failed).await;
    fixture.wait_error().await;
    fixture.close().await;
}

/// A named pool processes exactly like `candidates`: an all-local key
/// computes in the kernel without a request, a business key rides its own
/// `#cand` request, and each answer lands under its own key
/// (`$assign.<key>.ids` / `.users`) next to `$assign.candidates`.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_named_pools_land_under_their_keys() {
    let workflow = Workflow::new()
        .with_id("sch-assign-pools")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new().with("in", "${{ candidates }}").with(
                        "steps",
                        vec![
                            Step::new().with_id("vote").with_uses(
                                USES_IRQ,
                                Vars::new()
                                    .with("value", "${{ $item.value }}")
                                    .with("cc", "${{ $assign.cc.ids }}")
                                    .with("cc_users", "${{ $assign.cc.users }}"),
                            ),
                        ],
                    ),
                )
                .with_candidates(crate::model::CandidateExpr::Role {
                    name: "reviewers".to_string(),
                })
                // A bare string is the pool's expression: one literal user,
                // computed locally.
                .with_assign("cc", crate::model::CandidateExpr::from("u9"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The canonical pool is business data: one request for it, none for the
    // local named pool.
    let request = fixture.wait_created("approve#cand1").await;
    fixture
        .complete(
            &request.pid,
            &request.tid,
            Vars::new()
                .with("ids", json!(["u1", "u2"]))
                .with("count", 2),
        )
        .await;

    // The re-entered start folds the local named pool and fans the canonical
    // one out; every generated child reads the named pool under its own key.
    let created = fixture.wait_created_uses(USES_IRQ, 2).await;
    for message in &created {
        let params = message.params().expect("the resolved child params");
        assert_eq!(
            params.get::<serde_json::Value>("cc"),
            Some(json!(["u9"])),
            "the named pool lands under its own key"
        );
        assert_eq!(
            params.get::<serde_json::Value>("cc_users"),
            Some(json!([{ "id": "u9", "code": "u9", "name": "u9" }])),
            "users carries the participant records"
        );
    }
    fixture.close().await;
}

/// A minimal IRQ package a test engine registers, so a pool may name a
/// `uses` package of its own.
#[derive(Clone)]
struct CorpDirectoryPackage;

#[async_trait::async_trait]
impl crate::ActPackage for CorpDirectoryPackage {
    fn new(_config: &crate::Config) -> crate::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    fn definition() -> crate::ActPackageDefinition
    where
        Self: Sized,
    {
        crate::ActPackageDefinition {
            id: "corp.directory",
            name: "CorpDirectory",
            desc: "test computing package",
            version: "0.1.0",
            icon: "",
            doc: "",
            schema: serde_json::json!({}),
            options: None,
            run_as: crate::ActRunAs::Irq,
            resources: Vec::new(),
            catalog: crate::ActPackageCatalog::Core,
        }
    }
}

/// `assign.<key>.uses` is per-pool: the default computing package is
/// `acts.core.users`, and a pool may name its own. Each business pool rides
/// its own `#cand` request, and the answers land under their keys.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_named_pools_use_their_own_packages() {
    let workflow = Workflow::new()
        .with_id("sch-assign-pool-uses")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(
                    USES_IRQ,
                    Vars::new()
                        .with("cc", "${{ $assign.cc.ids }}")
                        .with("approvers", "${{ $assign.approvers.ids }}"),
                )
                // Business atoms: both pools defer to their computing
                // packages. The default `uses` is `acts.core.users`; the
                // second pool names its own.
                .with_assign(
                    "cc",
                    crate::model::CandidateExpr::Role {
                        name: "reviewers".to_string(),
                    },
                )
                .with_assign_package(
                    "approvers",
                    "corp.directory",
                    crate::model::CandidateExpr::Role {
                        name: "approvers".to_string(),
                    },
                )
        });
    let mut fixture = {
        let engine = crate::Engine::builder()
            .add_package::<CorpDirectoryPackage>()
            .start()
            .await
            .unwrap();
        FlowFixture::deploy_on(engine, &workflow).await
    };
    fixture.start(Vars::new()).await;

    // Two requests, one per pool, in declaration (map) order: the custom
    // package first, then the default one.
    let approvers = fixture.wait_created("s1#cand1").await;
    assert_eq!(approvers.uses.as_deref(), Some("corp.directory"));
    let params = approvers.params().expect("the approvers expression");
    assert_eq!(params.get::<String>("name").as_deref(), Some("approvers"));
    fixture
        .complete(
            &approvers.pid,
            &approvers.tid,
            Vars::new().with("ids", json!(["a1"])).with("count", 1),
        )
        .await;

    let cc = fixture.wait_created("s1#cand2").await;
    assert_eq!(cc.uses.as_deref(), Some(USES_USERS));
    let params = cc.params().expect("the reviewers expression");
    assert_eq!(params.get::<String>("name").as_deref(), Some("reviewers"));
    fixture
        .complete(
            &cc.pid,
            &cc.tid,
            Vars::new().with("ids", json!(["u1"])).with("count", 1),
        )
        .await;

    // The `Created` the client saw spoke at the first pool wait, with the
    // raw declarations; the resolution shows on the act's completed message
    // (the re-entered start pinned the resolved facts). The `irq` act still
    // waits for its client, so the test decides it once here.
    let created = fixture.wait_created("s1").await;
    let params = created.params().expect("the declared params");
    assert_eq!(
        params.get::<String>("cc").as_deref(),
        Some("${{ $assign.cc.ids }}")
    );
    fixture
        .complete(&created.pid, &created.tid, Vars::new())
        .await;
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    let params = completed.params().expect("the resolved params");
    assert_eq!(params.get::<serde_json::Value>("cc"), Some(json!(["u1"])));
    assert_eq!(
        params.get::<serde_json::Value>("approvers"),
        Some(json!(["a1"]))
    );
    fixture.close().await;
}

/// A container's `assign.owner` is a template, not the container's own
/// assignment: the generated children that declare no owner of their own
/// inherit it — each resolving the expression in its own scope at its
/// start — while a child with its own `assign.owner` keeps its own. The
/// inherited owner rides the child's act and its `Created` message.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_container_owner_is_inherited_by_ownerless_children() {
    let workflow = Workflow::new()
        .with_id("sch-assign-inherit")
        .with_rn("sch-assign-inherit")
        .with_step(|step| {
            step.with_id("task")
                .with_uses(
                    USES_BLOCK,
                    Vars::new().with("mode", "parallel").with(
                        "steps",
                        vec![
                            // No own assign: inherits the container's owner.
                            Step::new()
                                .with_id("a1")
                                .with_uses(USES_IRQ, Vars::new().with("k", "inherited")),
                            // Own assign wins over the container template.
                            Step::new()
                                .with_id("a2")
                                .with_uses(USES_IRQ, Vars::new().with("k", "own"))
                                .with_owner("${{ $initiator }}"),
                        ],
                    ),
                )
                .with_owner("${{ owner_var }}")
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let starter = crate::Principal::from_policy(&crate::UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["sch-assign-inherit".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture
        .start_as(
            &starter,
            Vars::new()
                .with("actor", "starter")
                .with("owner_var", "boss"),
        )
        .await
        .unwrap();

    let inherited = fixture.wait_created("a1").await;
    assert_eq!(
        inherited.owner.as_deref(),
        Some("boss"),
        "the ownerless child inherits the container's owner"
    );
    let own = fixture.wait_created("a2").await;
    assert_eq!(
        own.owner.as_deref(),
        Some("starter"),
        "the child's own owner wins over the container template"
    );
    fixture.close().await;
}

/// A container's pools are its own: the container computes them (locally
/// here — every atom is a literal) and its children inherit the computed
/// result through the scope chain. A key a child declares itself is
/// computed in the child's own scope and wins over the container's answer
/// for that child. No child ever re-asks the container's declaration.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_container_pools_are_inherited_by_children() {
    let workflow = Workflow::new()
        .with_id("sch-assign-pool-inherit")
        .with_rn("sch-assign-pool-inherit")
        .with_step(|step| {
            step.with_id("task")
                .with_uses(
                    USES_BLOCK,
                    Vars::new().with("mode", "parallel").with(
                        "steps",
                        vec![
                            // No own assign: inherits both keys.
                            Step::new().with_id("a1").with_uses(
                                USES_IRQ,
                                Vars::new()
                                    .with("k", "a1")
                                    .with("pool", "${{ $assign.candidates.ids }}")
                                    .with("cc", "${{ $assign.cc.ids }}"),
                            ),
                            // Own candidates wins; cc is still inherited.
                            Step::new()
                                .with_id("a2")
                                .with_uses(
                                    USES_IRQ,
                                    Vars::new()
                                        .with("k", "a2")
                                        .with("pool", "${{ $assign.candidates.ids }}")
                                        .with("cc", "${{ $assign.cc.ids }}"),
                                )
                                .with_candidates(crate::model::CandidateExpr::from("u2")),
                            // Own cc wins; candidates is still inherited.
                            Step::new()
                                .with_id("a3")
                                .with_uses(
                                    USES_IRQ,
                                    Vars::new()
                                        .with("k", "a3")
                                        .with("pool", "${{ $assign.candidates.ids }}")
                                        .with("cc", "${{ $assign.cc.ids }}"),
                                )
                                .with_assign("cc", crate::model::CandidateExpr::from("u8")),
                        ],
                    ),
                )
                .with_candidates(crate::model::CandidateExpr::from("u1"))
                .with_assign("cc", crate::model::CandidateExpr::from("u9"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let expected = [
        ("a1", json!(["u1"]), json!(["u9"])),
        ("a2", json!(["u2"]), json!(["u9"])),
        ("a3", json!(["u1"]), json!(["u8"])),
    ];
    for (node, pool, cc) in expected {
        let created = fixture.wait_created(node).await;
        let params = created.params().expect("the resolved child params");
        assert_eq!(
            params.get::<serde_json::Value>("pool"),
            Some(pool),
            "{node} candidates"
        );
        assert_eq!(params.get::<serde_json::Value>("cc"), Some(cc), "{node} cc");
    }
    // The container's declarations never cross into its children: no child
    // mints a `#cand` request of its own, whatever assign it declares.
    for node in ["a1", "a2", "a3"] {
        assert!(
            fixture
                .message(&format!("{node}#cand1"), MessageState::Created)
                .is_none(),
            "{node} never re-asks the container's pool"
        );
    }
    fixture.close().await;
}
