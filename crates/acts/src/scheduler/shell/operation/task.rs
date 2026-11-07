//! The `ExecuteTask` operation: starting one execution's act.
//!
//! Phase 2 is the step's entry check — the branch predicate pinned at
//! admission, the step's own `if`/`while` guard and its run bound, then the
//! act's start-time package resolution (the legacy act's `run()`, which
//! validates params before it either waits or executes) — and phase 3 plans
//! what the verdict means: the started execution, the act's `Created` message
//! (a `Msg` act skips it — its single delivery is the completion record), the
//! `ApplyAction` operation its effect will run, and the timer a node
//! declaring `timeouts` arms. A step `rule` whose condition holds before the
//! act starts contributes either a failure/skip plan or a durable decision
//! intent the started act applies as its first action (the act announces no
//! `Created` request for it).

use super::*;
use crate::scheduler::shell::*;

/// Phase 2's entry-check verdict: whether the step runs at all.
enum Guard {
    /// The step runs: nothing its own `if`/`while` or run bound said otherwise.
    Run,
    /// The step's own guard is false: the flow falls through to the next
    /// declared step.
    Skip,
    /// The step fails before it starts — a guard the engine cannot decide, or
    /// a node that already ran `max_node_run_times` times.
    Fail(Failure),
}

/// Phase 2's product for this kind: what the act's start was decided to be.
///
/// The two package facts ride the outcome because phase 2 read them from the
/// snapshot already — whether the acting node carries a `uses` package at all,
/// and whether the act's `Created` record is durable — and phase 3 would
/// otherwise re-derive both (a model decode, for the first).
pub(in crate::scheduler::shell) enum Start {
    /// The step's own guard skipped it: phase 3 records the fall-through.
    Skipped,
    /// The step failed before it started: phase 3 walks the error up (the
    /// legacy `emit_error` path).
    Failed(Failure),
    /// The act may start.
    Started {
        /// The act's facts: this operation's own resolution, or the ones a previous
        /// attempt pinned (a durable `Created` record is what tells them
        /// apart).
        facts: Option<ActFacts>,
        /// Whether *this* operation resolved them.
        fresh: bool,
        /// Whether the acting node carries a `uses` package at all.
        package: bool,
        /// Whether the act's `Created` record already exists.
        announced: bool,
        /// The client action a violated rule decided (`next`/`submit`/
        /// `abort`/`remove`). The started act executes it as its first
        /// decision instead of waiting for a client, in the same durable
        /// intent shape a client command uses.
        rule_action: Option<crate::event::EventAction>,
        /// The owner a declared `assign.owner` resolved to. `Some` only for a
        /// fresh start of an assigned act; its `__assignment` write rides
        /// this plan.
        assignment: Option<String>,
    },
    /// The act's params did not resolve: the execution is marked started and
    /// the decision is handed to its effect operation, which resolves again and
    /// fails it — the legacy act's `run()` gate, one operation later.
    Unresolved,
    /// The act declared an assign pool whose expression names business data:
    /// one IRQ round trip away, and the start waits for it. The plan pushes
    /// one candidates request under the act (`pool` names the declaration
    /// key the request answers); the settlement folds the answer into the
    /// act's variables under that key and re-enters this start (`first` =
    /// this is the first entry, which writes the wait marker; a re-entry
    /// after a skipped request asks again).
    Waiting {
        pool: String,
        params: serde_json::Value,
        uses: String,
        first: bool,
    },
    /// One of the act's assign pools (`pool`) resolved in the kernel — every
    /// atom was local — so that pool needs no round trip: the plan folds the
    /// computed pool into the act's variables under its own key (the same
    /// shape a settled answer lands in) and re-enters this start, which
    /// computes the next uncomputed pool or proceeds.
    Pool {
        pool: String,
        participants: Vec<crate::model::Participant>,
    },
}

/// The start operation: `Ready → Running` (auto) or `Ready → Waiting` (a waiting
/// package), plus the act's `Created` message record and, for an auto step,
/// the `ApplyAction` operation that performs its work.
pub(super) struct ExecuteTask;

#[async_trait::async_trait]
impl ActOperation for ExecuteTask {
    /// Nothing to check before the claim: a startable execution always starts.
    async fn precheck(&self, _host: &Workers, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    async fn run(&self, host: &Workers, operation: &mut Operation<'_>) -> Result<Outcome> {
        let snapshot = operation.snapshot();
        let task_id = &operation.task_id;
        let task = operation.task();
        // A branch whose predicate could not be evaluated fails on its first
        // operation: the fan-out admitted it with the failure pinned (see
        // `fanout_command`), which is the legacy branch task erroring in its
        // `init` — the container's error walk takes over from here.
        if task.is_some()
            && let Some(failure) = branch_error(snapshot, task_id)
        {
            return Ok(Outcome::Start(Start::Failed(failure)));
        }
        // The step's own guard and its run bound, decided before anything
        // resolves or runs (the legacy `Step::init`).
        let guard = if task.is_some() {
            Self::guard_verdict(host, snapshot, task_id).await?
        } else {
            Guard::Run
        };
        match guard {
            Guard::Skip => return Ok(Outcome::Start(Start::Skipped)),
            Guard::Fail(failure) => return Ok(Outcome::Start(Start::Failed(failure))),
            Guard::Run => {}
        }
        // The act's assign pools: local atoms compute in the kernel, one
        // evaluation ahead of anything else; business atoms need the spec's
        // `uses` package (one IRQ round trip) and the start waits for it.
        // Either way the fan-out and any owner expression that reads a pool
        // resolve on the re-entered start, with the pool in the act's scope.
        // Several declared pools settle one re-entry at a time: this pass
        // takes the first one not yet computed.
        let pools = match task {
            Some(task) => match Assign::declared_pools(snapshot, task_id, &task.model_node) {
                Ok(pools) => pools,
                Err(error) => {
                    return Ok(Outcome::Start(Start::Failed(Failure::new(
                        "invalid_assignment",
                        error.to_string(),
                    ))));
                }
            },
            None => Vec::new(),
        };
        if let Some(spec) = pools
            .into_iter()
            .find(|spec| !Assign::pool_computed(snapshot, task_id, &spec.pool))
        {
            // The spec's expression decides the path: naming business data
            // defers to the `uses` package (one IRQ round trip); an all-local
            // expression computes here, and a local one the scope cannot
            // answer fails the act instead of inventing an empty pool.
            if !names_business_data(&spec.params) {
                return Ok(
                    match host
                        .compute_candidates_pool(
                            snapshot,
                            Some(&operation.snapshot),
                            task_id,
                            &spec.params,
                        )
                        .await
                    {
                        Ok(participants) => Outcome::Start(Start::Pool {
                            pool: spec.pool,
                            participants,
                        }),
                        Err(error) => Outcome::Start(Start::Failed(Failure::new(
                            "invalid_assignment",
                            format!(
                                "act '{}': assign pool '{}' could not be computed: {error}",
                                task_id.node.as_str(),
                                spec.pool
                            ),
                        ))),
                    },
                );
            }
            let first = !Assign::pool_pending(snapshot, task_id, &spec.pool);
            return Ok(Outcome::Start(Start::Waiting {
                pool: spec.pool,
                params: serde_json::to_value(&spec.params).unwrap_or(serde_json::Value::Null),
                uses: spec.uses,
                first,
            }));
        }
        // A declared `assign.owner` is a human decision act's owner. The
        // declaration is refused on anything else before the act can wait,
        // and its string must resolve to a non-empty owner: an unowned
        // approval is never produced.
        let assignment_owner = match task {
            Some(task) => match Assign::declared_owner(snapshot, task_id, &task.model_node) {
                Ok(assignment) => assignment,
                Err(error) => {
                    return Ok(Outcome::Start(Start::Failed(Failure::new(
                        "invalid_assignment",
                        error.to_string(),
                    ))));
                }
            },
            None => None,
        };
        let assignment = match assignment_owner {
            Some(owner) => {
                match host
                    .resolve_assignment_owner(snapshot, Some(&operation.snapshot), task_id, &owner)
                    .await
                {
                    Ok(Some(owner)) => Some(owner),
                    Ok(None) => {
                        return Ok(Outcome::Start(Start::Failed(Failure::new(
                            "assignment_failed",
                            format!(
                                "act '{}': assign.owner resolved to an empty value; refusing an unowned decision",
                                task_id.node.as_str()
                            ),
                        ))));
                    }
                    Err(error) => {
                        return Ok(Outcome::Start(Start::Failed(Failure::new(
                            "assignment_failed",
                            format!(
                                "act '{}': assign.owner could not be resolved: {error}",
                                task_id.node.as_str()
                            ),
                        ))));
                    }
                }
            }
            None => None,
        };
        // The act's package facts and its own gate: every `uses` step resolves
        // its package at start, and the pinned facts ride every later message
        // record — so no later operation re-runs the headless scope, whose snapshot
        // seal must fail the act at start, never a completion.
        let created_key = format!(
            "message.created:{}:{}:{}",
            operation.id.process_id,
            operation.id.task.node.as_str(),
            operation.id.task.generation
        );
        let announced = snapshot
            .outbox
            .keys()
            .any(|outbox_id| outbox_id.key == created_key);
        // Whether the acting node is a `uses` step at all: the message record
        // exists for every package step, with or without a runner attached
        // (the payload then carries the raw declared params).
        let package = match task {
            Some(task) => step_uses(snapshot, task_id, &task.model_node)?.is_some(),
            None => false,
        };
        let start = if !package || announced {
            Start::Started {
                facts: act_facts(snapshot, task_id),
                fresh: false,
                package,
                announced,
                rule_action: None,
                assignment: assignment.clone(),
            }
        } else if let Some(task) = task {
            match host
                .resolve_step(
                    snapshot,
                    Some(&operation.snapshot),
                    task_id,
                    &task.model_node,
                )
                .await
            {
                Ok(facts) => {
                    // The step's input rules: the first `rules:` condition
                    // that holds against the scope and the resolved params
                    // runs its action before the act announces itself.
                    let params = facts
                        .as_ref()
                        .map(|facts| &facts.params)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    if let Some(verdict) =
                        input_rule_verdict(host, snapshot, task_id, &task.model_node, &params)
                            .await?
                    {
                        return Ok(Outcome::Start(match verdict {
                            RuleVerdict::Failure(failure) => Start::Failed(failure),
                            RuleVerdict::Skip => Start::Skipped,
                            RuleVerdict::Action(action) => Start::Started {
                                facts,
                                fresh: true,
                                package,
                                announced,
                                rule_action: Some(action),
                                assignment: assignment.clone(),
                            },
                        }));
                    }
                    Start::Started {
                        facts,
                        fresh: true,
                        package,
                        announced,
                        rule_action: None,
                        assignment: assignment.clone(),
                    }
                }
                Err(error) => {
                    // The act cannot start with these params (the legacy act's
                    // `run()` fails it before it waits or executes). The state
                    // table, like the legacy sequence, only decides a failure
                    // from a started act: this operation marks it started and hands
                    // the decision to the effect operation, which resolves again and
                    // fails it — the same error walk, one operation later.
                    tracing::debug!(
                        pid = %snapshot.process_id,
                        node = task_id.node.as_str(),
                        error = %error,
                        "act resolution failed at start; failing it on its effect operation"
                    );
                    Start::Unresolved
                }
            }
        } else {
            Start::Started {
                facts: None,
                fresh: false,
                package,
                announced,
                rule_action: None,
                assignment,
            }
        };
        Ok(Outcome::Start(start))
    }

    async fn next(
        &self,
        host: &Workers,
        operation: &Operation<'_>,
        outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let Outcome::Start(start) = outcome else {
            return Err(runtime("the start operation planned a foreign outcome"));
        };
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation.task();
        let now_ms = operation.now_ms;
        // The action this start seeds (an auto task's own completion) and
        // whether it is already durable.
        let action_id = OperationId {
            process_id: id.process_id.clone(),
            task: id.task.clone(),
            sequence: id.sequence + 1,
        };
        let action_exists = snapshot.operations.contains_key(&action_id);
        // Auto tasks seed their own completion action; waiting tasks stay
        // Running until a client action arrives.
        let auto = snapshot
            .variables
            .get(&(task_id.clone(), "auto".into()))
            .map(String::as_str)
            != Some("0");
        let mut plan = host.task_operation_plan(
            snapshot,
            id,
            &task_id,
            task,
            claim,
            "started",
            "task.started",
        )?;
        // The start note is once per execution: a re-entered start (an act
        // that waited for its candidate pool) must not re-emit it — the
        // outbox key would already exist and refuse the whole commit.
        let started_key = format!(
            "started:{}:{}:{}",
            id.process_id,
            id.task.node.as_str(),
            id.task.generation
        );
        if snapshot
            .outbox
            .keys()
            .any(|outbox_id| outbox_id.key == started_key)
        {
            plan.outbox.clear();
        }
        match start {
            Start::Failed(failure) => {
                let task = task
                    .ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
                return ApplyAction::failure_plan(
                    host, snapshot, id, &task_id, task, claim, &failure,
                )
                .await;
            }
            Start::Skipped => {
                let task = task
                    .ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
                return Self::guard_skip_plan(host, snapshot, id, &task_id, task, claim);
            }
            Start::Pool { pool, participants } => {
                // The kernel computed the pool locally (every atom of the
                // expression was local): fold it into the act's variables
                // under the pool's own key — the same shape a settled answer
                // lands in — and re-enter the start, which finds the pool in
                // scope, computes the next uncomputed pool, or proceeds
                // without a round trip.
                Assign::settle_local_pool(snapshot, &mut plan, &task_id, &pool, &participants)?;
                return Ok(plan);
            }
            Start::Waiting {
                pool,
                params,
                uses,
                first,
            } => {
                // The act parks like a waiting package (it owes the pool
                // request, not a client): without the park the ready pass
                // would re-drive the start and mint requests forever. The
                // wait marker is first-entry only; a re-entry after a skipped
                // request just asks again.
                if first {
                    if let Some(task) = task
                        && task.state
                            != crate::scheduler::core::model::task_graph::TaskState::Waiting
                    {
                        plan.state_changes.push(StateChange {
                            task: task_id.clone(),
                            expected_revision: task.revision,
                            from: task.state,
                            to: crate::scheduler::core::model::task_graph::TaskState::Waiting,
                        });
                    }
                    plan.variable_writes.push(VariableWrite {
                        scope: task_id.clone(),
                        name: Assign::pool_wait_var(&pool),
                        value: params.to_string(),
                    });
                }
                Assign::ask(snapshot, &mut plan, &task_id, &pool, params, uses)?;
                return Ok(plan);
            }
            Start::Unresolved => {
                let task = task
                    .ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
                if startable(task.state) {
                    plan.state_changes.push(StateChange {
                        task: task_id.clone(),
                        expected_revision: task.revision,
                        from: task.state,
                        to: crate::scheduler::core::model::task_graph::TaskState::Running,
                    });
                    if !action_exists {
                        plan.operations.push(OperationIntent {
                            id: action_id,
                            kind: OperationKind::ApplyAction,
                            ready_at_ms: None,
                        });
                    }
                }
                return Ok(plan);
            }
            Start::Started {
                facts,
                fresh,
                package: has_package,
                announced: created_exists,
                rule_action,
                assignment,
            } => {
                // A rule that failed with a decision action starts the act
                // and pins that decision as its first durable intent; the
                // apply operation replays it exactly like a client command.
                let rule_action = match rule_action {
                    Some(action) => Some(rule_action_kind(&action).ok_or_else(|| {
                        ActError::Runtime(format!(
                            "validation rule action '{action}' cannot be applied when the act starts"
                        ))
                    })?),
                    None => None,
                };
                let auto = auto && rule_action.is_none();
                // The act message: every `uses` step announces itself to the
                // client when it starts (the legacy act's `Created` message —
                // Irq waits on it, Msg informs, Func pairs it with the later
                // completion). The durable record rides the outbox; the
                // attached transport turns it into the engine's message.
                if has_package
                    && let Some(facts) = &facts
                    && fresh
                {
                    plan.variable_writes.push(VariableWrite {
                        scope: task_id.clone(),
                        name: PARAMS_VAR.to_string(),
                        value: facts.params.to_string(),
                    });
                    plan.variable_writes.push(VariableWrite {
                        scope: task_id.clone(),
                        name: OPTIONS_VAR.to_string(),
                        value: facts.options.to_string(),
                    });
                }
                if fresh && let Some(owner) = &assignment {
                    plan.variable_writes.push(VariableWrite {
                        scope: task_id.clone(),
                        name: ASSIGNMENT_VAR.to_string(),
                        value: Assignment::new(owner).value(),
                    });
                }
                // The execution announces itself: the act's Created message
                // for a `uses` step, the task-level one (type = the node kind)
                // for a package-less execution — the legacy engine's `on_task`
                // fired for every task, not only the acts. A `Msg` act is the
                // exception: it waits for no client, so it is delivered once —
                // its completion record, which carries the same payload — and
                // no `Created` precedes it.
                // A rule-triggered decision is the act's first verdict: it
                // must not ask a client whose answer the action already
                // gives.
                let announces_created = rule_action.is_none()
                    && (!has_package || !msg_decision(snapshot, id.task.node.as_str())?);
                if !created_exists
                    && announces_created
                    && let Some(mut payload) = operation_message_payload(
                        snapshot,
                        &task_id,
                        id.task.node.as_str(),
                        facts.as_ref(),
                        None,
                        None,
                    )?
                {
                    // The payload was built before this plan writes
                    // `__assignment`; the start's resolved owner is merged in.
                    if let Some(owner) = &assignment {
                        Assign::inject_payload_assignment(&mut payload, &Assignment::new(owner));
                    }
                    // The message id is minted with the durable record, not
                    // at delivery time: a later assignment notification
                    // references the original act message by this id, and a
                    // restart must not have to invent one.
                    payload["msg_id"] = serde_json::Value::String(crate::utils::longid());
                    plan.outbox.push(
                        crate::scheduler::core::model::transition_plan::OutboxIntent {
                            key: format!(
                                "message.created:{}:{}:{}",
                                id.process_id,
                                id.task.node.as_str(),
                                id.task.generation
                            ),
                            topic: "message.created".into(),
                            payload: payload.to_string(),
                        },
                    );
                }
                // A waiting act normally runs no effect (an `Irq` waits for
                // its client), but a subflow's effect starts the child run
                // the act then waits for. Seed that operation here, in the
                // same commit that parked the act, so the child can never
                // finish before the parent is waiting.
                let runs_while_waiting = if auto || rule_action.is_some() {
                    false
                } else {
                    match task {
                        Some(task) => host.step_runs_while_waiting(
                            snapshot,
                            Some(&operation.snapshot),
                            &task_id,
                            &task.model_node,
                        )?,
                        None => false,
                    }
                };
                if let Some(kind) = &rule_action {
                    let sequence = action_id.sequence;
                    let name = action_fact_var(sequence);
                    if !snapshot
                        .variables
                        .contains_key(&(task_id.clone(), name.clone()))
                    {
                        plan.variable_writes.push(VariableWrite {
                            scope: task_id.clone(),
                            name,
                            value: action_fact(kind, &[], None, None),
                        });
                    }
                    if !action_exists {
                        plan.operations.push(OperationIntent {
                            id: action_id.clone(),
                            kind: OperationKind::ApplyAction,
                            ready_at_ms: None,
                        });
                    }
                } else if !action_exists && (auto || runs_while_waiting) {
                    plan.operations.push(OperationIntent {
                        id: action_id,
                        kind: OperationKind::ApplyAction,
                        ready_at_ms: None,
                    });
                }
                if let Some(starting) = task.filter(|starting| startable(starting.state)) {
                    // Waiting packages park in `Waiting` (the legacy
                    // `Interrupt`); everything else runs to completion.
                    let target = if auto {
                        crate::scheduler::core::model::task_graph::TaskState::Running
                    } else {
                        crate::scheduler::core::model::task_graph::TaskState::Waiting
                    };
                    plan.state_changes.push(StateChange {
                        task: task_id.clone(),
                        expected_revision: starting.revision,
                        from: starting.state,
                        to: target,
                    });
                    // A node that declares `timeouts` starts its cost clock
                    // here and arms the timer that fires them (see
                    // `crate::scheduler::core::plan::timeout` and the timeout
                    // operation): the timer is durable, so a tick after a crash still
                    // fires a branch whose window opened while the host was
                    // down. Only timeout owners carry the stamp — it is a
                    // kernel fact, never a variable a model reads.
                    if !DispatchTimeout::timeout_branches(snapshot, &starting.model_node)?
                        .is_empty()
                    {
                        plan.variable_writes.push(
                            crate::scheduler::core::plan::timeout::start_write(&task_id, now_ms),
                        );
                        let sequence = next_plan_sequence(
                            snapshot,
                            &plan,
                            task_id.node.as_str(),
                            task_id.generation,
                        );
                        plan.operations.push(OperationIntent {
                            id: OperationId {
                                process_id: id.process_id.clone(),
                                task: task_id.clone(),
                                sequence,
                            },
                            kind: OperationKind::DispatchTimeout,
                            ready_at_ms: Some(now_ms.saturating_add(host.timer_poll_ms())),
                        });
                    }
                }
            }
        }
        Ok(plan)
    }
}

impl ExecuteTask {
    /// The step's own guard and run bound, decided at the start operation — the
    /// legacy `Step::init`, which decides whether a step runs at all before any
    /// act exists.
    ///
    /// - `if`/`while` false: the step is *skipped* and the flow falls through
    ///   to the next declared step — never the step's own `next` (that is what
    ///   lets a guarded self/backward `next` loop terminate). The private
    ///   marker [`GUARD_SKIP_VAR`] is what tells the propagation and the join
    ///   that this skip is a fall-through and not the deadness cascade.
    /// - a guard the engine cannot decide (a null or invalid predicate): the
    ///   step fails, exactly like the branch predicate the engine cannot
    ///   decide.
    /// - a node that already ran `max_node_run_times` times: the flow loops
    ///   without a guard that closes, so the run fails here — the legacy
    ///   `Process::create_task` bound, on the kernel's own run count (a
    ///   generation per turn).
    async fn guard_verdict(
        host: &Workers,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<Guard> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(Guard::Run);
        };
        let Some(step) = find_step_in_model(&model, task_id.node.as_str()) else {
            return Ok(Guard::Run);
        };
        // A handler step (a catch or a timeout branch) is dispatched by the
        // failure machinery or the tick, and *its* `if` is the selection that
        // dispatched it — the flow never runs one, so its guard is not the
        // flow's guard (`is_handler_step`; the import already refuses a
        // handler `while`, which would loop it).
        if is_handler_step(&model, task_id.node.as_str()) {
            return Ok(Guard::Run);
        }
        // `while` is the loop's guard and the step's own entry check in one;
        // `if` is only the entry check. A step declaring a `while` never
        // declares a `next` (rejected at import), so only one of the two can
        // be the guard of a looping step.
        if let Some(expr) = step.r#while.as_ref().or(step.r#if.as_ref()) {
            match host
                .evaluate_branch_predicate(snapshot, task_id, expr, &[])
                .await?
            {
                PredicateValue::True => {}
                PredicateValue::False => return Ok(Guard::Skip),
                PredicateValue::Invalid { message } => {
                    return Ok(Guard::Fail(Failure::new(String::new(), message)));
                }
                PredicateValue::Null => {
                    return Ok(Guard::Fail(Failure::new(
                        String::new(),
                        format!("step '{}' guard '{expr}' evaluated to null", step.id),
                    )));
                }
            }
        }
        // The run bound: `generation` *is* the count of times this node ran in
        // this run, because every turn mints a fresh generation.
        if host.max_node_run_times > 0 && task_id.generation >= host.max_node_run_times {
            return Ok(Guard::Fail(Failure::new(
                ELOOP_ECODE,
                format!(
                    "node '{}' ({}) in process '{}' was executed more than {} times, \
                     a loop in the workflow is likely",
                    step.id, step.name, snapshot.process_id, host.max_node_run_times
                ),
            )));
        }
        Ok(Guard::Run)
    }

    /// The plan of a step its own `if`/`while` guard skipped: the legacy
    /// `Step::init` guard falling through. The execution is `Skipped` (the
    /// legacy task's own state, and the row/message a client sees), the
    /// private marker records that the flow goes on, the act's Skipped message
    /// is emitted, and the propagation carries the flow into the next declared
    /// step.
    fn guard_skip_plan(
        host: &Workers,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
    ) -> Result<TransitionPlan> {
        let propagate_id = OperationId {
            process_id: id.process_id.clone(),
            task: id.task.clone(),
            sequence: id.sequence + 1,
        };
        let mut plan = host.task_operation_plan(
            snapshot,
            id,
            task_id,
            Some(task),
            claim,
            "skipped",
            "task.skipped",
        )?;
        // The marker the propagation and the join read (a `__` fact: out of
        // the expression space and the read model).
        plan.variable_writes.push(VariableWrite {
            scope: task_id.clone(),
            name: crate::scheduler::core::plan::propagation::GUARD_SKIP_VAR.to_string(),
            value: serde_json::json!(true).to_string(),
        });
        // The act's own Skipped message, like the cascade's operation emits.
        if let Some(payload) = operation_message_payload(
            snapshot,
            task_id,
            id.task.node.as_str(),
            act_facts(snapshot, task_id).as_ref(),
            None,
            None,
        )? {
            plan.outbox[0].payload = payload.to_string();
        }
        plan.state_changes.push(StateChange {
            task: task_id.clone(),
            expected_revision: task.revision,
            from: task.state,
            to: crate::scheduler::core::model::task_graph::TaskState::Skipped,
        });
        if !snapshot.operations.contains_key(&propagate_id) {
            plan.operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
                ready_at_ms: None,
            });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }
}

/// The kernel action a rule's `action` names when it executes at act start.
/// `error` and `skip` are decided by the start itself (failure/skip plans);
/// `back`, `cancel` and `push` need a target/uses the rule does not carry and
/// are refused by the rule verdict before they reach here.
fn rule_action_kind(
    action: &crate::event::EventAction,
) -> Option<crate::scheduler::core::commit::command::ActionKind> {
    use crate::event::EventAction as E;
    use crate::scheduler::core::commit::command::ActionKind as A;
    match action {
        E::Next => Some(A::Complete),
        E::Submit => Some(A::Submit),
        E::Abort => Some(A::Abort),
        E::Remove => Some(A::Remove),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::{
        operation::ClaimToken,
        task_graph::{TaskId, TaskState},
    };

    fn host() -> std::sync::Arc<Workers> {
        Workers::in_memory().0
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

    fn snapshot() -> ProcessSnapshot {
        ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap()
    }

    /// Every terminal start verdict needs the execution it decides.
    #[tokio::test]
    async fn terminal_starts_need_their_execution() {
        let host = host();
        let snapshot = snapshot();
        let id = id("a", 0);
        let claim = commit(&id);
        for outcome in [
            Outcome::Start(Start::Failed(Failure::new("E", "boom"))),
            Outcome::Start(Start::Skipped),
            Outcome::Start(Start::Unresolved),
        ] {
            let mut lane = CommandLane::detached(&host);
            let operation = Operation {
                id: &id,
                claim: &claim,
                task_id: id.task.clone(),
                task: None,
                snapshot: std::sync::Arc::new(snapshot.clone()),
                now_ms: 0,
                lane: &mut lane,
            };
            let error = ExecuteTask
                .next(&host, &operation, outcome)
                .await
                .unwrap_err();
            assert!(format!("{error}").contains("not found"), "{error}");
        }
    }

    /// Without a model, a guard is not the flow's to decide: the step runs.
    #[tokio::test]
    async fn a_missing_model_runs_the_guard() {
        let host = host();
        let mut snapshot = snapshot();
        let root = TaskId::new("root", 0).unwrap();
        snapshot
            .variables
            .remove(&(root, crate::scheduler::shell::model::MODEL_VAR.to_string()));
        assert!(matches!(
            ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("a", 0).unwrap())
                .await
                .unwrap(),
            Guard::Run
        ));
    }

    /// A node the model does not declare is not guarded.
    #[tokio::test]
    async fn an_unknown_node_runs() {
        let host = host();
        let snapshot = snapshot();
        assert!(matches!(
            ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("ghost", 0).unwrap())
                .await
                .unwrap(),
            Guard::Run
        ));
    }

    /// The handler's `if` selected it at dispatch; the flow never re-runs it.
    #[tokio::test]
    async fn a_handler_step_skips_the_flow_guard() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_timeout(|timeout| timeout.with_id("t1").with_if("false"))
        });
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        assert!(matches!(
            ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("t1", 0).unwrap())
                .await
                .unwrap(),
            Guard::Run
        ));
    }

    /// A false guard falls through instead of running the step.
    #[tokio::test]
    async fn a_false_guard_skips_the_step() {
        let host = host();
        let workflow =
            crate::model::Workflow::new().with_step(|step| step.with_id("s1").with_if("false"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        assert!(matches!(
            ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("s1", 0).unwrap())
                .await
                .unwrap(),
            Guard::Skip
        ));
    }

    /// A guard that evaluates to null cannot decide the step: it fails.
    #[tokio::test]
    async fn a_null_guard_fails_the_step() {
        let host = host();
        let workflow =
            crate::model::Workflow::new().with_step(|step| step.with_id("s1").with_if("null"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        match ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("s1", 0).unwrap())
            .await
            .unwrap()
        {
            Guard::Fail(failure) => assert!(failure.message.contains("evaluated to null")),
            _ => panic!("a null guard must fail"),
        }
    }

    /// A non-boolean guard is an invalid predicate and fails the step.
    #[tokio::test]
    async fn an_invalid_guard_fails_the_step() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_if("$not_declared()"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        assert!(matches!(
            ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("s1", 0).unwrap())
                .await
                .unwrap(),
            Guard::Fail(_)
        ));
    }

    /// A predicate runner failure is an error of the operation, not a verdict.
    #[tokio::test]
    async fn a_predicate_error_fails_the_operation() {
        struct Broken;
        impl crate::scheduler::shell::caps::Capability for Broken {
            fn key(&self) -> &str {
                "test.predicate"
            }
        }
        #[async_trait::async_trait]
        impl crate::scheduler::shell::caps::predicate::PredicateRunner for Broken {
            async fn evaluate(
                &self,
                _request: crate::scheduler::shell::caps::predicate::PredicateRequest,
            ) -> Result<PredicateValue> {
                Err(ActError::Runtime("predicate exploded".into()))
            }
        }
        let host = host();
        host.attach_predicate_runner(std::sync::Arc::new(Broken));
        let workflow =
            crate::model::Workflow::new().with_step(|step| step.with_id("s1").with_if("true"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        match ExecuteTask::guard_verdict(&host, &snapshot, &TaskId::new("s1", 0).unwrap())
            .await
            .unwrap()
        {
            Guard::Fail(failure) => {
                assert!(
                    failure.message.contains("predicate exploded"),
                    "{}",
                    failure.message
                )
            }
            _ => panic!("a predicate error must fail the step"),
        }
    }

    /// The run bound stops a loop that never closed its guard.
    #[tokio::test]
    async fn the_run_bound_fails_the_step() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| step.with_id("s1"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        let limit = host.max_node_run_times;
        let task_id = TaskId::new("s1", limit).unwrap();
        match ExecuteTask::guard_verdict(&host, &snapshot, &task_id)
            .await
            .unwrap()
        {
            Guard::Fail(failure) => assert_eq!(failure.code, ELOOP_ECODE),
            _ => panic!("a generation past the bound must fail"),
        }
    }

    /// A guard skip carries the fall-through marker, its message and the
    /// propagation hand-off.
    #[tokio::test]
    async fn a_guard_skip_plans_the_fall_through() {
        let host = host();
        let mut snapshot = snapshot();
        let task_id = TaskId::new("a", 0).unwrap();
        snapshot.tasks.get_mut(&task_id).unwrap().state = TaskState::Ready;
        let id = id("a", 0);
        let claim = commit(&id);
        let task = snapshot.tasks.get(&task_id).unwrap().clone();
        let plan =
            ExecuteTask::guard_skip_plan(&host, &snapshot, &id, &task_id, &task, &claim).unwrap();
        assert!(plan.variable_writes.iter().any(|write| {
            write.name == crate::scheduler::core::plan::propagation::GUARD_SKIP_VAR
        }));
        assert_eq!(plan.state_changes[0].to, TaskState::Skipped);
        assert_eq!(plan.outbox[0].topic, "task.skipped");
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }

    /// A hand-off that is already durable is not created a second time.
    #[tokio::test]
    async fn a_guard_skip_does_not_duplicate_the_propagation() {
        let host = host();
        let mut snapshot = snapshot();
        let task_id = TaskId::new("a", 0).unwrap();
        snapshot.tasks.get_mut(&task_id).unwrap().state = TaskState::Ready;
        let id = id("a", 0);
        let propagate = OperationId {
            sequence: id.sequence + 1,
            ..id.clone()
        };
        snapshot.operations.insert(
            propagate.clone(),
            crate::scheduler::core::model::operation::OperationRecord::pending(
                propagate,
                OperationKind::PropagateTask,
            ),
        );
        let claim = commit(&id);
        let task = snapshot.tasks.get(&task_id).unwrap().clone();
        let plan =
            ExecuteTask::guard_skip_plan(&host, &snapshot, &id, &task_id, &task, &claim).unwrap();
        assert!(plan.operations.is_empty());
    }
}
