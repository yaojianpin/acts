use super::{
    node::{Node, NodeData},
    node_tree::NodeTree,
};
use crate::{utils::shortid, ActError, Branch, Job, Step, Workflow};
use std::sync::Arc;

pub fn process_workflow(workflow: &mut Workflow, tree: &mut NodeTree) {
    let level = 0;
    if workflow.id.is_empty() {
        workflow.id = shortid();
    }

    let mut jobs = Vec::new();
    for job in workflow.jobs.iter_mut() {
        let node = process_job(job, tree, level + 1);
        jobs.push(node);
    }

    // in the end to set the job node parent
    // to make sure the workflow's jobs is the newest
    let data = NodeData::Workflow(workflow.clone());
    let root = tree.make("", data, level);
    for job in jobs {
        job.set_parent(&root);
    }
    tree.set_root(&root);
}

pub fn process_job(job: &mut Job, tree: &mut NodeTree, level: usize) -> Arc<Node> {
    if job.id.is_empty() {
        job.id = shortid();
    }
    let data = NodeData::Job(job.clone());
    let node = tree.make(&job.id, data, level);

    let mut prev = node.clone();
    for step in job.steps.iter_mut() {
        process_step(&job.id, step, tree, &node, &mut prev, level + 1);
    }

    node.clone()
}

pub fn process_step(
    root: &str,
    step: &mut Step,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) {
    if step.id.is_empty() {
        step.id = shortid();
    }
    let data = NodeData::Step(step.clone());
    let node = tree.make(root, data, level);

    // only set the nodes in save level
    if node.level == prev.level {
        prev.set_next(&node);
    } else {
        node.set_parent(&parent);
    }

    let next = step.next.clone();
    match next {
        Some(next_id) => {
            let flow_option = tree.node(&next_id);
            match flow_option {
                Some(next) => {
                    *node.next.write().unwrap() = Arc::downgrade(&next);
                }
                None => tree.set_error(ActError::NextError(next_id).into()),
            }
        }
        None => {
            if step.branches.len() > 0 {
                let mut branch_prev = node.clone();
                for branch in step.branches.iter_mut() {
                    process_branch(root, branch, tree, &node, &mut branch_prev, level + 1);
                }
            }
        }
    }

    *prev = node.clone();
}

pub fn process_branch(
    root: &str,
    branch: &mut Branch,
    tree: &mut NodeTree,
    parent: &Arc<Node>,
    prev: &mut Arc<Node>,
    level: usize,
) {
    if branch.id.is_empty() {
        branch.id = shortid();
    }
    let data = NodeData::Branch(branch.clone());
    let node = tree.make(root, data, level);
    if node.level == prev.level {
        prev.set_next(&node);
    }
    prev.set_next(&node);
    node.set_parent(&parent);
    // parent.children.write().unwrap().push(node.clone());

    let parent = node.clone();
    let mut step_prev = node.clone();
    for step in branch.steps.iter_mut() {
        process_step(root, step, tree, &parent, &mut step_prev, level + 1);
    }

    *prev = node.clone();
}
