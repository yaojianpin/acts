use super::{
    node::{Node, NodeContent},
    node_tree::NodeTree,
};
use crate::{
    utils::{longid, shortid},
    Act, ActError, Branch, Result, Step, Workflow,
};
use std::sync::Arc;

pub fn build_workflow(workflow: &mut Workflow, tree: &mut NodeTree) -> Result<()> {
    let level = 0;
    if workflow.id.is_empty() {
        workflow.id = longid();
    }

    let data = NodeContent::Workflow(workflow.clone());
    let root = tree.make(&data.id(), data, level)?;

    let mut prev = root.clone();
    for step in workflow.steps.iter_mut() {
        build_step(step, tree, &root, &mut prev, level + 1)?;
    }

    tree.model = Box::new(workflow.clone());
    tree.set_root(&root);

    Ok(())
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
    let data = NodeContent::Step(step.clone());
    let node = tree.make(&data.id(), data, level)?;

    if node.level == prev.level {
        prev.set_next(&node, true);
    } else {
        node.set_parent(parent);
    }

    match &step.next {
        Some(next) => match &tree.node(next) {
            Some(next) => {
                node.set_next(next, false);
            }
            None => tree.set_error(ActError::Runtime(format!(
                "found next node error by '{}'",
                next
            ))),
        },
        None => {
            if !step.branches.is_empty() {
                let mut branch_prev = node.clone();
                for branch in step.branches.iter_mut() {
                    build_branch(branch, tree, &node, &mut branch_prev, level + 1)?;
                }
            }
        }
    }

    if !step.acts.is_empty() {
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
    let data = NodeContent::Branch(branch.clone());
    let node = tree.make(&data.id(), data, level)?;
    node.set_parent(parent);

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

    let data = NodeContent::Act(act.clone());
    let node = tree.make(&act.id, data, level)?;
    node.set_parent(parent);

    *prev = node.clone();

    Ok(())
}
