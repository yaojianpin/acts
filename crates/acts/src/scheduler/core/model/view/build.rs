use crate::{
    Branch, Step, Workflow,
    utils::{longid, shortid},
};

/// Assign the ids the model does not declare, before the model is shared as an
/// immutable `Arc` or carried into the aggregate:
///
/// - an unnamed workflow gets a fresh long id — the `mid` every message of the
///   run carries;
/// - an unnamed step, branch, catch handler or timeout branch gets a fresh
///   short id.
///
/// The model validators (`Workflow::valid`/`tree_output`) and the kernel import
/// (`shell::model::ProcessSpec::from_workflow`) share this one pass: all need it
/// before anything derives node ids from the model. They run it on a copy, so the
/// caller's model stays untouched and the assigned ids are what land in the
/// aggregate's durable model.
pub(crate) fn assign_ids(workflow: &mut Workflow) {
    if workflow.id.is_empty() {
        workflow.id = longid();
    }
    for step in workflow.steps.iter_mut() {
        assign_step_ids(step);
    }
}

fn assign_step_ids(step: &mut Step) {
    if step.id.is_empty() {
        step.id = shortid();
    }

    for branch in step.branches.iter_mut() {
        assign_branch_ids(branch);
    }
    for catch in step.catches.iter_mut() {
        assign_step_ids(catch);
    }
    for timeout in step.timeouts.iter_mut() {
        assign_step_ids(timeout);
    }
}

fn assign_branch_ids(branch: &mut Branch) {
    if branch.id.is_empty() {
        branch.id = shortid();
    }

    for step in branch.steps.iter_mut() {
        assign_step_ids(step);
    }
}
