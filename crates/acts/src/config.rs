use serde::Deserialize;
use std::path::Path;
use toml::Table;

#[derive(Debug, Clone)]
pub struct Config {
    pub data: ConfigData,
    pub table: Table,
}

/// Hourly log files kept on disk when `[log].max_files` does not say
/// otherwise: one week of hourly files.
pub const DEFAULT_LOG_MAX_FILES: usize = 168;

/// Concurrent external effects a host runs when top-level `effect_concurrency`
/// does not say otherwise (A8). Effects run outside the pid's command lane, so
/// this is the one bound that can be reached by ordinary parallel workflows.
pub(crate) const DEFAULT_EFFECT_CONCURRENCY: usize = 256;

/// Concurrent recovery, reconciliation or projection commands a host runs when
/// the matching top-level key does not say otherwise (A8). These are control
/// plane passes; a handful is enough, and each has its own budget.
pub(crate) const DEFAULT_CONTROL_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigLog {
    pub dir: String,
    pub level: String,
    /// Hourly log files kept under `dir`, oldest removed first. Omitted uses
    /// [`DEFAULT_LOG_MAX_FILES`], `0` keeps every file. Read through
    /// [`ConfigLog::retained_files`], which applies the floor.
    #[serde(default)]
    pub max_files: Option<usize>,
}

impl ConfigLog {
    /// Number of hourly log files to keep under `dir`, or `None` to keep every
    /// file: `max_files` when configured (`0` disables the limit), else
    /// [`DEFAULT_LOG_MAX_FILES`]. At least 2 files are kept, because the
    /// appender prunes down to `max_files - 1` of the existing files before it
    /// writes the next one — `max_files = 1` would remove the file the server
    /// is still writing to.
    pub fn retained_files(&self) -> Option<usize> {
        match self.max_files {
            Some(0) => None,
            Some(files) => Some(files.max(2)),
            None => Some(DEFAULT_LOG_MAX_FILES),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ConfigData {
    /// How many runs may be in flight at once. A start beyond the cap is
    /// *parked*: its process row is persisted with the `None` state and
    /// nothing of the workflow runs until a run settles and the freed slot
    /// admits the parked run that has waited longest. The default is 1024.
    ///
    /// A negative value reads as "no bound" (`as usize`), exactly as it did
    /// when this bounded the resident process set.
    pub cache_cap: Option<i64>,
    /// Concurrent external effects (A8). `0` means unbounded; omitted uses
    /// 256. This budget is independent of [`cache_cap`](Self::cache_cap): an
    /// effect never consumes an execution slot, and a slow effect never holds
    /// the control plane's reconciliation quota.
    pub effect_concurrency: Option<usize>,
    /// Concurrent recovery scans and their exceptional commands (A8). `0`
    /// means unbounded; omitted uses 16.
    pub recovery_concurrency: Option<usize>,
    /// Concurrent manual reconciliations of `EffectUnknown`/`Quarantined`
    /// operations (A8). `0` means unbounded; omitted uses 16. Manual work
    /// never asks for an execution slot, so it cannot park a new run.
    pub reconciliation_concurrency: Option<usize>,
    /// Concurrent row projections and their repair work (A8). `0` means
    /// unbounded; omitted uses 16.
    pub projection_concurrency: Option<usize>,
    pub tick_interval_secs: Option<i64>,
    /// Interval of the ready/timer pass, in seconds (A10). This is normal-path
    /// liveness: the durable ready queue (including `DispatchTimeout` timers)
    /// is walked for due records, and an idle timer's record is deferred by
    /// this same interval. Omitted follows `tick_interval_secs`; values below
    /// 1 are clamped. It is independent of the recovery, outbox and projection
    /// intervals, so changing one never changes another loop's cadence.
    pub ready_interval_secs: Option<i64>,
    /// Interval of the recovery scanner, in seconds (A10). Recovery is the
    /// exceptional path (expired claims, prepared commits, effect unknowns,
    /// quarantine) and never the liveness source for ready work or timers.
    /// Omitted follows `tick_interval_secs`; values below 1 are clamped.
    pub recovery_interval_secs: Option<i64>,
    /// Interval of the outbox retry pass, in seconds (A10). The normal path
    /// delivers inside the operation's commit; this loop is the retry net for
    /// lost confirmations, deferred transports and expired delivery claims.
    /// Omitted follows `tick_interval_secs`; values below 1 are clamped.
    pub outbox_interval_secs: Option<i64>,
    /// Interval of the projection repair pass, in seconds (A10). Boot
    /// catch-up runs on this loop too; the in-memory repair queue is retried
    /// here, independently of recovery. Omitted follows
    /// `tick_interval_secs`; values below 1 are clamped.
    pub projection_interval_secs: Option<i64>,
    /// Interval of the schedule-trigger timer, in seconds (A10). Omitted
    /// follows `tick_interval_secs`; values below 1 are clamped.
    pub trigger_interval_secs: Option<i64>,
    /// Interval of the unacknowledged-message retry timer, in seconds (A10).
    /// Omitted follows `tick_interval_secs`; values below 1 are clamped.
    pub retry_interval_secs: Option<i64>,
    /// Interval of the snapshot-cache TTL sweep, in seconds (A10). Omitted
    /// follows `tick_interval_secs`; values below 1 are clamped.
    pub snapshot_interval_secs: Option<i64>,
    /// Operations the ready/timer pass drives per round (A10). At least 1,
    /// capped at 1024.
    pub ready_batch: Option<usize>,
    /// Exceptional operations the recovery scanner examines per round (A10).
    /// The scan resumes at the next index entry, so a large backlog is bounded
    /// work per round instead of one unbounded pass. At least 1, capped at
    /// 1024.
    pub recovery_batch: Option<usize>,
    /// Outbox records the retry pass delivers per round (A10). The pass
    /// resumes at the next record on the following round. At least 1, capped
    /// at 1024.
    pub outbox_batch: Option<usize>,
    /// Queued projection repairs retried per round (A10). At least 1, capped
    /// at 1024.
    pub projection_batch: Option<usize>,
    /// Interval of the terminal-run cleanup loop, in seconds. Omitted follows
    /// `tick_interval_secs`; the loop is independent of the driver's ready /
    /// recovery pass and of the message-retry timer, with its own budget and
    /// health. Values below 1 are clamped.
    pub cleanup_interval_secs: Option<i64>,
    /// How many terminal proc rows one cleanup pass examines (and how many
    /// unmarked rows it re-projects before deciding). At least 1.
    pub cleanup_batch: Option<usize>,

    /// max times an unacknowledged message delivery is re-sent before it
    /// turns into an `Error` that needs manual handling. Must be at least 1 —
    /// the engine refuses to start with a smaller value.
    pub max_message_retry_times: Option<i32>,
    /// max times a tree node can be executed in one process; protects against
    /// unbounded task creation caused by a node self-loop / cyclic `next`.
    /// Must be at least 1 — the engine refuses to start with a smaller value.
    pub max_node_run_times: Option<i64>,
    /// Maximum scheduler task lanes; defaults to available parallelism.
    /// Each lane executes one task at a time.
    /// Tasks for the same pid always hash to the same lane to preserve FIFO.
    pub scheduler_workers: Option<usize>,
    /// Maximum number of scheduler jobs buffered in memory, split evenly across
    /// the task lanes: a full lane overflows its producers to the durable
    /// outbox (or, for a fresh start, fails). Zero selects the default.
    pub scheduler_queue_cap: Option<usize>,
    /// Number of store-writer shards; defaults to available parallelism.
    /// Each shard applies one write at a time.
    /// Ops of the same pid always hash to the same shard to keep their
    /// enqueue order (task state durable before the outbox records that
    /// depend on it), so independent processes no longer queue behind one
    /// another's writes.
    pub store_writer_workers: Option<usize>,
    /// Maximum store-writer backlog, split evenly across the shards (at least
    /// one op each). Sizing is the refusal policy: a shard that runs out of
    /// room makes the writer refuse new work with `QueueFull` (its producers
    /// use their durable overflow path or report the overload) until the
    /// backlog drains, instead of buffering without limit or blocking every
    /// producer on a backed-up store. The bookkeeping of work already in
    /// flight waits for room instead of being refused. Zero selects the
    /// default.
    pub store_writer_queue_cap: Option<usize>,
    /// Filesystem root for process directories: a process runs in its own
    /// `<workdir>/<pid>`, which is what `Scope::workdir`/`Context::workdir`
    /// answer and what `$env.WORK_DIR` names. The directory lives exactly as
    /// long as the process's durable rows. Omitted means no directory
    /// control, and a process may touch whatever the server's own account
    /// can.
    pub workdir: Option<String>,
    // log config
    pub log: Option<ConfigLog>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: ConfigData::default(),
            table: Table::new(),
        }
    }
}

impl Config {
    pub fn create(path: &Path) -> crate::Result<Self> {
        let data = std::fs::read_to_string(path).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to load config file {}: {err}",
                path.display()
            ))
        })?;

        let table = toml::from_str::<Table>(&data).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the toml file({}): {err}",
                path.display()
            ))
        })?;

        let data = ConfigData::deserialize(table.clone()).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the config file({}): {err}",
                path.display()
            ))
        })?;

        Ok(Self { table, data })
    }

    pub fn get<'de, T>(&self, name: &str) -> crate::Result<T>
    where
        T: Deserialize<'de>,
    {
        let value = self
            .table
            .get(name)
            .ok_or_else(|| crate::ActError::Config(format!("config '{name}' does not exist")))?
            .clone();
        T::deserialize(value)
            .map_err(|err| crate::ActError::Config(format!("failed to get '{name}' config: {err}")))
    }

    pub fn has(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    /// Read another acts.toml and deep-merge it over this configuration: a
    /// key present in `path` overrides the base value, nested tables merge
    /// field by field (so an override file can change `[log].dir` while the
    /// `[log].level` from the base is kept), and scalar/array values replace
    /// wholesale. A missing file is a no-op; an unparsable file is an error.
    /// Used by `acts-server` to let local `acts.toml` files override the
    /// `~/.acts` defaults.
    pub fn overlay_file(&mut self, path: &Path) -> crate::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(path).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to load config file {}: {err}",
                path.display()
            ))
        })?;
        let table = toml::from_str::<Table>(&text).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the toml file({}): {err}",
                path.display()
            ))
        })?;
        merge_table(&mut self.table, table);
        self.data = ConfigData::deserialize(self.table.clone()).map_err(|err| {
            crate::ActError::Config(format!("failed to parse the merged config: {err}"))
        })?;
        Ok(())
    }

    /// The runaway protections cannot be switched off. `max_message_retry_times`
    /// bounds how often an unacknowledged delivery is re-sent before it needs
    /// manual handling, and `max_node_run_times` bounds how often one node can
    /// run inside a process before a looping workflow is errored instead of
    /// creating tasks without end — a value of 0 or below would disable one of
    /// those bounds, so a config carrying one is refused at engine start
    /// (`Runtime::create` is the one place every engine is built).
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if let Some(times) = self.data.max_message_retry_times
            && times < 1
        {
            return Err(crate::ActError::Config(format!(
                "max_message_retry_times must be at least 1 (got {times}); it bounds how \
                     often an unacknowledged message delivery is re-sent before it turns into \
                     an Error that needs manual handling (msg:resend / msg:clear)"
            )));
        }
        if let Some(times) = self.data.max_node_run_times
            && times < 1
        {
            return Err(crate::ActError::Config(format!(
                "max_node_run_times must be at least 1 (got {times}); it bounds how often \
                     one node can run inside a process, so a looping workflow errors instead of \
                     creating tasks forever"
            )));
        }
        // An empty workdir is an error rather than "no directory control":
        // the two cannot be told apart in the result, and a typo must not
        // silently drop the confinement.
        if let Some(workdir) = self.data.workdir.as_deref()
            && workdir.trim().is_empty()
        {
            return Err(crate::ActError::Config(
                "workdir cannot be empty; remove the key to run without directory control"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// How many runs may be in flight at once (see [`ConfigData::cache_cap`]).
    /// Defaults to 1024.
    pub fn cache_cap(&self) -> i64 {
        self.data.cache_cap.unwrap_or(1024)
    }
    /// Concurrent external effects (see [`ConfigData::effect_concurrency`]).
    pub fn effect_concurrency(&self) -> usize {
        self.data
            .effect_concurrency
            .unwrap_or(DEFAULT_EFFECT_CONCURRENCY)
    }
    /// Concurrent recovery commands (see [`ConfigData::recovery_concurrency`]).
    pub fn recovery_concurrency(&self) -> usize {
        self.data
            .recovery_concurrency
            .unwrap_or(DEFAULT_CONTROL_CONCURRENCY)
    }
    /// Concurrent manual reconciliations (see
    /// [`ConfigData::reconciliation_concurrency`]).
    pub fn reconciliation_concurrency(&self) -> usize {
        self.data
            .reconciliation_concurrency
            .unwrap_or(DEFAULT_CONTROL_CONCURRENCY)
    }
    /// Concurrent projections (see [`ConfigData::projection_concurrency`]).
    pub fn projection_concurrency(&self) -> usize {
        self.data
            .projection_concurrency
            .unwrap_or(DEFAULT_CONTROL_CONCURRENCY)
    }
    pub fn max_message_retry_times(&self) -> i32 {
        self.data.max_message_retry_times.unwrap_or(20)
    }
    pub fn max_node_run_times(&self) -> i64 {
        self.data.max_node_run_times.unwrap_or(1000)
    }
    pub fn tick_interval_secs(&self) -> i64 {
        self.data.tick_interval_secs.unwrap_or(15)
    }

    /// The ready/timer pass interval (A10): its own schedule, defaulting to
    /// the tick when unset. Never zero.
    pub fn ready_interval_secs(&self) -> i64 {
        self.data
            .ready_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The recovery scanner interval (A10): its own schedule, defaulting to
    /// the tick when unset. Never zero.
    pub fn recovery_interval_secs(&self) -> i64 {
        self.data
            .recovery_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The outbox retry interval (A10): its own schedule, defaulting to the
    /// tick when unset. Never zero.
    pub fn outbox_interval_secs(&self) -> i64 {
        self.data
            .outbox_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The projection repair interval (A10): its own schedule, defaulting to
    /// the tick when unset. Never zero.
    pub fn projection_interval_secs(&self) -> i64 {
        self.data
            .projection_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The schedule-trigger interval (A10): its own schedule, defaulting to
    /// the tick when unset. Never zero.
    pub fn trigger_interval_secs(&self) -> i64 {
        self.data
            .trigger_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The message-retry interval (A10): its own schedule, defaulting to the
    /// tick when unset. Never zero.
    pub fn retry_interval_secs(&self) -> i64 {
        self.data
            .retry_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// The snapshot TTL sweep interval (A10): its own schedule, defaulting to
    /// the tick when unset. Never zero.
    pub fn snapshot_interval_secs(&self) -> i64 {
        self.data
            .snapshot_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// Operations one ready pass drives (A10).
    pub fn ready_batch(&self) -> usize {
        self.data.ready_batch.unwrap_or(128).clamp(1, 1024)
    }

    /// Exceptional operations one recovery pass examines (A10).
    pub fn recovery_batch(&self) -> usize {
        self.data.recovery_batch.unwrap_or(128).clamp(1, 1024)
    }

    /// Outbox records one retry pass delivers (A10).
    pub fn outbox_batch(&self) -> usize {
        self.data.outbox_batch.unwrap_or(128).clamp(1, 1024)
    }

    /// Queued projection repairs retried per pass (A10).
    pub fn projection_batch(&self) -> usize {
        self.data.projection_batch.unwrap_or(32).clamp(1, 1024)
    }

    /// The terminal-run cleanup interval: its own schedule, defaulting to the
    /// tick when unset. Never zero.
    pub fn cleanup_interval_secs(&self) -> i64 {
        self.data
            .cleanup_interval_secs
            .unwrap_or_else(|| self.tick_interval_secs())
            .max(1)
    }

    /// Terminal proc rows one cleanup pass examines. Bounded so a large
    /// backlog costs one page per pass instead of an unbounded scan.
    pub fn cleanup_batch(&self) -> usize {
        self.data.cleanup_batch.unwrap_or(256).clamp(1, 4096)
    }

    /// Maximum concurrently executing scheduler jobs. This is also the explicit
    /// in-flight admission limit for the fixed task-lane pool.
    pub fn scheduler_workers(&self) -> usize {
        let configured = self
            .data
            .scheduler_workers
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get() / 2));
        configured.clamp(1, 1024)
    }

    /// Maximum in-memory scheduler backlog, split evenly across the task lanes
    /// (`scheduler_workers`), so resident `Arc<Task>`/`Arc<Process>` data is
    /// bounded by the cap — each lane keeps at least one job, so a cap below the
    /// lane count admits one job per lane instead. The durable outbox is the
    /// overflow queue.
    pub fn scheduler_queue_cap(&self) -> usize {
        self.data
            .scheduler_queue_cap
            .unwrap_or(4096)
            .clamp(1, 1_048_576)
    }

    /// Number of concurrent store-writer shards. A pid always hashes to one
    /// shard, so the ops of a process are applied in enqueue order while
    /// independent processes are persisted concurrently.
    pub fn store_writer_workers(&self) -> usize {
        let configured = self
            .data
            .store_writer_workers
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get() / 2));
        configured.clamp(1, 1024)
    }

    /// Maximum ops queued across the store-writer shards, split evenly
    /// (`store_writer_workers`, at least one op per shard). A saturated writer
    /// refuses new work with `QueueFull` until its backlog drains; the
    /// bookkeeping of work already in flight waits for room instead.
    pub fn store_writer_queue_cap(&self) -> usize {
        self.data
            .store_writer_queue_cap
            .unwrap_or(16384)
            .clamp(1, 1_048_576)
    }

    /// The configured process-directory root, if any (see
    /// [`ConfigData::workdir`]).
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
        self.data
            .workdir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(std::path::PathBuf::from)
    }

    pub fn log(&self) -> ConfigLog {
        self.data.log.clone().unwrap_or(ConfigLog {
            dir: "log".to_string(),
            level: "INFO".to_string(),
            max_files: None,
        })
    }
}

/// Merge `over` into `base` in place: nested tables merge field by field,
/// every other value replaces the base value for that key.
fn merge_table(base: &mut Table, over: Table) {
    for (key, value) in over {
        if let Some(existing) = base.get_mut(&key) {
            match (existing, value) {
                (toml::Value::Table(base_table), toml::Value::Table(over_table)) => {
                    merge_table(base_table, over_table);
                }
                (slot, value) => *slot = value,
            }
        } else {
            base.insert(key, value);
        }
    }
}

/// Controls behavior when a snapshot target's key params or data are absent
/// at a task's prepare step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingParamAction {
    /// Silently skip sealing data for this task.
    #[default]
    Skip,
    /// Return an error listing the missing parameters.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per-test scratch dir so parallel runs never collide.
    fn scratch(name: &str) -> std::path::PathBuf {
        let thread = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let dir = std::env::temp_dir().join(format!(
            "acts-config-{}-{}-{}",
            std::process::id(),
            name,
            thread
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn overlay_file_merges_nested_tables_field_by_field() {
        let dir = scratch("merge");
        let base = write(
            &dir,
            "base.toml",
            r#"
cache_cap = 1024
[log]
dir = "data"
level = "INFO"
"#,
        );
        let over = write(
            &dir,
            "over.toml",
            r#"
[log]
dir = "other"
"#,
        );

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&over).unwrap();

        // the override's [log].dir won, the base's level survived
        assert_eq!(config.log().dir, "other");
        assert_eq!(config.log().level, "INFO");
        assert_eq!(config.cache_cap(), 1024);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_file_replaces_scalars_and_adds_new_keys() {
        let dir = scratch("scalar");
        let base = write(
            &dir,
            "base.toml",
            "cache_cap = 1024\n[log]\ndir = \"data\"\nlevel = \"INFO\"\n",
        );
        let over = write(&dir, "over.toml", "cache_cap = 512\n[web]\nport = 10082\n");

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&over).unwrap();

        assert_eq!(config.cache_cap(), 512);
        assert!(config.has("web"));
        assert_eq!(config.log().dir, "data");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_file_missing_is_a_noop_and_unparsable_is_an_error() {
        let dir = scratch("missing");
        let base = write(
            &dir,
            "base.toml",
            "[log]\ndir = \"data\"\nlevel = \"INFO\"\n",
        );

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&dir.join("nope.toml")).unwrap();
        assert_eq!(config.log().level, "INFO");

        let bad = write(&dir, "bad.toml", "this is not [ valid toml");
        assert!(config.overlay_file(&bad).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn log_file_retention_defaults_and_parses_max_files() {
        let dir = scratch("log-retention");

        // a [log] section written before max_files existed still parses, and
        // falls back to a bounded file count instead of keeping every hourly
        // file forever
        let base = write(
            &dir,
            "base.toml",
            "[log]\ndir = \"data\"\nlevel = \"INFO\"\n",
        );
        let config = Config::create(&base).unwrap();
        assert_eq!(config.log().max_files, None);
        assert_eq!(config.log().retained_files(), Some(DEFAULT_LOG_MAX_FILES));

        // an explicit count is honored, 0 disables retention, and fewer than 2
        // files cannot be honored (the appender prunes the existing files down
        // to max_files - 1 before writing the next one)
        for (line, want) in [
            ("max_files = 48", Some(48)),
            ("max_files = 0", None),
            ("max_files = 1", Some(2)),
        ] {
            let path = write(
                &dir,
                "limit.toml",
                &format!("[log]\ndir = \"data\"\nlevel = \"INFO\"\n{line}\n"),
            );
            let config = Config::create(&path).unwrap();
            assert_eq!(config.log().retained_files(), want, "{line}");
        }

        // the shortcut used by embedders keeps the default count as well
        assert_eq!(
            Config::default().log().retained_files(),
            Some(DEFAULT_LOG_MAX_FILES)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn admission_quotas_have_independent_defaults_and_overrides() {
        let defaults = Config::default();
        assert_eq!(defaults.effect_concurrency(), DEFAULT_EFFECT_CONCURRENCY);
        for value in [
            defaults.recovery_concurrency(),
            defaults.reconciliation_concurrency(),
            defaults.projection_concurrency(),
        ] {
            assert_eq!(value, DEFAULT_CONTROL_CONCURRENCY);
        }

        let dir = scratch("admission-quotas");
        let path = write(
            &dir,
            "limits.toml",
            "cache_cap = 3\neffect_concurrency = 7\nrecovery_concurrency = 2\nreconciliation_concurrency = 5\nprojection_concurrency = 11\n",
        );
        let config = Config::create(&path).unwrap();
        assert_eq!(config.cache_cap(), 3);
        assert_eq!(config.effect_concurrency(), 7);
        assert_eq!(config.recovery_concurrency(), 2);
        assert_eq!(config.reconciliation_concurrency(), 5);
        assert_eq!(config.projection_concurrency(), 11);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn control_plane_intervals_default_to_the_tick_and_move_independently() {
        let defaults = Config::default();
        for interval in [
            defaults.ready_interval_secs(),
            defaults.recovery_interval_secs(),
            defaults.outbox_interval_secs(),
            defaults.projection_interval_secs(),
            defaults.trigger_interval_secs(),
            defaults.retry_interval_secs(),
            defaults.snapshot_interval_secs(),
        ] {
            assert_eq!(interval, defaults.tick_interval_secs());
        }

        let dir = scratch("loop-intervals");
        let path = write(
            &dir,
            "loops.toml",
            "tick_interval_secs = 15\nready_interval_secs = 1\nrecovery_interval_secs = 90\noutbox_interval_secs = 2\nprojection_interval_secs = 30\ntrigger_interval_secs = 7\nretry_interval_secs = 9\nsnapshot_interval_secs = 120\n",
        );
        let config = Config::create(&path).unwrap();
        // One loop moving never moves another.
        assert_eq!(config.ready_interval_secs(), 1);
        assert_eq!(config.recovery_interval_secs(), 90);
        assert_eq!(config.outbox_interval_secs(), 2);
        assert_eq!(config.projection_interval_secs(), 30);
        assert_eq!(config.trigger_interval_secs(), 7);
        assert_eq!(config.retry_interval_secs(), 9);
        assert_eq!(config.snapshot_interval_secs(), 120);
        // An unset loop still follows the tick.
        let with_tick = write(&dir, "tick.toml", "tick_interval_secs = 4\n");
        let config = Config::create(&with_tick).unwrap();
        assert_eq!(config.ready_interval_secs(), 4);
        assert_eq!(config.recovery_interval_secs(), 4);

        // A configured zero is clamped rather than turned into a busy loop.
        let zero = write(&dir, "zero.toml", "recovery_interval_secs = 0\n");
        let config = Config::create(&zero).unwrap();
        assert_eq!(config.recovery_interval_secs(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_reports_missing_and_invalid_files() {
        let dir = std::env::temp_dir().join(format!("acts-config-create-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(Config::create(&dir.join("missing.toml")).is_err());

        let path = dir.join("invalid.toml");
        std::fs::write(&path, "this is not [ valid toml").unwrap();
        assert!(Config::create(&path).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
