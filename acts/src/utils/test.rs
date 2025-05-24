use crate::{
    Engine, Signal, Workflow,
    scheduler::{Process, Runtime},
};
use std::sync::Arc;

#[allow(clippy::type_complexity)]
pub fn create_proc_signal<R: Clone + Default + Sync + Send + 'static>(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Process>,
    Arc<Runtime>,
    Arc<crate::export::Channel>,
    Signal<R>,
    Signal<R>,
) {
    create_proc_signal_with_auto_clomplete(workflow, pid, true)
}

#[allow(clippy::type_complexity)]
pub fn create_proc_signal_with_auto_clomplete<R: Clone + Default + Sync + Send + 'static>(
    workflow: &mut Workflow,
    pid: &str,
    auto_complete: bool,
) -> (
    Arc<Process>,
    Arc<Runtime>,
    Arc<crate::export::Channel>,
    Signal<R>,
    Signal<R>,
) {
    let engine = Engine::new().start();
    let rt = engine.runtime();

    let proc = rt.create_proc(pid, workflow);

    let emitter = engine.channel().clone();
    let sig = engine.signal(R::default());
    let rx2 = sig.clone();
    let rx3 = sig.clone();

    if auto_complete {
        emitter.on_complete(move |p| {
            // println!("message: {p:?}");
            if p.state().is_completed() {
                rx2.close();
            }
        });
    }

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        rx3.close();
    });

    (proc, rt, emitter, sig.clone(), sig.clone())
}
