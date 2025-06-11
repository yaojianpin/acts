use super::{
    node::{Node, NodeContent, NodeOutputKind},
    node_tree::NodeTree,
};
use crate::{
    Act, ActError, Branch, Result, Step, Workflow,
    utils::{longid, shortid},
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
        build_step(
            step,
            tree,
            &root,
            &mut prev,
            level + 1,
            NodeOutputKind::Normal,
            None,
        )?;
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
    typ: NodeOutputKind,
    on: Option<String>,
) -> Result<()> {
    if step.id.is_empty() {
        step.id = shortid();
    }
    let data = NodeContent::Step(step.clone());
    let node = tree.make(&data.id(), data, level)?;

    if node.level == prev.level {
        prev.set_next(&node, true);
    } else {
        node.set_parent_in(typ, on, parent);
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
            build_act(act, tree, &node, &mut act_prev, level + 1, true)?;
        }
    }

    if !step.catches.is_empty() {
        let mut catch_prev = node.clone();
        for catch in step.catches.iter_mut() {
            for step in catch.steps.iter_mut() {
                build_step(
                    step,
                    tree,
                    &node,
                    &mut catch_prev,
                    level + 1,
                    NodeOutputKind::Catch,
                    catch.on.clone(),
                )?;
            }
        }
    }
    if !step.timeout.is_empty() {
        let mut timeout_prev = node.clone();
        for timeout in step.timeout.iter_mut() {
            for step in timeout.steps.iter_mut() {
                build_step(
                    step,
                    tree,
                    &node,
                    &mut timeout_prev,
                    level + 1,
                    NodeOutputKind::Timeout,
                    Some(timeout.on.clone()),
                )?;
            }
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
        build_step(
            step,
            tree,
            &parent,
            &mut step_prev,
            level + 1,
            NodeOutputKind::Normal,
            None,
        )?;
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
    is_sequence: bool,
) -> Result<()> {
    if act.id.is_empty() {
        act.id = shortid();
    }

    let data = NodeContent::Act(act.clone());

    let node = tree.make(&act.id, data, level)?;

    if is_sequence {
        // set the act order one by one
        if node.level == prev.level {
            prev.set_next(&node, true);
        } else {
            node.set_parent(parent);
        }
        *prev = node.clone();
    } else {
        node.set_parent(parent);
    }

    if !act.catches.is_empty() {
        let mut catch_prev = node.clone();
        for catch in act.catches.iter_mut() {
            for step in catch.steps.iter_mut() {
                build_step(
                    step,
                    tree,
                    &node,
                    &mut catch_prev,
                    level + 1,
                    NodeOutputKind::Catch,
                    catch.on.clone(),
                )?;
            }
        }
    }
    if !act.timeout.is_empty() {
        let mut timeout_prev = node.clone();
        for timeout in act.timeout.iter_mut() {
            for step in timeout.steps.iter_mut() {
                build_step(
                    step,
                    tree,
                    &node,
                    &mut timeout_prev,
                    level + 1,
                    NodeOutputKind::Timeout,
                    Some(timeout.on.clone()),
                )?;
            }
        }
    }

    Ok(())
}

pub fn dyn_build_act(
    act: &mut Act,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
    _index: usize,
    is_sequence: bool,
) -> Result<()> {
    if act.id.is_empty() {
        act.id = shortid();
    }

    let data = NodeContent::Act(act.clone());
    let node = parent.append_node(&act.id, data, level);

    if is_sequence {
        // set the act order one by one
        if node.level == prev.level {
            prev.set_next(&node, true);
        } else {
            node.set_parent(parent);
        }
        *prev = node.clone();
    } else {
        node.set_parent(parent);
    }
    Ok(())
}
