//! The timer of an execution whose node declares `timeouts` (the
//! `DispatchTimeout` operation).
//!
//! Phase 1's pre-claim policy for this kind is the *idle* round: evaluating
//! the branches before the claim keeps a timer with nothing due from consuming
//! its own operation (it stays `Pending`, so the recovery scan re-wakes it —
//! the kernel's tick cadence). Phase 2 decides the round again, against the
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
    /// The cost is what passed since the execution's own start hop (the legacy
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
                            // fails on its first hop.
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
            let sequence = next_plan_sequence(snapshot, &plan, &task_id.node, task_id.generation);
            plan.operations.push(OperationIntent {
                id: OperationId {
                    process_id: id.process_id.clone(),
                    node: task_id.node.clone(),
                    generation: task_id.generation,
                    sequence,
                },
                kind: OperationKind::DispatchTimeout,
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

/// The timer hop: one execution's declared `timeouts`, evaluated once per tick
/// while the execution is live.
pub(super) struct DispatchTimeout;

#[async_trait::async_trait]
impl ActOperation for DispatchTimeout {
    /// The round, asked *before* the claim: a timer that wakes on the tick
    /// with nothing due must not consume its own operation, so the pre-claim
    /// verdict is the same round the hop would decide — an [`Precheck::Ignore`]
    /// leaves the operation `Pending` for the next tick.
    async fn precheck(&self, host: &KernelHost, pending: &Pending<'_>) -> Result<Precheck> {
        let id = pending.id;
        let task_id = crate::scheduler::core::model::task_graph::TaskId {
            node: id.node.clone(),
            generation: id.generation,
        };
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
    /// timer with nothing to do, and the hop then has nothing to commit.
    async fn run(&self, host: &KernelHost, hop: &Hop<'_>) -> Result<Outcome> {
        Ok(
            match host
                .timeout_round(hop.snapshot, &hop.task_id, hop.task, hop.now_ms)
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
        hop: &Hop<'_>,
        outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let Outcome::Timeout(round) = outcome else {
            return Err(runtime("the timer hop planned a foreign outcome"));
        };
        let snapshot = hop.snapshot;
        let id = hop.id;
        let task_id = &hop.task_id;
        let task = hop.task;
        let claim = hop.claim;
        Ok(match round {
            // The timer of a timed-out execution: this hop dispatches the due
            // branch.
            TimeoutRound::Dispatch {
                branch,
                guard,
                cost_ms,
                declared,
            } => host.timeout_plan(
                snapshot, id, task_id, task, claim, branch, guard, cost_ms, declared,
            )?,
            // Nothing can fire any more: the owner is decided or its node
            // declares no timeouts. The operation's own commit is its whole
            // remainder.
            TimeoutRound::Retire => KernelHost::empty_remainder(snapshot, id, claim),
            // The idle round is [`Outcome::Nothing`], which never reaches a
            // plan.
            TimeoutRound::Idle => {
                return Err(runtime("the timer hop replanned an idle round"));
            }
        })
    }
}
