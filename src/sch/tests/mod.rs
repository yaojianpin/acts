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
use crate::{event::Emitter, Engine, Signal, Workflow};
use std::sync::Arc;

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let runtime = Engine::new();
    let scher = runtime.scher().clone();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let sig = scher.signal(());
    let s1 = sig.clone();
    let s2 = sig.clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            s1.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s2.close();
    });
    (proc, scher, emitter)
}

fn create_proc_signal<R: Clone + Default + Sync + Send + 'static>(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Proc>,
    Arc<Scheduler>,
    Arc<Emitter>,
    Signal<R>,
    Signal<R>,
) {
    let runtime = Engine::new();
    let scher = runtime.scher().clone();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let sig = scher.signal(R::default());
    let rx2 = sig.clone();
    let rx3 = sig.clone();
    emitter.on_complete(move |p| {
        println!("message: {p:?}");
        if p.inner().state.is_completed() {
            rx2.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        rx3.close();
    });
    (proc, scher, emitter, sig.clone(), sig.clone())
}

fn create_proc_signal2<R: Clone + Default + Send + 'static>(
    workflow: &Workflow,
    pid: &str,
) -> (Engine, Arc<Proc>, Signal<R>, Signal<R>) {
    let engine = Engine::new();
    let scher = engine.scher().clone();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let sig = scher.signal(R::default());
    let rx2 = sig.clone();
    let rx3 = sig.clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            rx2.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        rx3.close();
    });
    (engine, proc, sig.clone(), sig.clone())
}
