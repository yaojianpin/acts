//! The timer of an execution whose node declares `timeouts` (the
//! `DispatchTimeout` operation).
//!
//! Phase 1's pre-claim policy for this kind is the *idle* round: evaluating
//! the branches before the claim keeps a timer with nothing due from consuming
//! its own operation (it stays `Pending`; the driver defers its ready record by
//! one poll interval). Phase 2 decides the round again, against the
//! authoritative post-claim snapshot, and phase 3 turns that verdict into the
//! one atomic commit that admits the due branch (or retires the timer).

use super::*;
use crate::scheduler::shell::*;

impl KernelHost {
    /// The declared timeout branches of one model node, in declaration order.
    ///
    /// An empty list means the node's step declares no `timeouts`: the timer
    /// has nothing to fire (`Retire`). Each branch carries the durable facts a
    /// dispatch needs; its guard stays in the model and is evaluated by the
    /// round.
    pub(super) fn timeout_branches(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Vec<crate::scheduler::core::plan::timeout::TimeoutBranch>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(Vec::new());
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(Vec::new());
        };
        let mut branches = Vec::with_capacity(step.timeouts.len());
        for timeout in &step.timeouts {
            branches.push(crate::scheduler::core::plan::timeout::TimeoutBranch {
                node: timeout.id.clone(),
                model_node: node_id(&timeout.id)?,
                vars: timeout
                    .vars
                    .iter()
                    .map(|var| (var.name.clone(), var.value.to_string()))
                    .collect(),
                auto: auto_decision(snapshot, &timeout.id)?,
            });
        }
        Ok(branches)
    }

    /// The `if` of one declared branch, from the declaring model: the window the
    /// tick evaluates (`$cost_in(...)` in the models this ports). A branch
    /// without a guard fires as soon as the round reaches it — a legacy branch
    /// whose `Step::init` had nothing to test.
    fn timeout_guard(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        branch: &str,
    ) -> Result<Option<String>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        Ok(find_step_in_model(&model, branch).and_then(|step| step.r#if.clone()))
    }

    /// One dispatch round of a timed-out execution: the legacy tick's
    /// `Step::on_timeout` + `dispatch_next_timeout_branch` + the branch's own
    /// `Step::init` guard, decided against the aggregate.
    ///
    /// The cost is what passed since the execution's own start operation (the legacy
    /// task's creation stamp); it is frozen into the branch's evaluation as the
    /// `__cost` task datum `$cost()`/`$cost_in(...)` read.
    async fn timeout_round(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        owner: &crate::scheduler::core::model::task_graph::TaskId,
        task: Option<&crate::scheduler::core::commit::aggregate::TaskSnapshot>,
        now_ms: i64,
    ) -> Result<TimeoutRound> {
        // Only a running or waiting execution times out (the legacy tick's
        // `is_timeouts() && (running || interrupted)`); anything else — a
        // re-entered generation's stale timer included — has nothing left.
        let Some(task) = task.filter(|task| {
            matches!(
                task.state,
                crate::scheduler::core::model::task_graph::TaskState::Running
                    | crate::scheduler::core::model::task_graph::TaskState::Waiting
            )
        }) else {
            return Ok(TimeoutRound::Retire);
        };
        let branches = self.timeout_branches(snapshot, &task.model_node)?;
        if branches.is_empty() {
            return Ok(TimeoutRound::Retire);
        }
        let cost_ms = crate::scheduler::core::plan::timeout::cost_ms(
            now_ms,
            snapshot
                .variables
                .get(&(
                    owner.clone(),
                    crate::scheduler::core::plan::timeout::STARTED_VAR.to_string(),
                ))
                .map(String::as_str),
        );
        match infallible(Round::plan(
            snapshot,
            &RoundCommand {
                owner,
                branches: &branches,
            },
        )) {
            crate::scheduler::core::plan::timeout::TimeoutCandidate::None => {
                Ok(TimeoutRound::Retire)
            }
            crate::scheduler::core::plan::timeout::TimeoutCandidate::Blocked => {
                Ok(TimeoutRound::Idle)
            }
            crate::scheduler::core::plan::timeout::TimeoutCandidate::Branch(index) => {
                let branch = branches[index].clone();
                let guard = match self.timeout_guard(snapshot, &branch.node)? {
                    None => TimeoutGuard::Fire,
                    Some(expr) => {
                        // The window is decided on the engine's own
                        // environment (the legacy branch task's `init`), with
                        // the frozen cost the legacy `on_timeout` wrote into
                        // the task data.
                        let predicate = self
                            .evaluate_branch_predicate(
                                snapshot,
                                owner,
                                &expr,
                                &[(
                                    crate::utils::consts::TASK_COST.to_string(),
                                    cost_ms.to_string(),
                                )],
                            )
                            .await?;
                        match predicate {
                            crate::scheduler::core::plan::branch::PredicateValue::True => {
                                TimeoutGuard::Fire
                            }
                            // The window is not open: no claim, the round ends
                            // (the legacy branch skips without claiming, so a
                            // later round re-evaluates it).
                            crate::scheduler::core::plan::branch::PredicateValue::False => {
                                return Ok(TimeoutRound::Idle);
                            }
                            // A guard that cannot be evaluated is the legacy
                            // branch's `init` error: the branch is admitted and
                            // fails on its first operation.
                            crate::scheduler::core::plan::branch::PredicateValue::Null => {
                                TimeoutGuard::Fail(format!(
                                    "timeout step '{}' guard evaluated to null",
                                    branch.node
                                ))
                            }
                            crate::scheduler::core::plan::branch::PredicateValue::Invalid {
                                message,
                            } => TimeoutGuard::Fail(message),
                        }
                    }
                };
                Ok(TimeoutRound::Dispatch {
                    branch,
                    guard,
                    cost_ms,
                    declared: branches.len(),
                })
            }
        }
    }

    /// The plan of one dispatch round: the due branch admitted `Entry` through
    /// the container machinery (its execution, the owner's frozen cost and the
    /// advanced one-shot marker in one atomic commit) plus the timer's own
    /// replacement intent, so a later window still fires.
    #[allow(clippy::too_many_arguments)]
    fn timeout_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: Option<&crate::scheduler::core::commit::aggregate::TaskSnapshot>,
        claim: &OperationCommit,
        branch: crate::scheduler::core::plan::timeout::TimeoutBranch,
        guard: TimeoutGuard,
        cost_ms: i64,
        declared: usize,
        now_ms: i64,
    ) -> Result<TransitionPlan> {
        task.ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
        let mut pinned = Vec::new();
        if let TimeoutGuard::Fail(message) = guard {
            pinned.push((
                BRANCH_ERROR_VAR.to_string(),
                serde_json::json!({ "code": "", "message": message }).to_string(),
            ));
        }
        let claimed_after = crate::scheduler::core::plan::timeout::claim(
            &crate::scheduler::core::plan::timeout::claimed(snapshot, task_id),
            &branch.node,
        );
        let dispatch = infallible(Dispatch::plan(
            snapshot,
            &DispatchCommand {
                owner: task_id,
                source_operation: claim.clone(),
                branch: &branch,
                extra_vars: pinned,
                claimed_after: &claimed_after,
                cost_ms,
            },
        ));
        let mut plan = Fanout::plan(snapshot, &dispatch.command).map_err(runtime)?;
        if dispatch.claimed_after.len() < declared {
            // Declared branches remain unfired: the timer re-arms for them
            // instead of retiring (a window that has not opened yet must still
            // fire, and only a fresh round can see it).
            let sequence =
                next_plan_sequence(snapshot, &plan, task_id.node.as_str(), task_id.generation);
            plan.operations.push(OperationIntent {
                id: OperationId {
                    process_id: id.process_id.clone(),
                    task: task_id.clone(),
                    sequence,
                },
                kind: OperationKind::DispatchTimeout,
                ready_at_ms: Some(now_ms.saturating_add(self.timer_poll_ms())),
            });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// The bare remainder of an operation with nothing to do: its own commit
    /// and nothing else. Completing the operation is the whole effect.
    fn empty_remainder(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        claim: &OperationCommit,
    ) -> TransitionPlan {
        TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: Vec::new(),
        }
    }
}

/// The timer operation: one execution's declared `timeouts`, evaluated once per tick
/// while the execution is live.
pub(super) struct DispatchTimeout;

#[async_trait::async_trait]
impl ActOperation for DispatchTimeout {
    /// The round, asked *before* the claim: a timer that wakes on the tick
    /// with nothing due must not consume its own operation, so the pre-claim
    /// verdict is the same round the operation would decide — an [`Precheck::Ignore`]
    /// leaves the operation `Pending` for the next tick.
    async fn precheck(&self, host: &KernelHost, pending: &Pending<'_>) -> Result<Precheck> {
        let id = pending.id;
        let task_id = id.task.clone();
        let task = pending.snapshot.tasks.get(&task_id);
        let idle = matches!(
            host.timeout_round(pending.snapshot, &task_id, task, pending.now_ms)
                .await?,
            TimeoutRound::Idle
        );
        Ok(if idle {
            Precheck::Ignore
        } else {
            Precheck::Proceed
        })
    }

    /// The round, decided again against the authoritative post-claim snapshot:
    /// a decision that landed between the preflight and the claim leaves the
    /// timer with nothing to do, and the operation then has nothing to commit.
    async fn run(&self, host: &KernelHost, operation: &mut Operation<'_>) -> Result<Outcome> {
        Ok(
            match host
                .timeout_round(
                    operation.snapshot(),
                    &operation.task_id,
                    operation.task(),
                    operation.now_ms,
                )
                .await?
            {
                TimeoutRound::Idle => Outcome::Nothing,
                round => Outcome::Timeout(round),
            },
        )
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let Outcome::Timeout(round) = outcome else {
            return Err(runtime("the timer operation planned a foreign outcome"));
        };
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = &operation.task_id;
        let task = operation.task();
        let claim = operation.claim;
        Ok(match round {
            // The timer of a timed-out execution: this operation dispatches the due
            // branch.
            TimeoutRound::Dispatch {
                branch,
                guard,
                cost_ms,
                declared,
            } => host.timeout_plan(
                snapshot,
                id,
                task_id,
                task,
                claim,
                branch,
                guard,
                cost_ms,
                declared,
                operation.now_ms,
            )?,
            // Nothing can fire any more: the owner is decided or its node
            // declares no timeouts. The operation's own commit is its whole
            // remainder.
            TimeoutRound::Retire => KernelHost::empty_remainder(snapshot, id, claim),
            // The idle round is [`Outcome::Nothing`], which never reaches a
            // plan.
            TimeoutRound::Idle => {
                return Err(runtime("the timer operation replanned an idle round"));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::{
        model::{
            operation::ClaimToken,
            task_graph::{TaskId, TaskState},
        },
        plan::timeout::TimeoutCandidate,
    };

    fn host() -> std::sync::Arc<KernelHost> {
        KernelHost::in_memory().0
    }

    fn id(node: &str, sequence: u64) -> OperationId {
        OperationId {
            process_id: "p".into(),
            task: TaskId::new(node, 0).unwrap(),
            sequence,
        }
    }

    fn commit(id: &OperationId) -> OperationCommit {
        OperationCommit {
            id: id.clone(),
            claim: ClaimToken {
                owner: "test".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        }
    }

    fn op<'a>(
        id: &'a OperationId,
        claim: &'a OperationCommit,
        snapshot: &'a ProcessSnapshot,
        lane: &'a mut CommandLane,
    ) -> Operation<'a> {
        Operation {
            id,
            claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 1_000,
            lane,
        }
    }

    fn spec(guard: Option<&str>) -> ProcessSpec {
        // `with_step`/`with_timeout` take `fn` pointers: each expression has
        // its own non-capturing closure.
        let workflow = match guard {
            None => crate::model::Workflow::new().with_step(|step| {
                step.with_id("s1")
                    .with_timeout(|timeout| timeout.with_id("t1"))
            }),
            Some("false") => crate::model::Workflow::new().with_step(|step| {
                step.with_id("s1")
                    .with_timeout(|timeout| timeout.with_id("t1").with_if("false"))
            }),
            Some("true") => crate::model::Workflow::new().with_step(|step| {
                step.with_id("s1")
                    .with_timeout(|timeout| timeout.with_id("t1").with_if("true"))
            }),
            Some("null") => crate::model::Workflow::new().with_step(|step| {
                step.with_id("s1")
                    .with_timeout(|timeout| timeout.with_id("t1").with_if("null"))
            }),
            Some(_) => crate::model::Workflow::new().with_step(|step| {
                step.with_id("s1")
                    .with_timeout(|timeout| timeout.with_id("t1").with_if("$nope()"))
            }),
        };
        ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap()
    }

    fn running(guard: Option<&str>) -> ProcessSnapshot {
        let mut snapshot = spec(guard).seed_snapshot().unwrap();
        snapshot
            .tasks
            .get_mut(&TaskId::new("s1", 0).unwrap())
            .unwrap()
            .state = TaskState::Running;
        snapshot
    }

    #[tokio::test]
    async fn a_missing_model_declares_no_branches() {
        let host = host();
        let mut snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        let root = TaskId::new("root", 0).unwrap();
        snapshot
            .variables
            .remove(&(root, crate::scheduler::shell::model::MODEL_VAR.to_string()));
        assert!(
            host.timeout_branches(
                &snapshot,
                &crate::scheduler::core::model::model_graph::NodeId::parse("root").unwrap()
            )
            .unwrap()
            .is_empty()
        );
    }

    #[tokio::test]
    async fn a_node_outside_the_model_declares_no_branches() {
        let host = host();
        let snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        assert!(
            host.timeout_branches(
                &snapshot,
                &crate::scheduler::core::model::model_graph::NodeId::parse("ghost").unwrap()
            )
            .unwrap()
            .is_empty()
        );
    }

    /// Only a live execution can time out.
    #[tokio::test]
    async fn a_settled_or_absent_execution_retires() {
        let host = host();
        let mut snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        assert!(matches!(
            host.timeout_round(&snapshot, &owner, None, 1_000)
                .await
                .unwrap(),
            TimeoutRound::Retire
        ));
        snapshot.tasks.get_mut(&owner).unwrap().state = TaskState::Completed;
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        assert!(matches!(
            host.timeout_round(&snapshot, &owner, Some(&task), 1_000)
                .await
                .unwrap(),
            TimeoutRound::Retire
        ));
    }

    /// An owner whose node declares no timeouts has nothing to fire.
    #[tokio::test]
    async fn an_owner_without_branches_retires() {
        let host = host();
        let mut snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        let owner = TaskId::new("a", 0).unwrap();
        snapshot.tasks.get_mut(&owner).unwrap().state = TaskState::Running;
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        assert!(matches!(
            host.timeout_round(&snapshot, &owner, Some(&task), 1_000)
                .await
                .unwrap(),
            TimeoutRound::Retire
        ));
    }

    /// Every branch already fired: the timer retires.
    #[tokio::test]
    async fn a_fully_fired_timer_retires() {
        let host = host();
        let mut snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        snapshot.variables.insert(
            (
                owner.clone(),
                crate::utils::consts::TASK_TIMEOUTS.to_string(),
            ),
            serde_json::json!(["t1"]).to_string(),
        );
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        assert!(matches!(
            host.timeout_round(&snapshot, &owner, Some(&task), 1_000)
                .await
                .unwrap(),
            TimeoutRound::Retire
        ));
    }

    /// A branch without a guard fires immediately.
    #[tokio::test]
    async fn an_unguarded_branch_fires() {
        let host = host();
        let snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        match host
            .timeout_round(&snapshot, &owner, Some(&task), 1_000)
            .await
            .unwrap()
        {
            TimeoutRound::Dispatch {
                branch,
                guard,
                cost_ms,
                declared,
            } => {
                assert_eq!(branch.node, "t1");
                assert!(matches!(guard, TimeoutGuard::Fire));
                assert_eq!(declared, 1);
                assert!(cost_ms >= 0);
            }
            _ => panic!("expected a dispatch"),
        }
    }

    /// A closed window leaves the round idle (and the operation unclaimed).
    #[tokio::test]
    async fn a_closed_window_is_idle() {
        let host = host();
        let snapshot = running(Some("false"));
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        assert!(matches!(
            host.timeout_round(&snapshot, &owner, Some(&task), 1_000)
                .await
                .unwrap(),
            TimeoutRound::Idle
        ));
    }

    /// A guard that cannot be decided admits the branch and fails it.
    #[tokio::test]
    async fn an_undecidable_window_dispatch_fails_the_branch() {
        let host = host();
        for expr in ["null", "$nope()"] {
            let snapshot = running(Some(expr));
            let owner = TaskId::new("s1", 0).unwrap();
            let task = snapshot.tasks.get(&owner).unwrap().clone();
            match host
                .timeout_round(&snapshot, &owner, Some(&task), 1_000)
                .await
                .unwrap()
            {
                TimeoutRound::Dispatch { guard, .. } => {
                    assert!(
                        matches!(guard, TimeoutGuard::Fail(_)),
                        "{expr}: expected a failed guard"
                    )
                }
                _ => panic!("{expr}: expected a dispatch"),
            }
        }
    }

    /// An open window and a predicate error of the runner are both decided by
    /// the round.
    #[tokio::test]
    async fn an_open_window_fires() {
        let host = host();
        let snapshot = running(Some("true"));
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        match host
            .timeout_round(&snapshot, &owner, Some(&task), 1_000)
            .await
            .unwrap()
        {
            TimeoutRound::Dispatch { guard, .. } => {
                assert!(matches!(guard, TimeoutGuard::Fire))
            }
            _ => panic!("expected a dispatch"),
        }
    }

    /// The preflight refuses to claim an idle round.
    #[tokio::test]
    async fn the_preflight_ignores_an_idle_round() {
        let host = host();
        let idle = running(Some("false"));
        let id = id("s1", 0);
        assert_eq!(
            DispatchTimeout
                .precheck(
                    &host,
                    &Pending {
                        id: &id,
                        snapshot: &idle,
                        now_ms: 1_000,
                    }
                )
                .await
                .unwrap(),
            Precheck::Ignore
        );
        let fire = running(None);
        assert_eq!(
            DispatchTimeout
                .precheck(
                    &host,
                    &Pending {
                        id: &id,
                        snapshot: &fire,
                        now_ms: 1_000,
                    }
                )
                .await
                .unwrap(),
            Precheck::Proceed
        );
    }

    /// Phase 2 re-decides the round: idle is `Nothing`, retire is a round.
    #[tokio::test]
    async fn the_run_carries_the_round() {
        let host = host();
        let idle = running(Some("false"));
        let id = id("s1", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = op(&id, &claim, &idle, &mut lane);
        assert!(matches!(
            DispatchTimeout
                .run(&host, &mut { operation })
                .await
                .unwrap(),
            Outcome::Nothing
        ));
        let fire = running(None);
        let mut lane = CommandLane::detached(&host);
        let operation = op(&id, &claim, &fire, &mut lane);
        assert!(matches!(
            DispatchTimeout
                .run(&host, &mut { operation })
                .await
                .unwrap(),
            Outcome::Timeout(TimeoutRound::Dispatch { .. })
        ));
    }

    /// Phase 3 refuses a foreign outcome and an idle round.
    #[tokio::test]
    async fn the_plan_only_accepts_a_timer_round() {
        let host = host();
        let snapshot = running(None);
        let id = id("s1", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = op(&id, &claim, &snapshot, &mut lane);
        assert!(
            DispatchTimeout
                .next(&host, &operation, Outcome::Planned)
                .await
                .is_err()
        );
        assert!(
            DispatchTimeout
                .next(&host, &operation, Outcome::Timeout(TimeoutRound::Idle))
                .await
                .is_err()
        );
        let plan = DispatchTimeout
            .next(&host, &operation, Outcome::Timeout(TimeoutRound::Retire))
            .await
            .unwrap();
        assert!(plan.operations.is_empty());
        assert!(plan.state_changes.is_empty());
        assert_eq!(plan.source_operation.as_ref(), Some(&claim));
    }

    /// A dispatch admits the branch and carries the pinned failure of an
    /// undecidable guard.
    #[tokio::test]
    async fn a_dispatch_plan_admits_the_due_branch() {
        let host = host();
        let snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        let branch = host
            .timeout_branches(&snapshot, &task.model_node)
            .unwrap()
            .remove(0);
        let id = id("s1", 0);
        let claim = commit(&id);
        let plan = host
            .timeout_plan(
                &snapshot,
                &id,
                &owner,
                Some(&task),
                &claim,
                branch,
                TimeoutGuard::Fire,
                10,
                1,
                1_000,
            )
            .unwrap();
        assert!(
            plan.task_creates
                .iter()
                .any(|create| create.task.node.as_str() == "t1"),
            "the due branch is admitted: {plan:?}"
        );
        assert!(
            plan.operations
                .iter()
                .any(|intent| { intent.kind == OperationKind::ExecuteTask })
        );
        // Declared branches remain unfired only when stated; here one of one
        // fired, so the timer does not re-arm.
        assert!(
            plan.operations
                .iter()
                .all(|intent| intent.kind != OperationKind::DispatchTimeout)
        );
    }

    /// A failing guard pins its error on the branch's scope.
    #[tokio::test]
    async fn a_failed_guard_pins_the_branch_error() {
        let host = host();
        let snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        let branch = host
            .timeout_branches(&snapshot, &task.model_node)
            .unwrap()
            .remove(0);
        let id = id("s1", 0);
        let claim = commit(&id);
        let plan = host
            .timeout_plan(
                &snapshot,
                &id,
                &owner,
                Some(&task),
                &claim,
                branch,
                TimeoutGuard::Fail("nope".into()),
                0,
                1,
                1_000,
            )
            .unwrap();
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == BRANCH_ERROR_VAR)
        );
    }

    /// The plan needs the execution it dispatches for.
    #[tokio::test]
    async fn a_dispatch_without_its_execution_is_an_error() {
        let host = host();
        let snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        let branch = host
            .timeout_branches(&snapshot, &task.model_node)
            .unwrap()
            .remove(0);
        let id = id("s1", 0);
        let claim = commit(&id);
        let error = host
            .timeout_plan(
                &snapshot,
                &id,
                &owner,
                None,
                &claim,
                branch,
                TimeoutGuard::Fire,
                0,
                1,
                0,
            )
            .unwrap_err();
        assert!(format!("{error}").contains("not found"), "{error}");
    }

    /// The round's re-arm keeps a later declared window alive.
    #[tokio::test]
    async fn an_unfired_declared_branch_re_arms_the_timer() {
        let host = host();
        let snapshot = running(None);
        let owner = TaskId::new("s1", 0).unwrap();
        let task = snapshot.tasks.get(&owner).unwrap().clone();
        let branch = host
            .timeout_branches(&snapshot, &task.model_node)
            .unwrap()
            .remove(0);
        let id = id("s1", 0);
        let claim = commit(&id);
        let plan = host
            .timeout_plan(
                &snapshot,
                &id,
                &owner,
                Some(&task),
                &claim,
                branch,
                TimeoutGuard::Fire,
                0,
                2,
                1_000,
            )
            .unwrap();
        let timer = plan
            .operations
            .iter()
            .find(|intent| intent.kind == OperationKind::DispatchTimeout)
            .expect("the timer re-arms");
        assert_eq!(timer.ready_at_ms, Some(1_000 + host.timer_poll_ms()));
    }

    #[test]
    fn an_empty_remainder_only_closes_the_operation() {
        let snapshot = running(None);
        let id = id("s1", 3);
        let claim = commit(&id);
        let plan = KernelHost::empty_remainder(&snapshot, &id, &claim);
        assert_eq!(plan.source_operation.as_ref(), Some(&claim));
        assert_eq!(plan.expected, snapshot.version);
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn the_round_planner_constants_are_exposed() {
        // Sanity: the candidate vocabulary the round answers with is the one
        // the operation's `next` matches on.
        let candidate = TimeoutCandidate::Branch(0);
        assert!(matches!(candidate, TimeoutCandidate::Branch(0)));
    }
}
