use super::{
    node::{Node, NodeData},
    node_tree::NodeTree,
};
use crate::{
    utils::{longid, shortid},
    Act, ActError, Branch, Job, Result, Step, Workflow,
};
use std::sync::Arc;

pub fn build_workflow(workflow: &mut Workflow, tree: &mut NodeTree) -> Result<()> {
    let level = 0;
    if workflow.id.is_empty() {
        workflow.id = longid();
    }

    let mut jobs = Vec::new();
    for job in workflow.jobs.iter_mut() {
        let node = build_job(job, tree, level + 1)?;
        jobs.push(node);
    }

    // in the end to set the job node parent
    // to make sure the workflow's jobs is the newest
    let data = NodeData::Workflow(workflow.clone());
    let root = tree.make(data, level)?;
    for job in jobs {
        job.set_parent(&root);
    }

    tree.model = Box::new(workflow.clone());
    tree.set_root(&root);

    Ok(())
}

pub fn build_job(job: &mut Job, tree: &mut NodeTree, level: usize) -> Result<Arc<Node>> {
    if job.id.is_empty() {
        job.id = shortid();
    }
    let data = NodeData::Job(job.clone());
    let node = tree.make(data, level)?;

    let mut prev = node.clone();
    for step in job.steps.iter_mut() {
        build_step(step, tree, &node, &mut prev, level + 1)?;
    }

    Ok(node.clone())
}

pub fn build_step(
    step: &mut Step,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) -> Result<()> {
    if step.id.is_empty() {
        step.id = shortid();
    }
    let data = NodeData::Step(step.clone());
    let node = tree.make(data, level)?;

    if node.level == prev.level {
        prev.set_next(&node, true);
    } else {
        node.set_parent(&parent);
    }

    match &step.next {
        Some(next) => match &tree.node(next) {
            Some(next) => {
                node.set_next(next, false);
            }
            None => tree.set_error(
                ActError::Runtime(format!("found next node error by '{}'", next)).into(),
            ),
        },
        None => {
            if step.branches.len() > 0 {
                let mut branch_prev = node.clone();
                for branch in step.branches.iter_mut() {
                    build_branch(branch, tree, &node, &mut branch_prev, level + 1)?;
                }
            }
        }
    }

    if step.acts.len() > 0 {
        let mut act_prev = node.clone();
        for act in step.acts.iter_mut() {
            build_act(act, tree, &node, &mut act_prev, level + 1)?;
        }
    }

    *prev = node.clone();

    Ok(())
}

pub fn build_branch(
    branch: &mut Branch,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) -> Result<()> {
    if branch.id.is_empty() {
        branch.id = shortid();
    }
    let data = NodeData::Branch(branch.clone());
    let node = tree.make(data, level)?;
    node.set_parent(&parent);

    let parent = node.clone();
    let mut step_prev = node.clone();
    for step in branch.steps.iter_mut() {
        build_step(step, tree, &parent, &mut step_prev, level + 1)?;
    }

    *prev = node.clone();

    Ok(())
}

pub fn build_act(
    act: &mut Act,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) -> Result<()> {
    if act.id.is_empty() {
        act.id = shortid();
    }

    let data = NodeData::Act(act.clone());
    let node = tree.make(data, level)?;
    node.set_parent(&parent);

    *prev = node.clone();

    Ok(())
}
