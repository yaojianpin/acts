use acts::{Engine, MessageState, Vars, Workflow};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() {
    // Processes armed before measurement. 2000 is the stress profile (the
    // completion cost depends on the live set); `PROF_ITERS=8` measures the
    // same fixed batch `benches/workflow.rs` times, which is the like-for-like
    // comparison with the criterion benchmark.
    let iters = std::env::var("PROF_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2000u32)
        .max(2);
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
    let engine = Engine::builder()
        .set_config(&config)
        .start()
        .await
        .expect("failed to start engine");
    engine
        .executor(&acts::Principal::unrestricted())
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
            .executor(&acts::Principal::unrestricted())
            .proc()
            .start(&workflow.id, Vars::new())
            .await
            .unwrap();
    }
    sig.recv().await;
    // Close the arming subscription before measuring: an open channel receives
    // every completion's events, which charges the measurement for message
    // fan-out that `benches/workflow.rs` (no channel registered) does not pay.
    chan.close();
    let tasks = {
        let guard = tasks.lock();
        println!("pre-created {} tasks", guard.len());
        guard.clone()
    };

    // The executor is bound once: constructing it per operation would put the
    // executor's own setup inside the sample, which the criterion benchmark
    // (and a real caller) does not pay per completion.
    let principal = acts::Principal::unrestricted();
    let executor = engine.executor(&principal);

    // warmup
    let (pid, tid) = &tasks[0];
    let _ = executor.act().complete(pid, tid, Vars::new()).await;

    let start = Instant::now();
    let mut samples = Vec::with_capacity(tasks.len().saturating_sub(1));
    for (pid, tid) in tasks.iter().skip(1) {
        let op = Instant::now();
        executor
            .act()
            .complete(pid, tid, Vars::new())
            .await
            .unwrap();
        samples.push(op.elapsed());
    }
    let total = start.elapsed();
    samples.sort_unstable();
    let pick = |p: f64| {
        samples[(((p / 100.0) * samples.len() as f64).ceil() as usize).clamp(1, samples.len()) - 1]
    };
    println!(
        "complete x{}: {:?}  avg {:.3} µs  p50 {:.3} µs  p95 {:.3} µs  p99 {:.3} µs  max {:.3} µs",
        samples.len(),
        total,
        total.as_micros() as f64 / samples.len() as f64,
        pick(50.0).as_secs_f64() * 1e6,
        pick(95.0).as_secs_f64() * 1e6,
        pick(99.0).as_secs_f64() * 1e6,
        samples.last().unwrap().as_secs_f64() * 1e6,
    );
}
