//! `assign.candidates` cells: the candidate pool is business data, computed
//! locally when every atom of the expression resolves in the kernel, and
//! otherwise by the spec's `uses` package (default `acts.core.candidates`)
//! over one IRQ request the declaring act pushes under itself. The act's
//! start waits for that answer — the fan-out (`in: "${{ candidates }}"`) and
//! an owner that references the pool resolve on the re-entered start.

use super::*;
use crate::MessageState;
use crate::{
    Step, Vars, Workflow, scheduler::core::model::task_graph::TaskState, utils::consts,
    utils::test::USES_IRQ,
};
use serde_json::json;
use serial_test::serial;

const USES_CANDIDATES: &str = "acts.core.candidates";
const USES_PARALLEL: &str = "acts.core.parallel";

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
    assert_eq!(request.uses.as_deref(), Some(USES_CANDIDATES));
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
                .with_assign("${{ candidates[0] }}")
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
                                name: "u1".to_string(),
                            },
                            crate::model::CandidateExpr::User {
                                name: "u2".to_string(),
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
/// acts.core.candidates` — no `assign.candidates` — is a plain IRQ act: the
/// engine sends the params with the `${{ … }}` string already computed and
/// the answer lands as the act's own outputs, never as a locally folded pool.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_step_uses_package_answers_without_local_folding() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-step-uses")
        .with_step(|step| {
            let step = step.with_id("pool").with_uses(USES_CANDIDATES, Vars::new());
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
                .with_assign("${{ candidates[0] }}")
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

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner.as_deref(), Some("u9"));
    fixture.close().await;
}

/// A generated child act reads the pool of the container that generated it:
/// `$assign.candidates` resolves the nearest computed pool along the scope
/// chain, and `$assign.at("<step-id>")` names an execution explicitly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_candidates_child_reads_the_parent_pool_through_assign() {
    let workflow = Workflow::new()
        .with_id("sch-candidates-assign")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(
                    USES_PARALLEL,
                    Vars::new().with("in", "${{ $assign.candidates }}").with(
                        "steps",
                        vec![
                            Step::new().with_id("vote").with_uses(
                                USES_IRQ,
                                Vars::new()
                                    .with("value", "${{ $item.value }}")
                                    .with("pool", "${{ $assign.candidates }}")
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

    // `in: "${{ $assign.candidates }}"` resolves the container's own pool.
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
                    .with("pool", "${{ $assign.candidates }}")
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
