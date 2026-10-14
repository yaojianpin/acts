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
    build_owned_workflow(workflow.clone(), tree)
}

pub(crate) fn build_owned_workflow(mut workflow: Workflow, tree: &mut NodeTree) -> Result<()> {
    let level = 0;
    if workflow.id.is_empty() {
        workflow.id = longid();
    }

    prepare_workflow_ids(&mut workflow);

    let workflow = Arc::new(workflow);
    let data = NodeContent::Workflow(workflow.clone());
    let root = tree.make(&data.id(), data, level)?;

    let mut prev = root.clone();
    for step in workflow.steps.iter() {
        build_step(
            step,
            tree,
            &root,
            &mut prev,
            level + 1,
            NodeOutputKind::Normal,
        )?;
    }

    tree.model = workflow;
    tree.set_root(&root);

    Ok(())
}

pub fn build_step(
    step: &Step,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
    typ: NodeOutputKind,
) -> Result<()> {
    if step.next.is_some() && step.r#while.is_some() {
        return Err(ActError::Runtime(format!(
            "step '{}' cannot combine 'while' with 'next': a while loop always flows to the next declared step when its condition fails",
            step.id
        )));
    }
    let data = NodeContent::Step(step.clone());
    let node = tree.make(&data.id(), data, level)?;

    if node.level == prev.level {
        prev.link_chain(&node);
    } else {
        node.set_parent_in(typ, parent);
    }

    match &step.next {
        Some(next) => match &tree.node(next) {
            Some(next) => {
                node.set_next(next, false);
            }
            None => {
                return Err(ActError::Runtime(format!(
                    "found next node error by '{next}'",
                )));
            }
        },
        None => {
            if !step.branches.is_empty() {
                let mut branch_prev = node.clone();
                for branch in step.branches.iter() {
                    build_branch(branch, tree, &node, &mut branch_prev, level + 1)?;
                }
            }
        }
    }

    if !step.catches.is_empty() {
        let mut catch_prev = node.clone();
        for catch in step.catches.iter() {
            build_step(
                catch,
                tree,
                &node,
                &mut catch_prev,
                level + 1,
                NodeOutputKind::Catch,
            )?;
        }
    }
    if !step.timeouts.is_empty() {
        let mut timeout_prev = node.clone();
        for timeout in step.timeouts.iter() {
            build_step(
                timeout,
                tree,
                &node,
                &mut timeout_prev,
                level + 1,
                NodeOutputKind::Timeout,
            )?;
        }
    }

    // create a step chain from the step array
    *prev = node;

    Ok(())
}

pub fn build_branch(
    branch: &Branch,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) -> Result<()> {
    let data = NodeContent::Branch(branch.clone());
    let node = tree.make(&data.id(), data, level)?;
    node.set_parent(parent);

    let mut step_prev = node.clone();
    for step in branch.steps.iter() {
        build_step(
            step,
            tree,
            &node,
            &mut step_prev,
            level + 1,
            NodeOutputKind::Normal,
        )?;
    }

    *prev = node;

    Ok(())
}

/// Assign identifiers before the workflow is shared as an immutable `Arc`.
fn prepare_workflow_ids(workflow: &mut Workflow) {
    for step in workflow.steps.iter_mut() {
        prepare_step_ids(step);
    }
}

fn prepare_step_ids(step: &mut Step) {
    if step.id.is_empty() {
        step.id = shortid();
    }

    for branch in step.branches.iter_mut() {
        prepare_branch_ids(branch);
    }
    for catch in step.catches.iter_mut() {
        prepare_step_ids(catch);
    }
    for timeout in step.timeouts.iter_mut() {
        prepare_step_ids(timeout);
    }
}

fn prepare_branch_ids(branch: &mut Branch) {
    if branch.id.is_empty() {
        branch.id = shortid();
    }

    for step in branch.steps.iter_mut() {
        prepare_step_ids(step);
    }
}

// pub fn build_act(
//     act: &mut Act,
//     tree: &mut NodeTree,
//     parent: &Arc<Node>,
//     prev: &mut Arc<Node>,
//     level: usize,
//     is_sequence: bool,
//     typ: NodeOutputKind,
// ) -> Result<()> {
//     if act.id.is_empty() {
//         act.id = shortid();
//     }

//     let data = NodeContent::Act(act.clone());

//     let node = tree.make(&act.id, data, level)?;

//     if is_sequence {
//         // set the act order one by one
//         if node.level == prev.level {
//             prev.set_next(&node, true);
//         } else {
//             node.set_parent_in(typ, parent);
//         }
//         *prev = node.clone();
//     } else {
//         node.set_parent_in(typ, parent);
//     }

//     Ok(())
// }

pub fn dyn_build_act(
    act: &mut Act,
    tree: &NodeTree,
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
    let node = tree.append_node(parent, &act.id, data, level)?;

    if is_sequence {
        // set the act order one by one
        if node.level == prev.level {
            prev.set_next(&node, true);
        } else {
            node.set_parent(parent);
        }
        *prev = node;
    } else {
        node.set_parent(parent);
    }
    Ok(())
}
