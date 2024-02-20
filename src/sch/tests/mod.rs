mod act;
mod message;
mod proc;
mod scher;
mod state;
mod step;
mod task;
mod tree;
mod vars;
mod workflow;

use super::{Proc, Scheduler};
use crate::{event::Emitter, Engine, Executor, Manager, Workflow};
use std::sync::Arc;

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            p.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        p.close();
    });
    (proc, scher, emitter)
}

fn create_proc2(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Proc>,
    Arc<Scheduler>,
    Arc<Emitter>,
    Arc<Executor>,
    Arc<Manager>,
) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let executor = engine.executor().clone();
    let manager = engine.manager().clone();
    let s = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s.close();
    });
    (proc, scher, emitter, executor, manager)
}
