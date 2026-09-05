use acts::{Engine, MessageState, Vars, Workflow};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let iters = 2000u32;
    let text = r#"
id: act_test
ver: 0.1.0
steps:
  - id: step1
    uses: acts.core.irq
    params:
      key: act1
"#;
    let workflow = Workflow::from_yml(text).unwrap();

    let mut config = acts::Config::default();
    config.data.cache_cap = Some(100_000);
    let engine = Engine::new()
        .with_config(&config)
        .start()
        .await
        .expect("failed to start engine");
    engine
        .executor()
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(()).double();
    let tasks = Arc::new(Mutex::new(Vec::new()));
    let tasks2 = tasks.clone();
    let chan = engine.channel();
    chan.on_message(move |e| {
        let tasks2 = tasks2.clone();
        let s = s.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                let mut t = tasks2.lock();
                t.push((e.pid.clone(), e.tid.clone()));
                if t.len() >= iters as usize {
                    s.close();
                }
            }
        }
    });

    for _ in 0..iters {
        engine
            .executor()
            .proc()
            .start(&workflow.id, Vars::new())
            .await
            .unwrap();
    }
    sig.recv().await;
    let tasks = {
        let guard = tasks.lock();
        println!("pre-created {} tasks", guard.len());
        guard.clone()
    };

    // warmup
    let (pid, tid) = &tasks[0];
    let _ = engine
        .executor()
        .act()
        .complete(pid, tid, Vars::new())
        .await;

    let start = Instant::now();
    for (pid, tid) in tasks.iter().skip(1) {
        engine
            .executor()
            .act()
            .complete(pid, tid, Vars::new())
            .await
            .unwrap();
    }
    let total = start.elapsed();
    println!(
        "complete x{}: {:?}  avg {:.3} µs",
        iters - 1,
        total,
        total.as_micros() as f64 / (iters - 1) as f64
    );
}
