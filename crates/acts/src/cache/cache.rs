use super::writer::{StoreWriter, WriteOp};
use crate::{
    Action, Config, Result,
    data::MessageStatus,
    scheduler::{Process, Runtime, Task},
    store::{KvStore, MemoryStore, Store},
};
use moka::sync::Cache as MokaCache;
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, instrument};
#[derive(Clone)]
pub struct Cache {
    cap: usize,
    procs: MokaCache<String, Arc<Process>>,
    store: Arc<Store>,
    writer: StoreWriter,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("cap", &self.cap())
            .field("count", &self.count())
            .finish()
    }
}

impl Cache {
    pub fn new(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Self> {
        let store = Arc::new(Store::new(
            store.unwrap_or_else(|| Arc::new(MemoryStore::new())),
        ));
        Ok(Self {
            cap: config.cache_cap() as usize,
            procs: MokaCache::new(config.cache_cap() as u64),
            store: store.clone(),
            writer: StoreWriter::spawn(store),
        })
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn count(&self) -> usize {
        self.procs.run_pending_tasks();
        self.procs.entry_count() as usize
    }

    pub fn close(&self) {
        self.writer.close();
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub fn push_proc(&self, proc: &Arc<Process>) -> Result<()> {
        self.push_proc_pri(proc, true)?;

        Ok(())
    }

    pub fn procs(&self) -> Vec<Arc<Process>> {
        let mut procs = Vec::new();
        for (_, proc) in self.procs.iter() {
            procs.push(proc.clone());
        }
        procs
    }

    #[instrument(skip(self, rt), fields(pid = %pid))]
    pub fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Result<Option<Arc<Process>>> {
        debug!("process: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Ok(Some(proc.clone())),
            None => {
                self.flush()?;
                if let Some(proc) = self.store.load_proc(pid, rt)? {
                    debug!(pid = %pid, "loaded process");
                    // add to cache
                    self.push_proc_pri(&proc, false)?;
                    return Ok(Some(proc));
                }
                Ok(None)
            }
        }
    }

    #[instrument(skip(self), fields(pid = %pid))]
    pub fn remove(&self, pid: &str) -> Result<bool> {
        debug!("remove pid={pid}");
        self.procs.remove(pid);
        // Removal is serialized through the writer (FIFO) so it can never
        // race writes still queued for the process — its completion markers
        // are applied first, then the rows are dropped. `flush` keeps the
        // callers' synchronous contract: when this returns, the removal is
        // durable and no pending write can resurrect the rows afterwards.
        self.writer.send(WriteOp::RemoveProc {
            pid: pid.to_string(),
        })?;
        self.writer.flush()?;
        Ok(true)
    }

    #[instrument(skip(self, rt))]
    pub fn restore(&self, rt: &Arc<Runtime>) -> Result<()> {
        debug!("restore");
        let cap = self.cap();
        let count = self.count();
        let mut check_point = cap / 2;
        if check_point == 0 {
            check_point = cap;
        }
        if count < check_point {
            // skip procs already in the cache to avoid redundant deserialization
            let cached: HashSet<String> = self.procs().iter().map(|p| p.id().to_string()).collect();
            let cap = cap - count;
            self.flush()?;
            for ref proc in self.store.load(cap, rt, &cached)? {
                if !self.procs.contains_key(proc.id()) {
                    self.push_proc_pri(proc, false)?;
                    if proc.state().is_none() {
                        proc.start()?;
                    }
                }
            }
        }
        Ok(())
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub fn upsert(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_pri(task, true)
    }

    #[cfg(test)]
    pub fn uncache(&self, pid: &str) {
        self.procs.remove(pid);
    }

    fn get_proc(&self, pid: &str) -> Option<Arc<Process>> {
        self.procs.get(pid)
    }

    pub(super) fn push_proc_pri(&self, proc: &Arc<Process>, save: bool) -> Result<()> {
        debug!("push process pid={}", proc.id());
        if save {
            self.store.upsert_proc(proc)?;
        }
        self.procs.insert(proc.id().to_string(), proc.clone());

        Ok(())
    }

    /// Persist a freshly started process and its root task as ONE atomic
    /// store batch, then cache the process and register the root task in
    /// memory — the very first durable write of a process, so a crash can
    /// never leave a durable proc row without its root task row (which would
    /// resume as a task-less, un-runnable process). The rows are durable
    /// before the root task is dispatched to the queue.
    #[instrument(skip(self, proc, root), fields(pid = %proc.id()))]
    pub(crate) fn start_proc(&self, proc: &Arc<Process>, root: Option<&Arc<Task>>) -> Result<()> {
        debug!("start process pid={}", proc.id());
        self.store.upsert_proc_with_task(proc, root)?;
        self.procs.insert(proc.id().to_string(), proc.clone());
        if let Some(task) = root {
            self.push_task_mem(task)?;
        }
        Ok(())
    }

    pub(super) fn push_task_pri(&self, task: &Arc<Task>, save: bool) -> Result<()> {
        if save {
            self.persist_task(task)?;
        }
        self.push_task_mem(task)?;

        Ok(())
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub(crate) fn upsert_async(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_mem(task)?;
        self.writer.send(WriteOp::Task(task.clone()))?;
        Ok(())
    }

    pub(crate) fn upsert_message_status(
        &self,
        pid: &str,
        tid: &str,
        status: MessageStatus,
    ) -> Result<()> {
        self.writer.send(WriteOp::MessageStatus {
            pid: pid.to_string(),
            tid: tid.to_string(),
            status,
        })
    }

    /// Durable outbox enqueue for a `next` operation. The `Pending` record is
    /// queued on the writer **after** the task state change the caller already
    /// queued (`emit_task`), so FIFO order guarantees the task is durable
    /// before the record — without blocking the caller. A crash leaves either
    /// nothing (consistent, no replay) or the record, which recovery replays.
    pub(crate) fn enqueue_next(&self, task: &Arc<Task>) -> Result<()> {
        self.writer.send(WriteOp::EnqueueNext {
            pid: task.pid.clone(),
            tid: task.id.clone(),
        })
    }

    /// Durable outbox enqueue for a client action: the `Pending` record (with
    /// the event + options payload) is queued **before** the action is applied
    /// in memory, so a crash before the state write lands is replayed on
    /// recovery. Deduplicated per `(pid, tid)` against any other in-flight
    /// record.
    pub(crate) fn enqueue_action(&self, action: &Action) -> Result<()> {
        self.writer.send(WriteOp::EnqueueAction {
            pid: action.pid.clone(),
            tid: action.tid.clone(),
            event: action.event.as_ref().to_string(),
            options: action.options.to_string(),
        })
    }

    /// Durable outbox close for a client action: the state write (and the
    /// message status) were already queued by the caller, so FIFO order makes
    /// `Done` durable only after both.
    pub(crate) fn complete_action(&self, task: &Arc<Task>) -> Result<()> {
        self.writer.send(WriteOp::OpDone {
            pid: task.pid.clone(),
            tid: task.id.clone(),
            r#type: crate::data::OpType::Action.as_ref().to_string(),
        })
    }

    /// Durable outbox close: queue the task persist (capturing the
    /// `NEXT_COMPLETE` marker), then queue the record close after it — FIFO
    /// order makes `Done` durable only after the marker, without blocking the
    /// event loop. If the process crashes between the two, the record is still
    /// `Pending` and recovery re-dispatches it; the durable marker turns the
    /// re-run into a no-op. Safe to call repeatedly: already-closed records
    /// are left untouched.
    pub(crate) fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        self.upsert_async(task)?;
        self.writer.send(WriteOp::OpDone {
            pid: task.pid.clone(),
            tid: task.id.clone(),
            r#type: crate::data::OpType::Next.as_ref().to_string(),
        })?;
        Ok(())
    }

    pub(crate) fn flush(&self) -> Result<()> {
        self.writer.flush()
    }

    fn persist_task(&self, task: &Arc<Task>) -> Result<()> {
        let p = task.proc();
        self.store.upsert_task(task)?;
        // update root task data when updating task
        if let Some(root) = p.root() {
            self.store.upsert_task(&root)?;
        }
        // update process to store when process state is completed
        if p.state().is_completed() {
            self.store
                .mark_proc_complete(&task.pid, p.end_time(), p.state())?;
        }

        Ok(())
    }

    fn push_task_mem(&self, task: &Arc<Task>) -> Result<()> {
        let p = task.proc();
        if let Some(proc) = self.procs.get(&task.pid) {
            proc.set_pure_state(p.state());
            proc.set_end_time(p.end_time());
            proc.push_task(task.clone())?;
        }

        Ok(())
    }
}
