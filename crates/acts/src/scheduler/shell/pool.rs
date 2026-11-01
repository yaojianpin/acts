//! The scheduler's fixed worker pool.
//!
//! One bounded FIFO and one consumer task per lane; every operation is routed
//! by its pid's [`LaneRouter`](crate::utils::LaneRouter) lane, so operations of
//! one process always land on the same lane and are executed one after another
//! by the same worker, while independent processes execute concurrently on
//! different lanes.
//!
//! A lane's queue is an accelerator, not the source of truth: every queued
//! operation also owns a durable ready record. A full lane drops the enqueue,
//! and the ready pass picks the record up again once the worker has drained
//! room. The `queued` map coalesces the wake and ready-scan copies of one
//! operation: the first source that enqueues it wins, and a ready copy upgrades
//! a wake copy so the worker still defers the record when the operation turns
//! out to be idle.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::{scheduler::core::model::operation::OperationId, utils::LaneRouter};

/// Why an operation was handed to the pool. A `Ready` job owns a durable ready
/// record that the worker defers when the operation is not due; a `Wake` job
/// only needs the operation driven now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JobSource {
    Wake,
    Ready,
}

/// What [`LanePool::enqueue`] did with one operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Enqueued {
    /// The operation entered its lane's queue.
    Yes,
    /// The operation was already queued; a ready source upgraded a wake one.
    AlreadyQueued,
    /// The lane had no room. The durable ready record is the retry.
    Full,
}

struct Lane {
    tx: mpsc::Sender<OperationId>,
    /// The operations accepted but not yet taken by this lane's worker, with
    /// the strongest source seen. It bounds the queue to one entry per
    /// operation.
    queued: Mutex<HashMap<OperationId, JobSource>>,
}

/// The pid-routed worker queues and the consumers' receiving ends.
pub(super) struct LanePool {
    router: LaneRouter,
    lanes: Vec<Lane>,
    receivers: Mutex<Option<Vec<mpsc::Receiver<OperationId>>>>,
    /// Whether a driver task took the receivers and routes wakes here. Before
    /// that (the deterministic test harness drives operations inline) wakes
    /// keep using the host's wake channel.
    active: AtomicBool,
    /// A lane refused an enqueue since the last read. The next completed job
    /// wakes the ready pass, which re-scans due records for the refused work.
    saturated: AtomicBool,
}

impl LanePool {
    /// `lanes` workers, each with a queue of `capacity / lanes` operations (at
    /// least one), and the receivers their tasks must consume.
    pub(super) fn new(lanes: usize, capacity: usize) -> Self {
        let router = LaneRouter::new(lanes);
        let lanes_count = router.lanes();
        let lane_capacity = (capacity.max(1) / lanes_count).max(1);
        let mut lanes = Vec::with_capacity(lanes_count);
        let mut receivers = Vec::with_capacity(lanes_count);
        for _ in 0..lanes_count {
            let (tx, rx) = mpsc::channel(lane_capacity);
            lanes.push(Lane {
                tx,
                queued: Mutex::new(HashMap::new()),
            });
            receivers.push(rx);
        }
        Self {
            router,
            lanes,
            receivers: Mutex::new(Some(receivers)),
            active: AtomicBool::new(false),
            saturated: AtomicBool::new(false),
        }
    }

    /// The lane a pid belongs to.
    pub(super) fn lane_of(&self, pid: &str) -> usize {
        self.router.lane_of(pid)
    }

    /// Number of worker lanes.
    #[cfg(test)]
    pub(super) fn lanes(&self) -> usize {
        self.lanes.len()
    }

    /// Take the consumers' receiving ends. `None` after the driver already
    /// took them: that host runs operations inline (the test harness).
    pub(super) fn take_receivers(&self) -> Option<Vec<mpsc::Receiver<OperationId>>> {
        self.receivers.lock().take()
    }

    /// Route wakes into the pool from now on.
    pub(super) fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Stop routing wakes into the pool (the driver is leaving).
    pub(super) fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    /// Whether wakes route into the pool.
    pub(super) fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Queue `id` on its pid's lane. The operation's ready record remains the
    /// durable truth when the lane is full.
    pub(super) fn enqueue(&self, id: &OperationId, source: JobSource) -> Enqueued {
        let lane = &self.lanes[self.lane_of(&id.process_id)];
        {
            let mut queued = lane.queued.lock();
            if let Some(current) = queued.get_mut(id) {
                // The ready pass saw the operation due while it still waits
                // behind a wake: keep the deferral behaviour of a ready job.
                if source == JobSource::Ready {
                    *current = JobSource::Ready;
                }
                return Enqueued::AlreadyQueued;
            }
            queued.insert(id.clone(), source);
        }
        match lane.tx.try_send(id.clone()) {
            Ok(()) => Enqueued::Yes,
            Err(_) => {
                lane.queued.lock().remove(id);
                self.saturated.store(true, Ordering::Release);
                Enqueued::Full
            }
        }
    }

    /// Take the source of a job just received by `lane`'s worker. It also
    /// removes the job from the coalescing map, so a wake arriving while the
    /// operation runs queues a fresh turn.
    pub(super) fn take(&self, lane: usize, id: &OperationId) -> Option<JobSource> {
        self.lanes.get(lane)?.queued.lock().remove(id)
    }

    /// Whether a lane refused an enqueue since the last read.
    pub(super) fn take_saturated(&self) -> bool {
        self.saturated.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::task_graph::TaskId;

    fn operation(pid: &str, sequence: u64) -> OperationId {
        OperationId {
            process_id: pid.to_string(),
            task: TaskId::new("s1", 0).unwrap(),
            sequence,
        }
    }

    /// Two pids whose XXH3 lanes differ — a deterministic search, so the test
    /// never depends on particular names hashing apart.
    fn pids_on_distinct_lanes(pool: &LanePool) -> (String, String) {
        let first = "p-pool-a".to_string();
        let lane = pool.lane_of(&first);
        for index in 0..10_000 {
            let candidate = format!("p-pool-b-{index}");
            if pool.lane_of(&candidate) != lane {
                return (first, candidate);
            }
        }
        panic!("no pid landed on another lane");
    }

    #[test]
    fn same_pid_jobs_share_a_lane_and_keep_their_order() {
        let pool = LanePool::new(4, 64);
        let receivers = pool.take_receivers().expect("fresh pool has receivers");
        let (a, b) = pids_on_distinct_lanes(&pool);
        let first = operation(&a, 1);
        let second = operation(&a, 2);
        let other = operation(&b, 1);
        let lane_a = pool.lane_of(&a);
        let lane_b = pool.lane_of(&b);
        assert_ne!(lane_a, lane_b);

        assert_eq!(pool.enqueue(&first, JobSource::Wake), Enqueued::Yes);
        assert_eq!(pool.enqueue(&second, JobSource::Wake), Enqueued::Yes);
        assert_eq!(pool.enqueue(&other, JobSource::Wake), Enqueued::Yes);

        let mut receivers = receivers;
        assert_eq!(receivers[lane_a].try_recv().unwrap(), first);
        assert_eq!(receivers[lane_a].try_recv().unwrap(), second);
        assert!(
            receivers[lane_a].try_recv().is_err(),
            "one pid's jobs stay on one lane, in enqueue order"
        );
        assert_eq!(receivers[lane_b].try_recv().unwrap(), other);
        assert_eq!(pool.take(lane_a, &first), Some(JobSource::Wake));
        assert_eq!(pool.take(lane_a, &second), Some(JobSource::Wake));
        assert_eq!(pool.take(lane_b, &other), Some(JobSource::Wake));
    }

    #[test]
    fn a_ready_scan_upgrades_a_queued_wake_without_a_second_job() {
        let pool = LanePool::new(1, 8);
        let mut receivers = pool.take_receivers().unwrap();
        let id = operation("p-upgrade", 1);

        assert_eq!(pool.enqueue(&id, JobSource::Wake), Enqueued::Yes);
        assert_eq!(pool.enqueue(&id, JobSource::Ready), Enqueued::AlreadyQueued);
        assert_eq!(pool.enqueue(&id, JobSource::Wake), Enqueued::AlreadyQueued);
        assert_eq!(receivers[0].try_recv().unwrap(), id);
        assert!(receivers[0].try_recv().is_err(), "only one job was queued");
        assert_eq!(pool.take(0, &id), Some(JobSource::Ready));
    }

    #[test]
    fn a_full_lane_refuses_the_job_and_leaves_it_to_the_ready_index() {
        let pool = LanePool::new(1, 1);
        let mut receivers = pool.take_receivers().unwrap();
        let first = operation("p-full", 1);
        let second = operation("p-full", 2);

        assert_eq!(pool.enqueue(&first, JobSource::Wake), Enqueued::Yes);
        assert_eq!(pool.enqueue(&second, JobSource::Wake), Enqueued::Full);
        assert!(pool.take_saturated(), "the refusal saturates the pool");
        assert!(!pool.take_saturated());

        assert_eq!(receivers[0].try_recv().unwrap(), first);
        assert_eq!(pool.take(0, &first), Some(JobSource::Wake));
        // Room is back: the refused operation can now be queued.
        assert_eq!(pool.enqueue(&second, JobSource::Wake), Enqueued::Yes);
    }
}
