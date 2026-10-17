//! TEMPORARY profiling probe (P7 task 1a). Deleted with the timers in
//! `shell::operation_trace` and the §10 record they feed.
//!
//! The engine path's per-phase breakdown: a 30-step workflow driven through
//! [`Engine`] with the timers on, so one operation's load / plan / deliver /
//! commit / wake / lifecycle / projection / slot work is reported in ms per
//! operation and as a share of the run. Run with
//! `cargo test --release -p acts --lib perf_probe -- --nocapture --ignored
//! --test-threads=1`.

use super::*;

use crate::{Engine, Step, Vars, Workflow};

/// A 30-step workflow of plain steps (no act, no branches): 31 nodes × 3
/// operations each.
fn thirty_step_workflow() -> Workflow {
    let mut workflow = Workflow::new().with_id("wf-thirty");
    workflow.steps = (0..30)
        .map(|index| Step::new().with_id(&format!("s{index}")))
        .collect();
    workflow
}

/// The per-phase operation breakdown of the engine path, three rounds.
#[tokio::test]
#[ignore]
async fn perf_probe_engine_operation_stages() {
    for round in 0..3 {
        let engine = Engine::builder()
            .set_store(std::sync::Arc::new(MemoryStore::new()))
            .start()
            .await
            .unwrap();
        let pid = format!("stagestruct{round}");
        let workflow = {
            let mut workflow = thirty_step_workflow();
            workflow.id = pid.clone();
            workflow
        };
        operation_trace::enable();
        let start = std::time::Instant::now();
        engine
            .runtime()
            .start(
                &workflow,
                Vars::new().with(crate::utils::consts::PROCESS_ID, pid.as_str()),
            )
            .await
            .unwrap();
        loop {
            if engine
                .runtime()
                .kernel()
                .process_completed(&pid)
                .await
                .unwrap()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_micros(200)).await;
        }
        let wall = start.elapsed();
        let totals = operation_trace::disable();
        let operations = totals[operation_trace::OPERATIONS] as f64;
        let process = engine
            .runtime()
            .kernel()
            .snapshot(&pid)
            .await
            .unwrap()
            .expect("aggregate exists");
        let bytes = crate::scheduler::durable::document::ProcessDocumentCodec::encode(&process)
            .unwrap()
            .len();

        let mut timed = 0u64;
        eprintln!(
            "engine operation stages round {round}: operations={operations} wall={wall:?} ms/operation={:.3} aggregate_bytes={bytes}",
            wall.as_secs_f64() * 1000.0 / operations,
        );
        for (index, name) in operation_trace::NAMES.iter().enumerate() {
            if index == operation_trace::OPERATIONS {
                continue;
            }
            let nanos = totals[index];
            timed += nanos;
            eprintln!(
                "  {name:<10} {:>8.3} ms/operation  ({:>5.1}% of the run)",
                nanos as f64 / 1_000_000.0 / operations,
                nanos as f64 / wall.as_nanos() as f64 * 100.0,
            );
        }
        eprintln!(
            "  {:<10} {:>8.3} ms/operation  ({:>5.1}% of the run)",
            "timed sum",
            timed as f64 / 1_000_000.0 / operations,
            timed as f64 / wall.as_nanos() as f64 * 100.0,
        );
        engine.close().await;
    }
}
