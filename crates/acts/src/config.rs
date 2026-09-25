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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigData {
    pub cache_cap: Option<i64>,
    pub tick_interval_secs: Option<i64>,

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
    /// `<workdir>/<pid>`, which is what `Process::workdir`/`Context::workdir`
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

    pub fn cache_cap(&self) -> i64 {
        self.data.cache_cap.unwrap_or(1024)
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
