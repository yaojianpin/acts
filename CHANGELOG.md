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
