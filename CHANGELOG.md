# 0.10.0

- add `tokio_local!` to make env module working with `Context`
- add `quickjs` runtime in env module
- use `quickjs` runtime in `pack` instead of `wit`
- remove `start` function from `Engine`
- add `Builder` to build engine with different config
- add workflow `env` to support all workflows can get env vars and set locally
- simplified the options of the `error` action
- merge action state to task state
- add engine channel to receive messages by options and the channel messages can re-send if not acked

# 0.10.1

- remove the warning code
- fix the doc test error
- rename engine.emitter to engine.channel
- rename data::message emit_id to chan_id, emit_pattern to chan_pattern
- delete data::message emit_count
- remove default feature

# 0.10.2

- update readme.md
- add homepage

# 0.10.3

- remove action result, the time will caculate by acts-channel
- refactor the info struct to make is easier to understand.

# 0.10.4

- modify the test error with 'store' feature

# 0.10.5

- remove the warnings in rust 1.82
- remove the duckdb bundle feature

# 0.10.6

- reset the build mode to bundled for store feature

# 0.11.0

- change store db to sqlite

# 0.12.0

- change the act yml format, use act: xx instead of !xx
- add setup to act and remove on_created, on_completed
- add act.expose for pack
- add nid for Message
- use 'do' act instead of 'cmd'
- expands executor with msg(), pack(), proc(), task(), act() and mode() instead of manager

# 0.12.1

- update act.set_output to act.expose
- keep act.expose only expose the vars to outputs
- fix the model tree output issue

# 0.12.3

- fix the test error with feature store

# 0.12.4

- fix test error for act each result check issue

# 0.12.5

- add export.msg unsub to support unsubscribe the messages by client
- fix the deadlock issue by subscribing with same client id by many times

# 0.13.0

- change the the query function to return PageData in trait DbSet for store collection
- add query_by and order_by to query fn
- add `mid` to message collection
- add ExecutorQuery to export list fn for msg, pack, proc, task, message

# 0.13.1

- fix: fix the memory store query error

# 0.13.2

- upgrate rquickjs to 0.8.1

# 0.13.3

- merge Luminvent's change
- fix: fix the clippy error

# 0.14.0

- improve the code quality
- add set_process_var for act_execution

# 0.15.0

- feat: allow to keep processes after completion
- fix: set process state if task is completed and is root task [#12](https://github.com/yaojianpin/acts/issues/12)

# 0.16.0

- feat: reafactoring the act package to support act extension [#8]
- feat: adding package meta struct to support package jsonschema definition [#9]
- feat: add acts-sqlite plugin
- feat: add acts-postgres plugin #[13]
- feat: add acts.cfg support
- feat: modify Config to support getting custom config section
- feat: add workflow.on events (manual, hook, chat)
- feat: add acts-state package to support get or set state
- feat: change the directory structure with acts, plugins, examples, benches, config

# 0.17.0
- feat: reactoring env module to add register_var trait
- feat: add "resects" user var to support get resects data from task context
- feat: add step env module to support get step vars by step id
- feat: change env var to $env
- feat: rename proc env_local to env
- feat: change event package params to Option<T>
- feat: skip initialization of plugin when there is no related section
- feat: add `acts.core.http` package plugin
- feat: add `pid` prefix to the acts.app.state package
- feat: support private vars (starts with `__`)
- feat: use `acts.toml` instead of `acts.cfg`
- feat: add `env.expose` in workflow to support set default outputs, the default outputs is `data`
- feat: add `acts.app.shell` package plugin to support nushell, bash and powershell
        support {{ }} refs the var in shell script
- feat: add `os` var in expression
- feat: use {{ }} for expression instead of ${ }
- feat: use var name directly in script or expression instead of $("var")
        use $get("var") instead of $("var")
        use $set("var", value) instead of $("var", value)
        use $inputs() instead of $act.inputs()
        use $data() instead of $act.data()

# 0.17.1
- fix: fix examples/plugins build issue when exclude examples

# 0.17.2
- fix: add info.MessageInfo uses property

# 0.18.0
- feat: change the workspace structure
- feat: use Query instead of ExecuteQuery
- feat: add `match` operation to FilterExpr
- feat: add `ui_schema` property to `ActPackageMeta`
- feat: add `vars` property to `Workflow`, `Step` and `Act`
- feat: change `inputs` and `outputs` to Json schema definitions of workflow
- feat: add `metadata` property to store UI data
- feat: remove `setup` property of `Workflow`, `Step` and `Act`
- feat: add `start_from_model` to export.proc
- feat: move `message.model` to `message.inputs.model`
- feat: refactoring `catches` and `timeouts` for `step` and `act`
- feat: change `vars`, `env` and `options.exposes` to `Vec<Variant>`
- feat: change `workflow.inputs` and `workflow.outputs` to `ActSchema`
- fix: fix the audit issues
       1. fix dependencies sqlx-mysql `rsa` issue （Timing Side-Channel Attack）
       2. fix dependencies rquickjs `dlopen_derive` `unmaintained` issue
- feat: add `rn` resource name to workflow and step
- feat: update workflow.ver to String type
- feat: add `desc` to store models
- feat: add dashmap for Package mod
- feat: add safe limit for quickjs runtime
- feat: change db store to kv store
        add db with sqlite, postgres, nats, redis, sled

# 0.19.0
- feat: remove `acts` from step struct
- feat: add `uses` and `params` to step struct
- feat: remove `catches` and `timeouts` from act struct
- feat: change `catches` and `timeouts` list from `Vec<Act>` to `Vec<Step>` in step 
- feat: remove `key` from message struct, the `key` info should be migrate to `params` properties
- feat: add document for english and chinese language
- feat: add default workflow version
- fix: fix issue #16 not save the data after setting the process vars

# 0.19.1
- fix: fix sch_task_start and pack_parallel_setup_list run fail

# 0.20.0
- remove `tag` and `rn` from workflow, step, branch and act
- use `options` to support the user custom extension, such as `tag`, `rn`
- add `exposes` to workflow, step, branch and act instead of options.exposes
- change `timeouts` and `catches` to support complex flow like `steps`
- add `Engine::builder()` to `Engine` and remove the export of EngineBuilder
- merge `ActPackageFn`  into into `ActPackage`
- change expression `{{ }}` to `${{ }}`
- merge the task logic `review` to `next`
- feat: add uses action check in  `task.next` before run into children tasks

# 0.20.1
- fix the package publish version issue closes #17

# 0.21.0
- migrates `acts-channel` from https://github.com/yaojianpin/acts-channel
- add plugin `acts-plugin-grpc` and `acts-plugin-web`
- add resolver to `EngineBuilder` and `Engine`
- feat: `console.log` supports multiple parameters
- feat: add `acts-package-nats` pubsub package
- feat: modify `execute` of `ActPackage` to async
- feat: change `ActTask` `run` to async
- feat: change `cache.upsert` to `cache.upsert_async` to improve performance
- perf: move `task.next` to async queue
- perf: optimize in-memory scan_prefix performance.
- perf: optimize `convert::get_expr` and `convert::get_exprs` to improve `fill_params` performance
- perf: optimize list iter and add shutdown token to runtime
- fix: act data cannot update to parent when abort or error task, test cases #pack_action_abort_on_step_with_inputs, pack_action_error_on_step_with_inputs
- feat: add tracing log for key function
- fix: fix examples/simple exposes type error

# 0.21.1
- fix: fix acts-channel version dependency issue, close #18

# 0.22.0
- change rust ci branch to `main` and `develep`
- remove truncate data issue in store/postgres
- feat: `store-*` features only control compilation; the matching store struct is exported when enabled
- feat: select the store with `EngineBuilder::set_store(Arc<dyn KvStore>)`, only one store is allowed, default is in-memory
- BREAKING: remove `Extender::register_store` and the runtime store override
- BREAKING: remove the `db` config section (`DbConfig`); create the backend struct externally and pass it to `set_store`
- fix: fix the event delivered to handlers in FIFO order
- feat: add `set_config` to `EngineBuilder` and unit test
- fix: change task id format with `{pid}{tid}`, remove `SEP` between `pid` and `tid`
- fix: add QuickJS interrupt handle to deal with deadline issue
- feat: change PostgresStore connection from global to local
- fix: make `task.next` propagation crash-safe with a durable outbox — pending `next` operations are recorded in a new `ops` store collection before in-memory dispatch and closed only after the task (with the `NEXT_COMPLETE` marker) is durably persisted; recovery replays unfinished records, and re-scheduling is deduplicated, so reloading after a crash never loses or duplicates propagation (removes the non-durable `NEXT_PENDING` sign)
- fix: make `task.action` propagation crash-safe with a durable outbox
- fix: fix the hang issue when executing `pack_irq_multi_threads`
- fix: bound the re-execution of a tree node per process — add `max_node_run_times` (default 1000, `EngineBuilder::max_node_run_times`, 0 disables); a node `next` self-loop or cycle now errors the process instead of creating tasks forever
- fix: `process.do_tick` fires timeouts only for tasks still in flight — completed, error, aborted and skipped tasks no longer schedule their timeout children on every tick
- fix: keep `TaskTree.push` only insert and not replace old task
- fix: one-shot atomic start process
- fix: not return error when tree build error
- fix: add `limit`, `ExprOp::Between`，`ExprOp::In` validation in store query
- fix: `StoreWriter.close` flushes, stops and joins the writer thread — when it returns no writer thread is left running, and later `send`/`flush` calls fail with a channel-closed error (close previously returned without joining the thread)
- fix: store writer `flush` reports the first failure of the writes queued since the previous flush — a failing write is surfaced through the barrier instead of acksing `Ok`; `Cache::flush` returns `Result`
- fix: serialize process removal through the store writer (`RemoveProc` op) — `cache.remove` no longer deletes the process rows directly while its completion markers are still queued, so removal can never race the pending writes; task writes reaching the writer after the removal are skipped instead of resurrecting rows or erroring
- fix: store index range scans were off by one at the exact value boundary — `Between`/`Le` dropped `value == to` and `Gt` included it — because index keys embed the record id after the value, so no key string can bound a whole value group; range/inequality ops now translate to half-open full-key intervals closed by a sentinel bound (`KEY_SEP_SUCC`), making inclusive/degenerate ranges and single-sided comparisons exact on every backend
- fix: `-` is `KEY_SEP` yet passed through the index value encoding, so `Eq`/`Ne` on `x` also matched stored values like `x-2`, and hyphenated strings leaked across range bounds by id order; `-` is now escaped (`=2D`) in value segments
- fix: string range/inequality queries on indexed fields fall back to the exact full-data scan (escaped characters do not sort in code-point order), keeping results correct; numeric fields keep the exact index path
- feat: add `Store::rebuild_indexes` / `KvCollection::rebuild_index` to recreate index entries after the encoding change — run once when upgrading existing stores (old hyphen-encoded index keys are dropped)
- fix: store `query` with `order_by` now sorts the whole matching set BEFORE pagination, so every page is the global top-N slice instead of a re-sorted arbitrary batch; multiple `order_by` fields apply in listed priority with per-field `asc`/`desc`; numbers sort numerically (previously compared as text, so `10` < `9`); a document missing or `null` on a sort key no longer panics the comparator — it sorts first on `asc` (last on `desc`), and rows tied on every key fall back to ascending id so offsets stay deterministic across pages
- feat: steps can now be written as bounded `while` loops — add `while: <cond>` and the step re-executes while the condition holds, then falls through to the next declared step when it fails (no self-`next` needed; `while` cannot be combined with `next`)
- fix: a step’s explicit `next` jump is preserved at model build — the following step in declaration order no longer clobbers it, and a step skipped by its `if`/`while` condition falls through to the next declared step instead of re-entering a self/backward `next`; the `max_node_run_times` guard still errors true (unconditional) cycles
- fix: stop sending the message if message retry update fails
- fix: make `Signal` one-shot fires broadcast — a single `send`/`close` now releases every concurrent `recv` waiter and receivers joining after the fire return immediately; previously `Notify::notify_one` woke at most one receiver, so the rest hung forever (`update` closures may still call `close` on the same signal)
- refactor: replace `std::sync::{Mutex, RwLock}` with `parking_lot` everywhere — locks are infallible (no `unwrap`/`map_err` at call sites) and guards are smaller; `tokio::sync` stays for async-held locks
- fix: stop sending message if `store_if` returns `false`
- BREAKING: messages are now stored split: the canonical emitted `Message` (one row per message id, payload stored once) in the `messages` collection, and one `Delivery` row per (message × channel/service) in a new `deliveries` collection — Ack/Retry/Clear/Redo, the retry timer and task-completion close-outs all operate on delivery rows keyed by their own delivery id; every channel delivery of the same event shares one `msg_id` but has its own `delivery_id`, so multiple grpc/SSE clients (and future nats/kafka adapters) ack independently
- feat: each delivered event carries `delivery_id`; channel handlers store the canonical message once and create one delivery row per ack channel, tagging the handler event with the new delivery id so the client can ack exactly its own delivery
- feat: retry re-sends are routed to the owning channel only (`Emitter::emit_delivery`) — an acked channel never sees another channel's retries; each redelivery reuses the same delivery id for exactly-once consumer dedup
- feat: message manager ops — `msg:ack`, `msg:get`, `msg:rm` key on delivery ids; `msg:clear`/`msg:redo` accept an `id` for one delivery while keeping the batch `pid`/all forms (`MessageExecutor::clear_delivery`, `redeliver`)
- feat: delivery rows expose `msg_id`/`chan_id` indexes; `MessageInfo` joins delivery state with its canonical message
- migration: existing v1 merged message rows read as canonical messages (delivery state is not carried over); run `Store::rebuild_indexes` once when upgrading existing stores
- BREAKING: `Workflow.on` entries are now triggers (`kind` + `params`), not `Act`s — replace `uses: acts.event.manual|hook|chat` with `kind: manual|chat|hook`. Triggers only declare the workflow start surface; `Act` stays for in-process steps/actions
- feat: add `Trigger` model (`kind`: `manual`/`chat`/`hook`/`schedule`, or a registered event package id for custom triggers); `EventInfo` exposes `kind` + schedule run state
- feat: `schedule` triggers — engine timer polls due cron rows (`sec min hour day month dow`) and starts the workflow; run state (`last_run`/`next_run`) is persisted on the trigger row, survives restarts, and manual firing is refused
- feat: web URL triggers — a `manual` trigger fired over HTTP doubles as a webhook; `acts-plugin-web` adds `POST /hooks/{model-id}:{trigger-id}` that starts the declared trigger with the request body as payload and returns the process id (no separate `webhook` kind)
- fix: model re-deploy now reconciles trigger rows — changed triggers update, removed triggers are deleted (previously stale rows stayed fireable); trigger rows keep their schedule state across re-deploys unless the cron changed
- BREAKING: remove the `acts.event.manual|hook|chat` packages (superseded by trigger kinds)
- migration: existing v0 `events` rows (`uses: acts.event.X`) are upcast to `kind` on load
- fix: collection writes are atomic per document — `KvStore` gains `batch` (applies a `StoreBatchOp` list all-or-nothing via a native transaction on memory/sled/sqlite/postgres/redis, sequential best-effort on nats), and `DbCollection::create`/`update`/`delete` now commit the data row and its index rows in one batch, so a mid-write failure can no longer leave a document without its indexes (or index rows without the data row)
- fix: model `deploy` is atomic across the model row and its trigger (`events`) rows — trigger reconciliation (create/update/stale-drop, schedule state preservation) moved from `ModelExecutor` into `Store::deploy` and commits with the model row in one `KvStore::batch`, so a mid-deploy failure can no longer leave a deployed model with half-reconciled triggers
- fix: model removal is atomic too — `ModelExecutor::rm` now deletes the model row and its trigger (`events`) rows in one `KvStore::batch` (moved into `Store::rm_model`), so a mid-removal failure can no longer leave stale trigger rows behind
- fix: process removal is atomic — `Store::remove_proc` deletes the process's task rows, durable outbox (`ops`) rows and the proc row in one `KvStore::batch` (`remove_proc_rows`), so a crash mid-removal can no longer leave a half-deleted process that would resurrect broken on restore
- fix: a process's first persist is atomic — `Process::start` now writes the proc row and its root task row as one `KvStore::batch` (`Cache::start_proc` → `Store::upsert_proc_with_task`) before the root task is dispatched, closing the crash window where a durable Running proc row had no task rows yet and would resume as an un-runnable, task-less process
- fix: `Engine::start` failure now releases the partially started runtime — the store writer thread, event loop, recovery writes and any timer tasks are torn down (`rt.close()`) before returning the error, instead of leaking a live writer std-thread and polling timers on an engine that never became usable
- BREAKING: `EventExecutor::start` (and its `start_hook` helper) is now `async` — the `hook` trigger awaits the completion signal directly (`sig.recv().await`) instead of parking a `sync::block_on`; callers of `executor.evt().start(...)` must `.await` it
- BREAKING: change `ActTask::init` to `async`
- BREAKING: `KvStore` / `DbCollection` are now `async` traits (`#[async_trait]`; native `async fn` cannot be dyn-dispatched) — implementers of custom stores must make every method `async` and add `#[async_trait::async_trait]`
- BREAKING: store backends open asynchronously — `SqliteStore::open`/`open_in_memory`, `PostgresStore::open`, `NatsStore::open`, `RedisStore::open` are now `async` (sqlite keeps one connection behind a `tokio` mutex; redis moves to the async multiplexed client); `MemoryStore`/`SledStore` are unchanged
- BREAKING: the whole store façade is async — `Store` (incl. the `cache` writer extensions), `KvCollection`/`DbCollection`, `Cache` (`proc`/`remove`/`restore`/`flush`/`close`/…), and the scheduler event/timer/recovery paths await directly; `utils::sync::block_on` and the dedicated second tokio runtime are removed, so store operations no longer panic on current-thread runtimes or steal worker/blocking threads on multi-thread runtimes
- BREAKING: the store writer is a tokio task instead of a std thread — FIFO ordering and flush-barrier semantics are unchanged, but `StoreWriter::flush`/`close` (and `Cache::flush`/`close`, `Engine::close`) are `async`
- BREAKING: `Engine::start` and `Engine::close` are now `async` (start recovers pending actions and drains the writer on failure; close drains and joins the writer task)
- BREAKING: all executor data methods are `async` — `model().deploy/list/get/rm`, `proc().start/start_from_model/list/get/get_process`, `act().submit/back/cancel/complete/abort/skip/fail/push/remove/set_process_vars/do_action`, `msg().list/get/ack/rm/clear/redo/clear_delivery/redeliver/unsub`, `task().list/get`, `pack().publish/list/get/rm`, `evt().list/get`; registration/accessor methods stay sync
- BREAKING: channel and emitter handlers are now async closures — `on_message/on_start/on_complete/on_error/on_proc/on_task(|e| async move { … })` with the event owned by value; events of one process are delivered in emission order (never concurrent), while different processes run concurrently — a slow handler only stalls its own process
- BREAKING: `Extender::register_package`, `package::init`, `ActPackage::start` and `ActPackageDefinition` publishing are async (a plugin that published packages from `ActPlugin::on_init` must publish them from an async context instead)
- BREAKING: `SqliteStore` no longer exposes the `conn` field; `RedisStore` no longer exposes a synchronous `redis::Connection`
- perf: store operations are true awaits on the ambient runtime — no `block_in_place`, no per-op thread hop, no second runtime; PG/NATS round trips no longer pin threads, redis no longer blocks a worker inline
- fix: engine store ops work under current-thread `#[tokio::main]`/`#[tokio::test]` runtimes (previously any async-context store op with a real backend panicked on `block_in_place`)
- BREAKING: remove the `keep_processes` config option — a finished process is always cleaned up (its proc/task/outbox/message/delivery rows are deleted once its deliveries have settled); there is no "keep completed processes" mode anymore
- BREAKING: the delivery row status type is now its own `DeliveryStatus` (renamed from `MessageStatus`; same integer codes, zero DB migration) with an explicit lifecycle — `Created` (row stored, not yet successfully handed over) → `Delivered` (delivery succeeded: the channel handler ran to completion; marked only from `Created`, never downgrading a row the handler acked/closed) → `Acked` (client confirmed, intermediate) → `Completed` (engine close, final) — `Error` (retries exhausted) is resolved only by a manual resend/clear and keeps its process alive until then
- feat: process cleanup is now driven by delivery settlement instead of the terminal state — a finished process is not deleted synchronously; when a task reaches a terminal state the engine-authoritative writer closes the task's own delivery rows (`Completed`) and re-checks the proc's `removable` mark (a process with no delivery rows — every message done with its own state — or all deliveries settled is marked on the spot); the retry-timer sweep then deletes marked processes through the writer (`RemoveProc`, FIFO after any still-queued writes) in one atomic batch with their message/delivery/outbox/task rows, so an `Error` delivery (manual handling) or still-settling rows can never race the removal
- fix: index scans for `In`/`Ne`/range/`Between` recovered the record id with the LAST `-` separator — an id containing `-` (e.g. a user-supplied process id) was truncated and the row silently dropped from results; the value segment can never contain `-` (escaped), so the FIRST separator is the value/id boundary
- fix: a late `msg:ack` no longer downgrades a delivery the engine already closed — `Completed` is final and a stale ack is a no-op (previously the ack reverted the row to the intermediate `Acked` and could un-settle the process)
- fix: `msg:rm`/manager views aside, deliveries of a finished process are removed with it — unacked `Created`/`Delivered` rows of a dead process are no longer re-sent by the retry timer (deleting the process stops the retries)
- feat: `exposes`/`inputs`/`vars` entries whose `type` is omitted are no longer treated as `string` — the exported value is validated and kept by its runtime JSON type (previously a name-only `exposes` entry exported as `string`, so a number/boolean/object output failed completion with a type error); a literal `value` derives its concrete type (e.g. `value: 10` ⇒ `number`); an explicit `type` still validates strictly, and serializing a model no longer writes back an implicit `type: string` (round-trips preserve the untyped state)
- feat: task scope vars are decoupled from the task lifecycle row — a new `vars` collection (`data::TaskVars`, keyed by the same composite `pid + tid` as its task) holds each scope's `data`/`sealed`; a task persist now writes the small lifecycle row and only the vars rows whose scope data actually changed (mutation-tracked dirty flags on the task, flushed over the parent chain by both the sync and the writer persist paths), so pure state transitions no longer rewrite the root row on every task write, and a child output folding up to an intermediate declaring ancestor is persisted at that owner — previously only the root was re-written, leaving intermediate ancestor scopes stale after a crash
- feat: restore loads task vars with one pid-scoped `vars` query and re-attaches them onto the reloaded tasks; durable-row manager views (`task().list`/`task().get`) join the paired vars row for `TaskInfo.data`
- migration: task rows written before this change still parse (their embedded `data`/`sealed` are ignored); scope vars of in-flight processes are not carried over to `vars` rows — a restart of the engine re-persists current scopes on their next flush
- test: cross-backend `store_task_vars_*` cases (create/find, pid-scoped + tid index queries, update without row duplication, remove with index cleanup, upcast) plus cache cases proving lifecycle-only writes never touch vars rows and an ancestor-scope update survives a store round-trip without writing the root
- fix: evict a finished process from the in-memory cache on its terminal proc event — its durable rows stay (the sweeper removes them once every delivery settled), but the freed slot lets the `restore` pass pull other persisted processes (e.g. left over from a crash) back into the cache and auto-start them; previously finished processes squatted in the cache at/over the restore checkpoint and blocked restoring others, and restore passes (triggered by terminal events of different processes, which run concurrently) are now serialized so two passes can never load and auto-start the same persisted process twice
- BREAKING: the in-memory proc cache no longer evicts — moka's LRU cache is replaced by an exact resident map whose capacity (`cache_cap`) is enforced at start time instead of by cache pressure: when the resident set is full a new process is *parked* (its durable row is persisted with state `None`, it is not resident) and the restore pass after the next terminal event starts it, oldest first; previously an over-cap start evicted an arbitrary resident process — usually a long-running or client-waiting one, since recency of cache access is unrelated to liveness — which orphaned the pid (its queue tasks and tick loop keep the old instance alive) and reloading it later created a second `Process` instance racing the first. Running processes are now never evicted, so a live pid always has exactly one in-memory instance
- perf: `restore` refills free slots from parked rows with an indexed `state = none` query ordered by creation (oldest first), replacing the checkpointed rescan of the whole non-terminal set — a slot is topped back to `cap` on every terminal event, and parked demand is bounded, not queued in memory
- fix: a process parked over `cap` is no longer cached by an on-demand `proc()` lookup (a resident parked row would occupy a slot forever, since restore skips resident pids and would never start it)
- feat: engine restart now resumes processes that were in flight when it crashed — boot loads durable `Ready`/`Running`/`Pending` proc rows into the resident set (oldest first, ahead of parked `None` rows, capped) and re-dispatches the tasks the outbox replay left mid-flight; the durable outbox records are replayed first (`recover_actions`), then only tasks with NO pending record are re-run through `exec` — a `Running` leaf cut off mid-run is reset to `Ready` and re-executes (at-least-once: a crash-point task may produce its external effect once more, converging through the state-machine guards `NEXT_COMPLETE`/`IN_CHILDREN`/`schedule_once` dedup), while running parents are driven by their resumed children and `Pending`/`Interrupt` tasks wait for their parent/client as before; previously a mid-run task whose `run` was interrupted (no outbox record yet) stalled forever after a restart — only op-driven propagation was recovered
- fix: boot-resume overflow is no longer stranded — in-flight (`Ready`/`Running`/`Pending`) rows beyond the resident cap are queued at boot (oldest first) and each slot freed by a terminal event now resumes one of them (`Runtime::restore` → `Cache::resume_from_queue`), since `restore` only refills parked (`None`) rows; previously the overflow was only warned about and those processes waited forever


# 0.23.0
- BREAKING: the external store backends moved out of `acts` into the new `acts-store` crate — `SqliteStore`, `PostgresStore`, `RedisStore`, `NatsStore` and `SledStore` are no longer exported from `acts`, and the `acts` `store-*` cargo features (and their sqlx/redis/sled/async-nats dependencies) are removed; add `acts-store` with the matching feature (`cargo add acts-store --features sqlite`) and import the backend from `acts_store` (`use acts_store::SqliteStore;`). The `KvStore` trait, the in-memory `MemoryStore`, the `Store` façade and all row models stay in `acts`; pass the backend to `EngineBuilder::set_store(Arc<dyn KvStore>)` exactly as before
- test: the cross-backend store suite (previously `acts`'s `store::tests`) moved to `acts-store/tests` — one generated suite runs against memory, sqlite and sled in CI (nats/redis/postgres behind their features), and the engine-on-sqlite tests (`engine_set_store_sqlite`, `engine_sqlite_runs_on_current_thread_runtime`) moved there too
- test: the `acts-store` cross-backend suite no longer shares one backend instance across `#[tokio::test]`s — nats/redis/sqlx spawn their connection tasks on the runtime that is current at `open()`, so an instance created by the first test died with that test's runtime and every later test on those backends failed (`broken pipe` / `publish failed: channel closed`); each test now opens its own backend inside its own runtime, and the nats/redis/postgres suites (93 cases each, behind their features) pass against live services
- feat: snapshot-backed sealed data — external systems feed versioned values through `Engine::snapshot()` (`upsert`/`remove`/`read`) over any transport (gRPC/NATS/Kafka adapters); at each task's prepare the engine seals the matching entry from the local cache, so sealing never performs network I/O
- feat: snapshot policies — `SnapshotPolicy::PerProc` (default) seals once per task lineage and descendants inherit the pinned value (a retried/resumed task keeps its first value); `SnapshotPolicy::PerTask` re-reads the latest cache value at every new task
- feat: per-target scope keys — `SnapshotOptions::key_params` derive the snapshot scope from task params (parent-chain traversal), `on_missing` controls missing-param/absent-data behavior
- feat: registration — `EngineBuilder::add_snapshot`/`Engine::add_snapshot` pre-register targets; `upsert` on an unregistered name auto-registers it with default options
- feat: snapshot TTL — `SnapshotOptions::ttl_secs` expires entries not refreshed in time; expired values are dropped on read and by a periodic purge timer, so dead scopes cannot grow the cache unboundedly (pairs with `remove()` tombstones); `SnapshotEntry::ts_ms` renamed `timestamp`
- BREAKING: the callback-based `ConfigResolver` trait and `add_resolver`/`register_resolver` plumbing are removed — sealed data is snapshot-only; migrate by registering a target (`add_snapshot`) and feeding values through `Engine::snapshot().upsert()`
- feat(acts-channel): client helpers `ActsChannel::upsert_snapshot` / `remove_snapshot` to feed and delete snapshot data over gRPC
- feat(acts): the transport-agnostic name→engine action dispatch now lives in the engine as `acts::actions` — `apply(&Engine, name, Vars)` maps channel-message action names (`act:`/`model:`/`pack:`/`proc:`/`task:`/`msg:`/`evt:`/`snap:`) to engine operations and returns the JSON wire value, with `NotFound`/`Invalid`/`Internal` error kinds mapping to transport semantics; adds snapshot actions `snap:upsert` / `snap:remove` / `snap:get` (one scope, `null` when absent) / `snap:ls` (all scopes)
- feat(acts-plugin-grpc): `do_action` refactored onto the shared dispatch with unchanged wire/error semantics — snapshot feed and query are now available over gRPC; added an end-to-end test (client upsert/remove against a live plugin)
- feat(acts-plugin-nats, new): exposes the same message surface as the gRPC plugin over NATS core — actions are request/reply on `<subject>.cmd` through the shared dispatch, engine events are forwarded per configured `[[nats.channels]]` (filters mirror `MessageOptions`), ack/redelivery keeps the gRPC semantics (`msg:ack`); adds README and live tests that skip when no broker is reachable (`ACTS_NATS_URL`)
- feat(acts-plugin-web): `/api/snap/{upsert,remove,get,ls}` endpoints backed by the shared dispatch (missing scope answers `data: null`, same shape as the other transports)
- feat(acts-cli): new `snapshot` subcommand with `upsert` / `get` / `ls` / `remove`; adds README
- feat(acts-server): engine construction extracted into a library (`build_engine` + `ServerPlugins`) shared by the binary and its tests; the NATS plugin is registered when the config has a `[nats]` section; adds README and a NATS wiring test
- ci: the GitHub Actions test job runs a NATS service container and sets `ACTS_NATS_URL`, so the NATS live tests (plugin and acts-server wiring) run against a real broker — they auto-skip when no server is reachable
- fix: update `grpc`, `web` and `nats` config
- fix: add snapshot-backed sealed-data config in acts.toml - default set with `profile` and `secrets` in `acts-server`
- fix: adit error `RUSTSEC-2026-0258`

# 0.23.1
- fix: move `acts-plugin-common` to `acts` as `actions` and remove the origin common reference from all plugins 
- fix: `acts-server` dependencies version error for `acts-package-*` and `acts-plugin-*`

# 0.24.0
- feat: make sure `grpc` plugin is always included in `acts-server`
- feat(acts-server): the server config moves to `~/.acts/acts.toml` (`$ACTS_CONFIG_DIR` overrides) — on first start acts-server creates the directory and writes an embedded default config (the template is compiled into the binary, so a `cargo install acts-server` binary auto-creates it too); a local `./acts.toml` in the working directory deep-merges per key over the `~/.acts` defaults
- feat(acts-server): the `[db]` section is optional and defaults to the sled backend under the config dir; other stores are selected with `type` (`sqlite`/`postgres`/`redis`/`nats`) plus `database_url`, and the `ACTS_DATABASE_URL` env var overrides `database_url`; the server now compiles all `acts-store` backends
- feat(acts): `Config::overlay_file` deep-merges a second acts.toml over a loaded config (nested tables merge per field, scalars/arrays replace) so layered configs can override single options
- build: add a release workflow that builds `acts-server` and `acts-cli` for linux-x86_64 / macos-x86_64 / macos-aarch64 / windows-x86_64 and uploads them to the GitHub release on every `v*` tag
- fix: a vars mutation can no longer be lost to a stale dirty-flag clear — a task scope's vars row is written, then the scope's dirty flag is cleared only when no mutation raced the write (a per-task generation counter is bumped by every vars mutation and compared across the durable write); previously `persist_task_rows` cleared the flag unconditionally after the awaited store write, so a concurrent `set_sign`/`set_data` landing mid-write could be cleared without its data ever being persisted — e.g. the `NEXT_COMPLETE` marker was durably dropped while the outbox record closed `Done`, so crash recovery lost its idempotent-replay guard and re-ran an already-completed `next`
- fix: the postgres store compares keys byte-ordered — the `key` column is created `TEXT COLLATE "C"` (previously the database locale collation, e.g. en_US.UTF-8, weighed the `KEY_SEP`/`KEY_SEP_SUCC` separator bytes equal and broke the half-open `[lower, upper)` index scans at exact bounds: a `Gt` included the bound value and an inclusive `Between` dropped its upper row); pre-existing tables keep their old collation — recreate or `ALTER TABLE ... ALTER COLUMN key TYPE text COLLATE "C"` once

# 0.24.1
- fix: `schedule` triggers arm to their actual next cron fire instead of firing on the first engine tick — a freshly deployed or cron-changed schedule row now stores `next_run = cron.next()` (not `now`), so it fires at its first cron boundary, and re-deploying an unchanged schedule deterministically keeps `last_run`/`next_run` (previously the armed-now row fired immediately and rolled the run state between the deploy and the re-deploy, racing the reconcile)
- ci: the release workflow builds `macos-x86_64` by cross-compiling `x86_64-apple-darwin` on `macos-latest` instead of the retired `macos-13` runners (jobs queued forever waiting for a runner that no longer exists); every target now declares its rust `target` triple and builds with `--target`


# Unreleased
- perf: cache registered package instances per engine registration instead of recreating them for every Func act or custom event trigger; package constructors now run once per successful first use and replaced registrations get a fresh cache slot
- perf: `acts-package-state` uses a long-lived Redis multiplexed async connection and async `GET`/`SET` instead of opening and blocking a synchronous connection on every execution
- perf: `acts-package-http` uses a package-level async `reqwest::Client`, reusing connections and TLS sessions instead of creating a blocking client for every HTTP act
- feat: `acts-package-http` supports an optional `timeout-ms` param for the total request duration; when omitted, requests keep the previous no-timeout behavior
- fix: `acts-package-http` is registered as a `Func` act so the engine executes its package handler instead of leaving the task interrupted
- perf: `acts-package-shell` runs child processes with `tokio::process` and captures stdout/stderr asynchronously without blocking a scheduler worker
- feat: `acts-package-shell` supports an optional `max-output-bytes` param to cap each captured output stream and fail when the limit is exceeded
- fix: `acts-package-nats` no longer creates a private Tokio runtime or calls `block_in_place`; it connects lazily on first async execution and reuses the NATS client, avoiding the current-thread runtime panic and blocking a scheduler worker
- fix: break the `Process → TaskTree → Task → Process` reference cycle — a `Task` now holds its process `Weak`ly, so a finished process evicted from the cache is actually deallocated together with its whole task tree; previously the cycle kept every finished process and all of its tasks (with their scope data) alive forever as unreachable cyclic garbage — `evict` freed the resident-set slot but never the memory, so the `cache_cap` park/refill design still leaked unboundedly
- BREAKING: `Task::proc()` returns `Option<Arc<Process>>` instead of `&Arc<Process>` — `None` only for a task clone that outlived its evicted process (engine-driven paths always see a live process); a late root-task write from the store writer whose process is already gone falls back to the task's own state to stamp the proc row complete, so the sweeper still removes it
- test: `cache_evict_breaks_proc_task_cycle` proves both sides of the contract — the task tree stays queryable while the caller holds the process, and the process is deallocated once the last holder drops
- fix: queue items now carry a process execution lease, so a queued task can still execute when a concurrent terminal event evicts its process before the scheduler reaches the item
- fix: scheduler task execution is panic-isolated and takes the ordinary task-error path, while queue producers fail after the event loop exits instead of silently accumulating unbounded work
- fix: JS value conversion propagates allocation/conversion errors instead of panicking; oversized BigInt results are rejected rather than silently wrapped
- fix: remove unsafe impl Send/Sync
- fix: store `Between`/`In` fallback filters compare integers exactly — `cmp_json_val` now uses the same exact numeric ordering as `order_by` instead of lossy `f64` conversion, so adjacent integers above `2^53` (including the full `u64` range) no longer compare equal in non-indexed scans
- fix: store numeric range fallback filters handle mixed numeric types exactly — `LT`/`LE`/`GT`/`GE` now reuse the exact `order_by` numeric comparator instead of coercing the right side through the left side's integer type or `f64`, so cases such as `3 < 3.5`, `5 < u64::MAX`, and `i64::MAX < u64::MAX` no longer produce false negatives
- fix: atomically claim per-pid cache-miss single-flight, so concurrent callers cannot pass the empty check before another leader inserts; every waiter now receives the same loaded `Arc<Process>` instead of racing duplicate process instances
- fix: admit external process ids through an atomic in-process pid claim, so concurrent starts that all miss the durable row fail as duplicates instead of creating two running or parked instances
- fix: invalid `ChannelOptions` globs no longer panic the public `Channel::channel` / `engine.channel_with_options` path — invalid `type`, `state`, or `uses` patterns fall back to `*` with a warning (so remote channel-query input cannot turn an unclosed bracket into a handler panic), while invalid custom `options` globs remain skipped
- fix: `Config::create` and `EngineBuilder::set_config_source` now return config errors instead of panicking on missing, unreadable, or malformed files; the builder's implicit default config falls back to defaults with a warning
- fix: system environment values that cannot deserialize in `Context::get_env` are treated as absent with a warning instead of panicking the scheduler
- fix: web and gRPC plugins no longer use `unwrap` for bind/serve, socket parsing, remote-peer access, or channel-message serialization; malformed input and runtime transport failures are logged or returned as `Status` errors
- BREAKING: `Engine` now represents a successfully started engine only — configuration lives solely on `EngineBuilder`, `EngineBuilder::start().await` returns `Engine`, and `Engine::new()`/`Engine::start()` are removed; update `Engine::new().start()` to `Engine::builder().start()`
- fix: Index `Eq/Range` scans without value pushdown: all backends scan the entire field region, and SQLite/Postgres also return rows for the entire field region
- perf: cache package definitions and compiled JSON Schema validators in `Runtime`, keyed by act `uses`; repeated `Irq`/`Msg`/`Func` acts no longer perform a package store lookup, reparse the schema text, or recompile the validator on every execution
- perf: cache `ActSchema` validators by serialized schema content, reusing compiled validators for repeated workflow input/output validation
- perf: share process workflow models with `Arc`, avoid deep-cloning the whole workflow while creating or emitting task messages, and reuse one model clone during process startup/build/restoration
- feat: validate `Func` package params against the package JSON Schema before creating and executing the package instance
- fix: invalidate a runtime's cached package definition when the package is published, removed, or re-registered through the engine extender
- perf: pool QuickJS contexts in the expression environment and reuse initialized built-in modules instead of recreating a runtime, context, and all modules for every `${{...}}` expression
- fix: refresh task vars, user vars and sealed data before every pooled expression, and restore isolated global state so one expression cannot leak globals into the next
- perf: linearize `Task::vars` parent-chain merging and reduce redundant `Vars` cloning in JSON conversion and expression filling
- perf: use borrowed scheduler-context access for expression evaluation instead of cloning the scoped context for each nested call
- perf(acts-store): move sled reads, writes, batches and scans onto tokio blocking threads so disk I/O no longer occupies async workers; batches keep one atomic apply-and-flush while single puts/deletes preserve their existing durability window
- BREAKING: `Context::scope` now takes `&Context`; update callers from `Context::scope(context, ...)` to `Context::scope(&context, ...)`
- perf(acts-store): pool SQLite file connections in WAL mode so reads can run concurrently and writes no longer queue behind large scans; batches keep their atomic `BEGIN IMMEDIATE` semantics while in-memory stores retain one shared pooled connection
- fix(plugins): SSE and gRPC transports deregister their channel handler when the client disconnects — the SSE response body and the gRPC `on_message` response stream each carry a drop guard that calls `Channel::close()`, so finished connections no longer leak handlers into the emitter map (previously every dead client kept matching all future messages: unbounded map growth, per-message O(all historical channels) glob matching, a doomed `tokio::spawn` send per dead SSE channel, and three store writes plus one `messages().exists` read per message per dead ack channel); the SSE stream also ends cleanly instead of spinning on its closed queue once the handler is deregistered; regression tests on both transports use ack-delivery message rows as the leak detector (a live channel stores one row per message, a dropped one stores none)
