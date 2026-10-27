//! Branch cells migrated from the pre-kernel scheduler suite (`branch/*`,
//! plus the branch cases of `act`/`message`): guard selection, the `else`
//! branch, several true branches, and the state a branch holds while its child
//! waits.

use serial_test::serial;

use crate::{Vars, Workflow, scheduler::core::model::task_graph::TaskState, utils::test::USES_IRQ};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

/// A branch's own `if` selects it; a branch with neither `if` nor `else` is
/// unselected and settles `Skipped` (the legacy `sch_task_branch_empty_if`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_without_a_guard_is_skipped() {
    let workflow = Workflow::new()
        .with_id("flow-branch-empty")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| branch.with_id("b1").with_step(|inner| inner.with_id("i1")))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Skipped);
    fixture.close().await;
}

/// The `else` branch runs when every guarded branch fails; a matching guard
/// runs and the `else` is skipped (the legacy `branch/if` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_guards_select_the_else_or_the_matching_branch() {
    // v = 1: `v < 0` fails, the else branch runs.
    let workflow = Workflow::new()
        .with_id("flow-branch-else")
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_else(true)
                        .with_step(|inner| inner.with_id("i1"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("v < 0")
                        .with_step(|inner| inner.with_id("i2"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);
    fixture.close().await;

    // v = 1: `v > 0` runs; the else branch is skipped.
    let workflow = Workflow::new()
        .with_id("flow-branch-if-else")
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|inner| inner.with_id("i1"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_else(true)
                        .with_step(|inner| inner.with_id("i2"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);
    fixture.close().await;
}

/// Two guarded branches without an `else`: exactly the matching one runs
/// (the legacy `sch_task_branch_if_two_no_else`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_mutually_exclusive_branch_guards_select_one_branch() {
    let workflow = Workflow::new()
        .with_id("flow-branch-two")
        .with_var("v", 1)
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|inner| inner.with_id("i1"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("v <= 0")
                        .with_step(|inner| inner.with_id("i2"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);
    fixture.close().await;
}

/// Every branch whose guard holds runs; a false guard is skipped even when
/// another branch already matched (the legacy `sch_task_branch_if_mutli_true`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_multiple_true_branch_guards_all_run() {
    let workflow = Workflow::new()
        .with_id("flow-branch-multi")
        .with_var("v", 5)
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|inner| inner.with_id("i1"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("v <= 0")
                        .with_step(|inner| inner.with_id("i2"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b3")
                        .with_if("v > 2")
                        .with_step(|inner| inner.with_id("i3"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);
    assert_eq!(fixture.state("b3").await, TaskState::Completed);
    fixture.close().await;
}

/// A selected branch whose child waits stays `Running` (the kernel parks a
/// container as running, not waiting); it completes once the child settles
/// (the legacy `sch_task_branch_if_false_else_running`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_stays_running_while_its_child_waits() {
    let workflow = Workflow::new()
        .with_id("flow-branch-running")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch.with_id("b1").with_else(true).with_step(|inner| {
                        inner
                            .with_id("i1")
                            .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
                    })
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("false")
                        .with_step(|inner| inner.with_id("i2"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("i1").await;
    assert_eq!(fixture.state("b1").await, TaskState::Running);
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    fixture.close().await;
}

/// A branch that `needs` a predecessor stays queued until the predecessor's
/// whole subtree settles, then runs its own steps (the legacy
/// `sch_task_branch_needs_state`, now without the refusal).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_needs_runs_after_its_predecessor_completes() {
    let workflow = Workflow::new()
        .with_id("flow-branch-needs")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch.with_id("b1").with_if("true").with_step(|inner| {
                        inner
                            .with_id("i1")
                            .with_uses(USES_IRQ, Vars::new().with("key", "first"))
                    })
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_if("true")
                        .with_need("b1")
                        .with_step(|inner| {
                            inner
                                .with_id("i2")
                                .with_uses(USES_IRQ, Vars::new().with("key", "second"))
                        })
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let first = fixture.wait_created("i1").await;
    assert_eq!(fixture.state("b1").await, TaskState::Running);
    assert_eq!(
        fixture.state("b2").await,
        TaskState::Created,
        "the dependent branch is queued behind b1"
    );
    assert!(
        fixture.messages_of("i2").is_empty(),
        "b2's steps must not have started"
    );

    fixture.next(&first.pid, &first.tid).await.unwrap();
    let second = fixture.wait_created("i2").await;
    assert_eq!(
        fixture.state("b1").await,
        TaskState::Completed,
        "b1 settled before b2 was admitted"
    );
    fixture.next(&second.pid, &second.tid).await.unwrap();
    completed(&fixture.wait_terminal().await);
    assert_eq!(fixture.state("b2").await, TaskState::Completed);
    fixture.close().await;
}

/// Every id in `needs` must settle: the dependent waits for both
/// predecessors, not just the first one.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_needs_waits_for_every_predecessor() {
    let workflow = Workflow::new()
        .with_id("flow-branch-needs-many")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch.with_id("b1").with_if("true").with_step(|inner| {
                        inner
                            .with_id("i1")
                            .with_uses(USES_IRQ, Vars::new().with("key", "one"))
                    })
                })
                .with_branch(|branch| {
                    branch.with_id("b2").with_if("true").with_step(|inner| {
                        inner
                            .with_id("i2")
                            .with_uses(USES_IRQ, Vars::new().with("key", "two"))
                    })
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b3")
                        .with_if("true")
                        .with_need("b1")
                        .with_need("b2")
                        .with_step(|inner| {
                            inner
                                .with_id("i3")
                                .with_uses(USES_IRQ, Vars::new().with("key", "three"))
                        })
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let first = fixture.wait_created("i1").await;
    let second = fixture.wait_created("i2").await;
    assert_eq!(fixture.state("b3").await, TaskState::Created);

    fixture.next(&first.pid, &first.tid).await.unwrap();
    fixture.wait_state("b1", TaskState::Completed).await;
    assert_eq!(
        fixture.state("b3").await,
        TaskState::Created,
        "one settled predecessor is not enough"
    );
    assert!(fixture.messages_of("i3").is_empty());

    fixture.next(&second.pid, &second.tid).await.unwrap();
    let third = fixture.wait_created("i3").await;
    fixture.next(&third.pid, &third.tid).await.unwrap();
    completed(&fixture.wait_terminal().await);
    assert_eq!(fixture.state("b3").await, TaskState::Completed);
    fixture.close().await;
}

/// A dependency may name a later-declared sibling, and a predecessor that its
/// own guard skipped still counts as settled: `needs` is ordering, not a
/// success chain.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_needs_accepts_forward_and_skipped_predecessors() {
    let workflow = Workflow::new()
        .with_id("flow-branch-needs-forward")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("true")
                        .with_need("b2")
                        .with_step(|inner| {
                            inner
                                .with_id("i1")
                                .with_uses(USES_IRQ, Vars::new().with("key", "after"))
                        })
                })
                .with_branch(|branch| {
                    branch.with_id("b2").with_if("false").with_step(|inner| {
                        inner
                            .with_id("i2")
                            .with_uses(USES_IRQ, Vars::new().with("key", "never"))
                    })
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("i1").await;
    assert_eq!(fixture.state("b2").await, TaskState::Skipped);
    assert_eq!(fixture.state("b1").await, TaskState::Running);
    fixture.next(&created.pid, &created.tid).await.unwrap();
    completed(&fixture.wait_terminal().await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert!(
        fixture.messages_of("i2").is_empty(),
        "the skipped predecessor's steps never run"
    );
    fixture.close().await;
}

/// An unguarded branch with `needs` is a stage of the ordered fan-out, not an
/// unselected branch: it runs after its predecessor (the docs' `needs` shape).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_needs_without_a_guard_is_selected() {
    let workflow = Workflow::new()
        .with_id("flow-branch-needs-unguarded")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_else(true)
                        .with_step(|inner| inner.with_id("i1"))
                })
                .with_branch(|branch| {
                    branch
                        .with_id("b2")
                        .with_need("b1")
                        .with_step(|inner| inner.with_id("i2"))
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    assert_eq!(fixture.state("b2").await, TaskState::Completed);
    fixture.close().await;
}
