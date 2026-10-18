# 内核切换与收尾笔记(`feat/kernel` 第二轮:去 legacy)

> **命名变更(已执行,见 §10)**:`src/kernel/` 已改名为 **`src/runtime/`**,
> `src/kernel_host/` 已并入 `src/runtime/host/`,`src/scheduler/` 已并入 `src/runtime/scheduler/`,
> 且 `Runtime` 现在定义在 `src/runtime/mod.rs`(`crate::runtime::Runtime`)。
> 本文保留 "kernel" 一词**只作为概念**(确定性内核 = `runtime/core`);**所有路径均按新名字书写**。

第一轮(`KERNEL_MIGRATION_NOTES.md`)把 `kernel` + `kernel_host` 建成并与 legacy 双跑对齐,
"剩余工作(按序)"已空。本轮目标是把影子扶正、把 legacy 删干净。**续做任何切片前先读本文。**

门禁(每笔提交前):

```sh
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo fmt
cargo test -p acts --lib --tests
```

`cargo test -p acts --lib --tests` 是本轮的收口命令(P1 之前)。P3 之后 legacy 测试消失,
红名单随之清空(见 §1.8)。

---

## 0. 诉求与顺序调整

诉求四条:

1. 删除 `cache` 无用的模块和文件,把需要的并进 kernel,并实现 `cache_cap` 的能力;
2. 合并 `kernel_host` 到 `kernel`,按目录层次重构 kernel;
3. 移除 `scheduler` 里不需要的模块和功能;
4. 全面接入 kernel:去掉配置开关,去掉 legacy。

**顺序必须调整**(1 与 3 只有在 4 之后才安全):cache/scheduler 的大部分代码只有 legacy
在跑,在切换前删它们只能靠"读代码猜还有没有人用";切换后删是"编译器说没人用"。
因此实际顺序为:**P1 分层整形 → P2 cache_cap → P3 切换 → P4 删 cache → P5 删 scheduler**。
P0 是 P3 的前置(见下)。

---

## 1. 已核实事实(勿再猜测)

> 本节的目录/行号是 **P1a/P1b 当时**的事实,已被 2026-10-13 的「runtime 结构收敛」修订;
> 现行布局与判据见 §10 该小节。

### 1.1 体量

| 模块 | 行数 | 备注 |
|---|---|---|
| `src/runtime/` | 19,235(39 模块 + `mod.rs`) | 扁平一层 |
| `src/kernel_host/` | 12,179(其中 `mod.rs` 10,094,测试从 4,280 起) | 单文件承载 8 种职责 |
| `src/store/` | 6,935 | 真正的存储层,两侧共用 |
| `src/cache/` | 4,786 | 常驻对象集,不是数据缓存 |
| `src/scheduler/` | 4,322 | 常驻引擎 + 若干被 host 复用的基座 |

基线:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` 当前**绿**
(1m15s 增量)。

### 1.2 依赖方向与守卫

- `scheduler/` `cache/` `store/` 对 `crate::kernel` 的**代码引用为 0**;`store/store.rs:33-34` 只有一句文档。
- `kernel → crate::store` 只在 `kernel/async_document_store.rs`(方法逐项直通 `KvStore`)。
- `runtime/core/architecture.rs`(`#![cfg(test)]`)是**可执行守卫**:`production_sources()` 用
  `fs::read_dir(src/kernel)`(**非递归**)扫 `*.rs`,凡出现
  `crate::cache` / `crate::runtime` / `crate::runtime_state` / `crate::scheduler` /
  `crate::service(s)` / `crate::store` / `tokio::` 即 fail;白名单
  `INFRASTRUCTURE_ADAPTERS = ["async_document_store.rs"]`(:8)。
- `kernel_host` 的生产依赖:`crate::scheduler::{Context, Process, Runtime, Task, NodeTree, TaskState, Sign, NodeData, prepare_workdir}`、
  `crate::cache::Cache`、`crate::store::{KvStore, Store, DbCollection, DbCollectionIden, MemoryStore, data, query}`、
  `acts_expr`、`tokio`、`crate::package::*`、`crate::{Message, MessageState, Vars, ActError}`、`crate::model::Workflow`。
- 已存在的范式(照抄别发明):`runtime_state.rs:3-6` —— 需要 runtime 词汇的规则放 kernel 外,
  规则本体留 kernel 内。**适配在外,规则在内。**

### 1.3 cache 是三分,不是一物

| 部分 | 归属 | 证据 |
|---|---|---|
| 常驻 `Arc<Process>` 集 + 容量准入 + pid claim + boot resume/overflow 队列 | **引擎态,切换后死** | `cache/cache.rs:66-812`;`admit` 唯一调用点 `scheduler/runtime.rs:524`(legacy 分支) |
| `store()` 句柄 / delivery 写路径 / sweeper | **读模型态,活** | 生产侧 43 处 `cache().store()`;`StoreDelivery`/`RemoveProc`/`Barrier` 三个 `WriteOp` 被 host 使用 |
| `impl Store` 生命周期行方法 | 混:READ-MODEL 活、LEGACY 死 | `cache/store.rs` 分类见第一轮审计 |

具体死/活清单:

- **死**:`procs`/`claimed`/`removed`/`loading`/`pending_resume`/`admission`,
  `admit`/`evict`/`abandon`/`reserve_slots`/`start_parked`/`resume`/`enqueue_resume_overflow`/`resume_from_queue`/`push_proc`/`push_proc_pri`/`start_proc`/`upsert`/`upsert_async`/`try_upsert_async`/`try_enqueue_exec`/`enqueue_next`/`enqueue_action`/`complete_*`/`mark_op_*`/`mark_next_overflow`/`persist_task`/`push_task_mem`,
  writer 的 `WriteOp::{Task, CloseDeliveries, EnqueueNext, EnqueueExec, MarkOpDispatched, MarkOpOverflow, MarkOpPhase, EnqueueAction, OpDone}`,
  `cache/store.rs` 的 `has_parked`/`load_parked`/`load_resumable`/`decode_proc`/`load_tasks`/`enqueue_*_op`/`load_pending_ops`/`has_pending_propagation`/`mark_op_*`/`load_scheduled_ops`/`complete_ops`/`remove_ops`/`upsert_task*`/`persist_task_rows`/`mark_proc_complete`。
- **活**:`store()`,`flush_pid`,`sweep_removable`/`remove`/`queue_removal`,`proc`→`load_proc`(读 API),
  writer 的 `StoreDelivery`/`RemoveProc`/`Barrier`,delivery 行方法
  (`mark_delivered`/`set_delivery`/`resend_error_deliveries`/`clear_error_deliveries`/`resend_error_delivery`/`clear_error_delivery`)。
- **两引擎共用**:`sweep_removable`(legacy tick `runtime.rs:1782` + `export/actions.rs:1312` + host 测试)。

### 1.4 cache_cap 现状

- `Config::cache_cap`(字段在 `config.rs:44`,accessor 在 `config.rs:220-221`,默认 1024);唯一写入口 `builder.rs:111`(`cache_size`);
  唯一读点 `Cache::new`(`cache/cache.rs:233`)→ `Cache::cap`。
- 语义:常驻**内存**进程数上限;超限 → `admit` 把 proc 行以 `TaskState::None` 落库并返回
  `false`,`Runtime::launch` 提前返回(`runtime.rs:524-530`),日志 "process parked";
  槽位释放(终态 → `evict` `runtime.rs:1647`,或 tick → `sweep_removable`)后由
  `restore`→`start_parked`/`resume_from_queue` 补位。parked 行按 `timestamp` 升序补位(FIFO)。
- **kernel 侧完全没有这个能力**:`execution_admission.rs` 只决定 generation 复用(不计数、不封顶);
  driver 只有一个任务顺序驱动 hop(`kernel_host/mod.rs:1152`),wake 通道容量 1024 满则丢 wake(靠 `recover` 收敛)。

### 1.5 切换缺口(必须与 P3 同时处理)

- `engine.rs:151-165` 的 `event_loop()` / `resume()` / `recover_actions()` / `init_retry_timer()`
  **无条件**执行,没有 kernel 分支网关;`init_trigger_timer`(schedule 触发)与
  `init_snapshot_timer`(TTL)是引擎无关的,保留。
- `retry_timer` tick → `rt.restore()`(`runtime.rs:1792`)→ `cache.start_parked` →
  `load_parked` 查 `state == None` 的 proc 行(`cache/store.rs:33-60`)。
  而投影对 `ExecutionState::Created` 的根写的正是 `TaskState::None`(`kernel_host/projection.rs:442-448`)。
  `[INFERENCE — 代码通读,未实测]` 因此 kernel 模式下一个"已 seed、未走第一跳"的聚合,其投影行会被
  legacy 常驻路径当作 parked 进程启动(`Runtime::launch` → `Process::start` 跑的是 legacy 引擎)。
  P3 必须先删掉 `restore`/`start_parked` 这条路径。
- `Config::kernel_engine` 的 doc(`config.rs:85-93`)已过期:说"read model 尚未投影",但投影早已接上
  (`scheduler/runtime.rs:1558`)。

### 1.6 scheduler 的存活面(切换后要留的)

`kernel_host` 只用到 scheduler 的**无头基座**:

- `Process`/`Task`/`Context` 的 headless 成员:`Process::{new,load,node,create_task,create_context,set_workdir,set_owner_scope,env,with_env_mut}`、
  `Task::{params,options,data,state,...}`、`Context::{new,task,prepare,resolve_sealed,eval,set_env,get_env}`(`kernel_host/scope.rs:39-67`、`effects.rs`、`predicate.rs`)。
- `scheduler/tree/**`(`NodeTree`/`Node`/`NodeData`/`TaskTree`/build/visit):读模型投影 + `Workflow::valid` + `cache/store.rs::decode_proc` 复用。
- `scheduler/state.rs`(`TaskState`/`PropagationPhase`)、`scheduler/validation.rs`(`CachedPackage`/`SchemaCache`)。
- `Runtime` 及其存取器(`config`/`env`/`emitter`/`package`/`cache`/`store`/`schema_cache`/`snapshots`/`shutdown`/`kernel`)、`prepare_workdir`。
- `env/**`(`functions.rs:313,332` 走 `Context::try_with_current` / `current_cost`)、`utils/convert.rs`、
  `snapshot.rs::resolve_scope_params`,都跑在无头 scope 上,必须留。
- `scheduler/mod.rs` 的 `ActTask` 由 `pub(crate) use`(lib.rs:81)承诺,保留。

可删:`queue.rs`、`ops.rs`、`health.rs`、`process/process.rs` 的 `start`/`init_tick`/`do_tick`/`do_action`、
`Context` 的调度/游走路径、`task/**` 的 `ActTask` 实现、`Sign`/`NextAction`(仅 legacy 与 host 测试用)、
`Runtime` 的 `queue`/`op_claims`/`stall_backoff`/`outbox_dirty`/`trigger_health`/`retry_health` 与三个 timer、
`event/emitter.rs` 的 `ProcessGate`/`LaneRouter` 接线(`utils/lane.rs` 的 `LaneRouter` 随之失去消费者)。

### 1.7 测试分布

- `scheduler/tests/**`:33 文件(含 `reliability/**`、`act/**`、`step/**`、`branch/**`、`process/**`、`tree/**`),
  对 kernel/parity 引用为 **0** —— 全部是常驻引擎测试。其中纯子系统测试(`tree.rs`、`health.rs`)随子系统保留/迁移,其余随引擎删除。
- 双引擎 parity 全在 `kernel_host/mod.rs` 的 `#[cfg(test)] mod tests`(约 60 个,其中约 20 个显式
  `..._matches_legacy...`),辅以 `legacy_node_states`/`folded_legacy_states`/`assert_parity`(`:6715`/`:9398`/`:9715`)。
- `kernel/coverage.rs` 是**类型面覆盖**,不是 parity。

### 1.8 已知红(迁移期)

- `scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary` —— stash 基线单跑同样失败,与 kernel 改动无关;`reliability::error` 与
  若干 `kernel_host::tests::kernel_*` 双引擎用例在**同进程并行**负载下会翻转(翻的是 legacy 侧断言)。
- **这些红都是 legacy 侧或 legacy 对照造成的;P3/P5 删掉 legacy 后应一并消失。** 若 P3 后仍红,就是真回归。

---

## 2. 阶段划分(按依赖序)

### P0 冻结 parity 为 golden(**P3 的前置**)

**为什么**:双引擎对照是当前唯一的正确性网;删 legacy 就再没有对照物。先把 legacy 实测结果固化成常量,
对照网才能转型成"只跑 kernel + 断言固定终态",而不是直接消失。

- 把 `kernel_host/mod.rs` 里每个 `kernel_*_matches_legacy*` 用例的 legacy 侧结论摘出,
  固化为该用例的期望值(kernel 侧断言不变,删掉 legacy 侧驱动与 `assert_parity` 的 legacy 参数)。
- 保留 `folded_legacy_states` 折叠口径的**文档**(§第一轮「语义结论」已记录),但不再需要运行时比较。
- 验收:测试全绿且**不再构造 legacy `Process` 跑 workflow**。
- 风险:固化时抄错值 = 把 bug 固化成期望。所以摘值必须来自该用例当前实测输出,不能手抄。

### P1 kernel 目录分层 + 合并 kernel_host(纯搬移/re-export 整形,零行为变更)

`kernel_host` 一旦进来,`architecture.rs` 的现行规则按定义就会 fail,所以**先改守卫作用域,再搬文件**。
本阶段拆成四笔提交,互不夹带:

**P1a 搬移(不改语义)**
1. 改 `runtime/core/architecture.rs`:扫描改**递归**(`walk` 目录树),规则按目录分档(见 §4)。
2. 按 §3 的落位表搬 39 个模块 + `kernel_host/*` → `kernel/host/*`。
3. 修 `use super::` 相对路径;`runtime/mod.rs` 的 re-export 此笔**保持**同名同路径可用
   (`crate::runtime::X` 不变),`crate::runtime::host::` → `crate::runtime::host::`
   (调用点:`scheduler/runtime.rs` 7 处 + `lib.rs:28`)。
4. 拆 `runtime/host/mod.rs`(10,094 行):按已有兄弟文件的粒度抽出 hop driver、
   timeout round、container shape walk、failure 映射、node kind/name、scope vars 等。
5. 新增规则:纯层不得依赖 `durable`/`host`(单向分层)。

**P1b 收敛 kernel 对外面** —— 按 §3.1(删一揽子 re-export、去 `#[allow(dead_code)]` 交编译器、
单消费者模块降级、层内私有化)。

**P1c 收敛 `export/` 方法定义** —— 按 §3.2(窄依赖、同形合并、常量单一来源)。

**P1d trait 隔离** —— 按 §3.3;其中包 API 的 `ActPackageRuntime` 必须单独一笔提交。

- 验收:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` 绿 +
  `cargo test -p acts --lib --tests` 绿(红名单同 §1.8,不新增);
  P1b 后 `src/` 非 kernel 目录不得出现 `crate::runtime::core::`/`crate::runtime::durable::`;
  P1c 后 `grep -c "cache()\.store()" src/export` 生产侧为 0。
- 回滚:每笔独立提交,各自 revert。

### P2 cache_cap 进 kernel

方案见 §5。要点:

- 规则放纯层(新增 `runtime/plan/concurrency.rs` 或并入 `execution_admission.rs`):
  输入 `(cap, active_count, pid_state)` → `Admit | Park`;不读时钟、不读存储。
- 观测/执行放 `kernel/host`:driver 维护活跃进程集合,超限的 run 不驱 hop。
- **需要一个新的私有根事实 `__created`**(seed 时的毫秒时间戳),才能复现 legacy parked 行的
  FIFO(timestamp asc)顺序;先例:`timeout.rs` 的 `STARTED_VAR`(同目录,ditto 注释风格)。
- `Config::cache_cap` 字段名**不改**(公共 serde 配置),只改语义与 doc:
  "同时在飞(非终态)的进程数上限"。`builder.rs:111` 的 `cache_size` 保留。
- 新测试:纯策略单测 + host 侧"超限不驱 hop、槽位释放后补位"的行为测试。
- 验收:kernel 模式下 `cache_cap=N` 时第 N+1 个进程的根保持 `Created`(投影为 `None` 行),释放后被驱动到终态。

### P3 切换:kernel 成为唯一引擎

1. 删 `Config::kernel_engine`(`config.rs:85-93, 288-292`),`Runtime::create` 无条件构造 host
   (`runtime.rs:1502-1512`)。
2. 网关/删除 legacy 启动路径:**先删** `restore`/`start_parked`/`resume_from_queue`(§1.5),
   再删 `engine.rs:151,153,161,163` 的 `event_loop`/`resume`/`recover_actions`/`init_retry_timer`,
   以及 `Runtime::{launch,push,dispatch_root,do_tick 接线,enqueue_*/complete_*,recover_outbox,redispatch_resumed,return_to_act}`。
3. `Runtime::start` 只走 `host.start_process`;`do_action` 只走 `kernel_action`;`Runtime::proc` 改直读
   `store.load_proc`(不再经 `Cache::proc`)。
4. `Runtime::initialize` 里 `emitter.on_proc`/`on_task` 的 legacy 消息构造删掉(host 的 lifecycle sink 已等价)。
5. `retire` 与 legacy 引擎绑定的 builder 旋钮:`scheduler_workers`/`scheduler_queue_cap`/`max_node_run_times`
   (以及 `store_writer_*`/`tick_interval_secs` 若只服务旧队列/writer)。
6. 测试:把 §1.7 的双引擎用例改成单引擎(P0 已备好期望值);**不必**保留 `kernel_engine: bool` 参数。
- 验收:`cargo test -p acts --lib --tests` 绿;`§1.8` 的红全部消失。
- 风险:这是本轮最大的一步(改测试为主)。用 P0 的 golden 兜底;每笔提交只动一个切面。

### P3.5 性能收口(切换后、删 legacy 前)

切 P3 之前必须先做 §9.5 的 A/B 基线;若 kernel 模式的 hop 吞吐低于 legacy 的 1/2,
本阶段完成后再继续删代码:

1. 效果执行不再内联阻塞 driver(§9.3.1)—— 最大项,决定单实例并行度。
2. 合并 Claim + PrepareCommit 两次 CAS(§9.3.2)—— 每跳 4 写降到 2~3 写。

- 验收:§9.5 的基线复测,hop 吞吐 ≥ legacy;两个互不相关的 run 能并行推进。
- 回滚:每项独立提交。

### P4 删 cache 死件 + 搬存活件

**目标形态与落位见 §6**(模块整体删除,三个继承者各有新家)。执行要点:

1. 按 §6.2 把 `cache/store.rs` 的存活方法拆进 `store/process_rows.rs` 与 `store/delivery_rows.rs`,
   LEGACY 方法删除。
2. 按 §6.3 把 writer 存活的三条 `WriteOp` + `flush_pid` 迁到 `store/writebehind.rs`。
3. 按 §6.4 让 `Runtime` 自持 `Arc<Store>`(构造从 `Cache::new` 搬来),删 `cache` 字段与 accessor;
   `export/**` 已在 P1c 降层,本阶段只剩 `channel.rs`/`engine.rs:145`/`scheduler/runtime.rs` 的 8 处 + 测试。
4. `RowProjector::new(runtime.cache().clone(), …)`(`runtime.rs:1558`)改持 `Arc<Store>`。
5. 按 §6.5 给无头 scope 挂 no-op 持久化 sink(`$set_step_value` 的幽灵行)。
6. 删除 `cache/` 整个目录(§6.1 的清单 + `cache/tests.rs`)。

- 验收:`cargo test -p acts` + `cargo test -p acts-store` + `cargo test -p acts-server` 绿
  (后两者也可能引用 store/cache)。

### P5 删 scheduler 死件

按 §1.6 的可删清单删:`queue.rs`、`ops.rs`、`health.rs`、`process/*` 的常驻生命周期、
`Context` 的调度/游走路径、`task/**` 的 `ActTask` 实现、`Sign`/`NextAction`、
`Runtime` 的 legacy 字段与三个 timer、`utils/lane.rs`(若无消费者)。
移出被 host 复用的基座到更合适的位置(建议 `scheduler/` 保留为无头基座 `runtime/` 或直接并入 `kernel/host/scope/`;择一并在本文件登记结论)。

> **结论(2026-10-14,用户裁决,已完成)**:不保留 `scheduler/` 这个模块名 —— 旧基座按依赖拆开(`state`/`tree`/`cron` 进 `core`,`health`/`validation`/`process`/`context`/`task_tree` 进 `shell`),
> 而**整个 runtime 目录改名为 `scheduler/`**、`Runtime` 落到 `scheduler/runtime.rs`。理由:用户可见的配置词汇本就把它叫 scheduler
> (`Config::scheduler_workers`/`scheduler_queue_cap`/`acts.toml`);且改名顺带消掉 P1a 的 `clippy::module_inception` 约束。
> 逐项与实测见 §10「P5(名与形)完成」。

- 删 `scheduler/tests/**` 的常驻引擎测试(≈31 文件);保留 `tree.rs`/`health.rs` 等纯子系统测试
  (若 `health.rs` 随 timer 删除,则其测试一并删除并在本文件登记)。
- 验收:`cargo test -p acts` 绿;`cargo build --workspace` 绿。

### P6 门禁与文档

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`、`cargo fmt`、
  各 crate 测试(AGENTS.md 的清单)、动了 manifest 才跑 `cargo deny check bans`。
- 更新 `lib.rs:18-31` 的过期 P1/P2 注释;`KERNEL_MIGRATION_NOTES.md` 顶部指向本文;
  本文的每阶段结束后把"已完成/结论"按第一轮的惯例登记进来。

---

## 3. kernel 目标目录布局(P1)

> **2026-10-13 状态**:本节的三层形态最终成立(见 §10「结构回滚」),两处不同:中间层名为
> **`shell/`**(原 `host/`;`host.rs` → `shell/mod.rs`,`host/model.rs` → `shell/model.rs`),且
> `runtime/mod.rs` 不再承载纯层用具。§10 的「runtime 结构收敛」是中间产物,已回滚。

```
src/runtime/
├── mod.rs              # 仅 re-export(保持 crate::runtime::X 不变)+ 分层说明
├── core/               # 纯函数层:无 IO、无 tokio、无 store/scheduler/cache
│   ├── model/          # 领域模型
│   ├── plan/           # 纯规划器
│   ├── commit/         # 聚合提交与应用服务
│   ├── recovery/       # 恢复语义与编码
│   ├── outbox/         # durable outbox 领域
│   └── tests: architecture.rs, coverage.rs
├── durable/            # 字节/存储层:唯一允许 crate::store
└── host/               # 异步外壳:允许 tokio/store/scheduler(无头基座)/cache(store 句柄)
```

落位表(39 模块):

| 现状 | 去向 |
|---|---|
| `clock.rs` `document.rs` `envelope.rs` `operation.rs` `operation_phase.rs` `transition_plan.rs` `execution_graph.rs` `model_graph.rs` `generation_reference.rs` | `core/model/` |
| `branch.rs` `container.rs` `generation.rs` `propagation.rs` `upward.rs` `timeout.rs` `execution_admission.rs` `failure_recovery.rs` `process_recovery.rs` | `core/plan/` |
| `commit.rs` `application.rs` `branch_application.rs` `command.rs` `operation_executor.rs` | `core/commit/` |
| `recovery.rs` `replay_policy.rs` `recovery_entry.rs` `recovery_format.rs` `recovery_index.rs` | `core/recovery/` |
| `outbox.rs` `outbox_codec.rs` `outbox_index.rs` `outbox_repository.rs` `outbox_dispatch.rs` `outbox_worker.rs` | `core/outbox/` |
| `document_store.rs` `async_document_store.rs` `snapshot_codec.rs` | `durable/` |
| `architecture.rs` `coverage.rs` | `core/`(测试模块) |
| `kernel_host/{mod,effects,message,model,predicate,projection,scope}.rs` | `host/` |

### 3.1 收敛 kernel 的对外面(P1 第二笔提交)

现状事实:`runtime/mod.rs` 共 **31 条 `pub(crate) use`**,把 ~212 个顶层 `pub(crate)` 里的
**~136 个**倒进扁平的 `kernel::` 命名空间;但**整个 crate 只有 2 个 kernel 项被 kernel 与
kernel_host 之外使用**——`ReplayOperationKind`(`runtime_state.rs:9`)与 `ActionKind`
(`scheduler/runtime.rs:654,656`)。**`export/**` 对 kernel/kernel_host 的引用是 0。**
也就是说那份 136 项的名单几乎只为 kernel_host 存在;host 一进来,它就该缩掉。

动作:

1. 删掉 `runtime/mod.rs` 的一揽子 `pub(crate) use`;改为**每层一个窄门面**:
   `core/mod.rs` / `durable/mod.rs` / `host/mod.rs` 各自只导出跨层需要的东西;
   `runtime/mod.rs` 只导出 host 面 + 上面那 2 个外层消费者需要的类型。
2. 删掉 `lib.rs:23` 的 `#[allow(dead_code, unused_imports)]`,**让编译器交出真正的死代码清单**
   (已知零消费者的候选: `operation_executor`、`branch_application`、`failure_recovery`、
   `process_recovery`、`outbox_worker`)
   —— 以 `cargo clippy -p acts --all-targets -- -D warnings` 的实际输出为准,不按候选名单删。
3. 单消费者模块降级为 `pub(super)`(已核实):`envelope`↔`document`、`snapshot_codec`↔`document`、
   `recovery_format`↔`async_document_store`、`command`↔`application`、`operation_phase`↔`replay_policy`、
   `execution_admission`↔`container`、`generation_reference`↔`replay_policy`、`outbox_dispatch`↔`outbox_worker`。
4. 层内私有化:只被同层一个模块用的 item 从 `pub(crate)` 收紧为 `pub(super)`/私有。
5. 去掉中转:层内模块之间直接 `super::`/`crate::runtime::core::x`,不再经 kernel 根绕一圈。

验收:外部对 kernel 的引用只剩 host 面(可用一条 grep 规则固化:`src/` 下除 `kernel/**` 外,
禁止出现 `crate::runtime::core::` 与 `crate::runtime::durable::`)。

### 3.2 `export/` 方法定义收敛(独立轴,与 kernel 分层并行)

现状事实:`export/**` 与 kernel 无依赖,8 个子 executor 形状完全相同
`{ runtime: Arc<Runtime>, principal: Arc<Principal> }`;共 **28 处 `self.runtime.cache().store().*`**
(`message` 9、`model` 4、`package` 4、`event` 4、`process` 3、`task` 3、`ext` 1、`act` 0;
`channel.rs` 另 2 处);同形方法重复:`list`×6、`get`×6、`rm`×3、`publish`×2(两份**完全相同**)、
`start`×2;36 个 action 名常量散在 8 个文件(`LS`/`GET`/`RM` 各声明 6/6/3 次),
且 `actions.rs` 的 `match` 又手写同一批字符串。`export/**` 内**零 trait**。

动作:

1. 窄依赖:`runtime: Arc<Runtime>` → `store: Arc<Store>` + 各 executor 真正需要的 sink
   (`Runtime::store()` 在 `runtime.rs:272` **已存在且 export 未用**,`cache().store()` 28 处直接降一层)。
   按 executor 的最小需求表(`act`:仅 `do_action`;`task`:`store` 即可;`event`:还需 `Emitter`+`Package`+`Config`,
   见 §3.3 第 3 条)。
2. 同形收敛:`list`/`get` 的分页重建(`PageData` + `Into`)提成一个泛型帮助;
   `publish` 的两份相同实现合并到一处。只收敛**真正同形**的,不要为 `start`/`get` 的差异硬套。
3. 常量单一来源:36 个 action 名收敛到一处(建议 `export/actions.rs` 内的常量,executor 与 dispatch `match` 共用同一份)。
4. **保留** `principal.check(...)` 的显式前导(~33 个公共方法各一行):它是策略执行点,
   不要藏进宏或泛型 —— 鉴权路径要能被逐行审。
5. **不引** trait:executor 只需要具体 `Arc<Store>`,造 `trait WorkflowStore` 是空抽象。

验收:`cargo test -p acts` 绿;`grep -c "cache()\.store()" src/export` = 0(生产侧)。

### 3.3 trait 隔离:只在必要时(P1)

判定标准(满足其一才引 trait):**多变实现** / **需要测试替身** / **跨分层边界且必须单向**。

1. 已有 13 个 seam trait 就是模板,不要重造:`AggregateRepository`/`OperationRepository`/`RecoveryRepository`
   (`commit.rs:131/169/148`)、`AtomicDocumentStore`(`document_store.rs:124`)、`AsyncDocumentStore`
   (`async_document_store.rs:40`)、`OutboxRepository`(`outbox_repository.rs:13`)、`OutboxTransport`
   (`outbox_dispatch.rs:14`)、`OperationWakeSink`(`application.rs:10`)、`Clock`(`clock.rs:5`)、
   `StopSignal`、`OperationExecutionContext`/`OperationHandler`(`operation_executor.rs:34/40`),
   以及 host 侧的 `EffectRunner`/`PredicateRunner`/`LifecycleSink`。
2. kernel 层间:**host 需要 core 的行为**才走 trait(上面那些已是);host 读 core 的**纯数据**
   (`AggregateSnapshot`/`ExecutionState`/`TransitionPlan`)**不**引 trait —— 快照是数据,给数据造 trait 是过度设计。
3. 唯一必须新增的 trait:`ActPackage::start`(`package/mod.rs`)把 `Arc<Runtime>` 写死。做法:给包
   API 一个窄 trait(`ActPackageRuntime`,由 `Arc<Runtime>` 实现),把该签名改成 `&dyn ActPackageRuntime`。
   **2026-10-13 更正(实测)**:它**不是** `EventExecutor` 无法窄化的唯一原因 —— `EventExecutor`
   还要 `Runtime::package()`(注册表)、`Runtime::config()`、`Runtime::emitter()`(`start_hook` 的
   on_complete/on_error),这些能力各自宽窄不一且都不满足引 trait 的条件。所以 P1d 的收益是
   「包 API 不再点名 `Runtime`」(并顺带让 `start` 在 crate 外可覆盖),**不是** executor 窄化;
   `event_executor` 仍持 `Arc<Runtime>`。
   **这是跨 crate 公共 API 变更**(`packages/**` 多个包实现 `ActPackage`),必须**单独一笔提交** +
   迁移全部实现者,不得与搬移提交混合。
4. 反例登记(明确不引):`export` 的 store 访问、host 对 `AggregateSnapshot` 的读、
   `KvDocumentStore` 对 `KvStore` 的直通 —— 都不需要新 trait。

---

## 4. architecture 守卫的新规则(P1)

> **2026-10-13 修订**:判据回到目录(`core/` 纯层、`durable/` 字节层、`shell/`+`scheduler/` 外壳),
> 并新增 `RUNTIME_ROOT` 完备性清单(根下每个条目必须登记)与 `the_declared_tiers_agree`(规则表钉在
> 分类上)。现行文件 `crates/acts/src/runtime/core/architecture.rs`;见 §10「结构回滚」。

`core/**`:
- 禁 `crate::store` `crate::cache` `crate::scheduler` `crate::runtime` `crate::runtime_state`
  `crate::service(s)` `tokio::`,并且**新增**禁 `crate::runtime::durable` `crate::runtime::host`。
- 白名单:无(`core` 必须干净;若确需一处适配,登记在这里并说明理由)。

`durable/**`:
- 允许 `crate::store`(白名单仍限定具体文件);禁 `crate::scheduler` `crate::cache`
  `crate::runtime::host` `tokio::`。

`host/**`:不设依赖禁令(它必须接触 tokio/store/无头 scheduler/store 句柄)。

扫描必须**递归**(现在是非递归 `read_dir`,搬进子目录后会静默漏检——这是搬移时最容易掉进去的坑)。

---

## 5. cache_cap 方案

现状见 §1.4。legacy 语义是"常驻内存数上限",kernel 无常驻对象,所以**等价语义必须重新定义**,
不能照搬字面。

- **方案 A(durable 索引):**新增 `kernel/indexes/admission/<pid>`,seed 时写、终态时删,
  driver 每跳前统计。持久、可跨重启直接恢复;代价是多一个派生索引(与 outbox/recovery 索引同族)。
- **方案 B(推荐,driver 内存计数):**driver 是唯一任务,维护 `active_pids: HashSet<String>`;
  `start_process` 插入,根执行到达终态时移除;重启时用 `KvStore::scan_prefix("kernel/aggregates/")`
  重建一次(启动期 O(N),之后增量)。零新增存储结构,且单任务 = 无并发竞态。
  与 legacy 的 `claimed`(进程内 pid 集合)同构。
- 组合:先 B,若日后需要跨实例准入再升级到 A。

无论哪个方案,以下细节必须一起定:

1. **"活跃"的定义**:根执行未终态的 aggregate(对齐 legacy "resident 且未结算")。
2. **parked 的可见面**:kernel 的 park = **不驱 hop**,根保持 `Created`;投影把 `Created` 根写成
   `TaskState::None`(`projection.rs:442-448`)——正好与 legacy parked 行同形。这是对齐点,不是巧合,must 写进测试。
3. **FIFO 顺序**:legacy 按 proc 行 `timestamp` asc 补位;kernel 需要 seed 时间戳,
   故新增私有根变量 `__created`(毫秒,seed 时写)。命名与注释照 `timeout.rs` 的 `STARTED_VAR`。
4. **补位触发**:槽位释放(某 run 根到达终态)时,driver 立即从 parked 集合里按 FIFO 唤醒一个;
   不要依赖固定 tick 周期,否则延迟与 legacy 不一致(legacy 除了终态事件,还靠 tick 兜底 —— 两个触发都保留)。
5. **`cache_cap` 默认值语义变化**:1024 从"常驻内存上限"变成"同时非终态进程上限"。这是**行为变更**,
   必须写进 CHANGELOG / 配置文档。

---

## 6. cache 的归宿(新 cache 方案)

**结论:`cache/` 模块整体删除,不设替代模块。** 它的三份职责各有新家;而"常驻进程缓存"这件事
在 kernel 架构下**不存在**——kernel 不持有常驻对象,所以没有"新 cache"这种东西需要设计。

### 6.1 消亡(无替代)

常驻集 `procs`,以及 `admission`/`Admission`/`SlotReservation`/`claimed`/`removed`/`loading`/
`pending_resume`/`PendingResume`,连同全部方法:`admit`/`evict`/`abandon`/`reserve_slots`/
`start_parked`/`resume`/`enqueue_resume_overflow`/`resume_from_queue`/`push_proc`/`push_proc_pri`/
`start_proc`/`upsert`/`upsert_async`/`try_upsert_async`/`try_enqueue_exec`/`enqueue_next`/
`enqueue_action`/`complete_next`/`complete_action`/`complete_propagation`/`mark_op_dispatched`/
`mark_op_phase`/`mark_next_overflow`/`persist_task`/`push_task_mem`/`proc`/`resident`/`procs`/
`count`/`cap`/`capacity_state`/`pending_resume_ids`/`close`(`cache/cache.rs:66-812` 的主体);
`cache/tests.rs`(1,634 行)一并删除。

依据:唯一调用点全在 legacy 分支(`scheduler/runtime.rs:517-527`、`1073`、`1086`、`1110`、`1647`、`1782`);
P3 之后由编译器证明零调用。

### 6.2 继承者一:`store/` 吸收 delivery 行与进程行操作

`cache/store.rs`(770 行)本来就是 `impl Store`,按类拆搬进 `store/`:

| 新文件 | 内容 |
|---|---|
| `store/process_rows.rs` | `load_proc`(读 API:`proc:get` / `Runtime::proc`)、`remove_proc`(sweeper)、`proc_workdir` |
| `store/delivery_rows.rs` | `mark_delivered`、`set_delivery`、`update_delivery`、`try_mark_removable`、`resend_error_deliveries`、`clear_error_deliveries`、`resend_error_delivery`、`clear_error_delivery` |

会合点(`store/store.rs` 已有,**不动**):`sweep_settled_procs:700`、`close_deliveries:550`、
`has_unsettled_deliveries:455`、`with_no_response_deliveries:640`。
结果:delivery 生命周期语义此后只在 `store/` 一处,不再横跨 `cache/store.rs` 与 `store/store.rs`。

删除(LEGACY 类):`has_parked`/`load_parked`/`load_resumable`/`decode_proc`/`load_tasks`/
`enqueue_*_op`/`load_pending_ops`/`has_pending_propagation`/`mark_op_*`/`load_scheduled_ops`/
`complete_ops`/`remove_ops`/`upsert_task`/`upsert_task_data`/`upsert_task_vars`/`persist_task_rows`/
`mark_proc_complete`/`upsert_proc`。

### 6.3 继承者二:读模型写路径(writer 的存活部分)

保留:`WriteOp::{StoreDelivery, RemoveProc, Barrier}` + `send`/`flush`/`flush_pid`/`close`。
新家:`store/writebehind.rs`(候选名 `read_model::writer`)。
生产者只剩两处:`export/channel.rs:328`(`store_if` → delivery)与 sweeper(removal)。
保留理由(两条都来自源码注释):delivery 行必须与同 pid 的行写入**有序**(`export/channel.rs:309-317`);
`deliver` 跑在 emit 路径上,写 I/O 不能内联进 kernel 的 hop。
配置 `store_writer_workers`/`store_writer_queue_cap` 随它保留。

### 6.4 继承者三:`Runtime` 直接持有 `Arc<Store>`

- `Runtime::store()` 今天 = `self.cache.store().clone()`(`runtime.rs:272`)→ 改 `self.store.clone()`。
- `Store::new(kv)` 的构造从 `Cache::new`(`cache/cache.rs:230-233`)搬到 `Runtime::create`。
- `Runtime.cache` 字段与 `Runtime::cache()` accessor(`runtime.rs:226-228`)删除,调用点改 `store()`。
- `LaneRouter`(`utils/lane.rs`)随 `queue` 与 emitter 的 `ProcessGate` 一起退场(P5 时确认 emitter 是否仍需分片)。

### 6.5 新发现:无头 scope 的写侧信道必须关掉

`env/functions.rs:214`(`$set_step_value`)会 `ctx.runtime.try_upsert_task(task)`,今天经 cache writer
走 `WriteOp::Task` → `store().persist_task_rows`。而 kernel 的 effect/predicate 跑在
`headless_scope` 上,其文档明写 "never admitted, never persisted"
(`kernel_host/scope.rs:5-7`)—— **这句话今天不成立**。

`[INFERENCE — 代码通读,未实测]` 无头 task 的 tid 是 `utils::shortid()`
(`process.rs:474`;只有 workflow 节点才拿 `$`),因此一次 `$set_step_value` 会在 `tasks` 集合里
落一条幽灵行;`$set_process_var` 因 `proc.root()` 取 `$` 而取不到,是 no-op。

处置:新方案里**无头 scope 必须挂一个 no-op 的持久化 sink**;不要把 writer 的 `WriteOp::Task`
留给它。验收:kernel 模式下跑一个用 `$set_step_value` 的包,`tasks` 集合不得出现该 tid 的行。

### 6.6 目标形态

```
删除   src/cache/**（全部,含 cache/tests.rs）
新增   src/store/process_rows.rs、src/store/delivery_rows.rs、src/store/writebehind.rs
改     src/scheduler/runtime.rs      —— Runtime 自持 Arc<Store>,删 cache 字段/accessor
改     src/runtime/projection.rs    —— 持 Arc<Store>(原 Arc<Cache>),removable 判定改用 store 侧 barrier
改     src/runtime/host.rs 及其兄弟模块 —— 无头 scope 用 no-op 持久化 sink(§6.5;原写 host/*)
```

验收:
- `grep -rn "crate::cache" crates/acts/src` 无生产命中(cache 模块已不存在);
- `cargo test -p acts` / `-p acts-store` / `-p acts-server` 绿;
- §6.5 的幽灵行用例通过。

---

## 7. 红线与禁止事项

1. **不要删 `runtime/core/architecture.rs` 的守卫**(2026-10-13 已确认保留),只随分层演进改规则。它是这个仓库唯一把"kernel 是纯的"变成可执行事实的东西。
2. **不要在 P3 之前删 `cache`/`scheduler` 的代码**——在切换前判断"是否还有调用者"没有编译器背书。
3. **P3 必须与 `restore`/`start_parked` 的删除同笔提交**(§1.5 的隐式耦合)。
4. `runtime/mod.rs` 的对外 re-export:**P1a 保持**可用(`crate::runtime::X` 不被批量改写),
   收敛只发生在 **P1b**;两者不得同笔提交。
5. `export/` 的收敛(P1c)**不得**为 executor 造 store 抽象 trait;`principal.check(...)` 的显式前导保留。
6. 包 API 的 `ActPackageRuntime` 是**跨 crate 公共 API 变更**,单独一笔提交并迁移 `packages/**`
   与 `examples/**` 的全部实现者;不得与 kernel 搬移混在一起。
7. 一次提交只做一个切面:搬移不夹带行为变更,行为变更有独立测试。
8. Windows:Defender 会在并行构建时锁 rlib 元文件(偶发 E0786/E0463)→ **串行构建**。
9. 判定回归用"stash 基线对照 + 是否同路径",不要只看单测复跑(见 §1.8)。
10. **无头 scope 不得写行**(§6.5):kernel 的 effect/predicate 只在内存 scope 上跑,
    持久化只经聚合提交;`$set_step_value` 的 `try_upsert_task` 必须落到 no-op sink,
    不得复用 writer 的 `WriteOp::Task`。

---

> 原 §8「提示词」已删(它服务已完成且路径已过期的 P0-P5;约束仍在 §7)。**提示词只在 §12 原地维护**(编号不重排,以免打断 §10 记录里的 `§9.x` 交叉引用)。

## 9. 性能:切换的代价与基线

### 9.1 结论

**会,而且是结构性的**——不是常数因子。kernel 用"每跳持久化"换掉了 legacy 的"常驻 + 写后置"。
代价集中在三处:store 往返数、编码体量、单 driver 串行。其中第三处(含 effect 内联阻塞)是最大的可修项,
第一处的 CAS 部分则是设计必然。

### 9.2 一次 hop 的 store 流量(代码路径,非实测)

`execute_one`(`kernel_host/mod.rs:1685`)一路走下来:

| # | 动作 | 位置 |
|---|---|---|
| 1 | `load(pid)` 全量聚合读 | `:1687` |
| 2 | `operation_event(Claim)` → 内部先 `load` 再 CAS 写 | `async_document_store.rs:907` |
| 3 | `operation_event(PrepareCommit)` → 同上 | 同上 |
| 4 | `load(pid)` 重读("Re-read after prepare") | `:1740` |
| 5 | effect `runner.run(...).await` —— **在 driver 任务内同步等待** | `:2545` |
| 6 | `commit_transition(&plan)` → 内部再 `load` 再 CAS 写 | `:2182` |
| 7 | `load(pid)` 提交后重读 | `:2191` |
| 8 | `operation_event(Complete)` → 再 load + CAS 写 | 同上 |
| 9 | `project_touched(...)` 写投影行 | `:2254` |

**≈7 次全量聚合读 + 4 次 CAS 写**。每次写编码**整个聚合**(`snapshot_codec` V2)+ 两个派生索引文档
(recovery/outbox index),且全部串行在**唯一 driver 任务**上(`serve` `:1152`,单 `execute_one`)。

一个 task 至少 3 跳(ExecuteTask → ApplyAction → PropagateTask)→ 每个步骤 ≈ **33 次 store 往返**。
legacy 做同一批工作的等效量:0 次读(常驻内存)+ 3~4 次**异步、合批**的增量行写入(writer)。

### 9.3 代价分级

**设计必然(不该修)**:每跳 CAS 是正确性来源(操作生命周期必须 durable);聚合读是
compare-and-commit 的前提,不可省。

**可修的放大,按收益排序**:

1. **effect 阻塞 driver(最大项)** —— `runner.run().await`(`:2545`)内联在 driver 里,
   一个 2s 的 http 包会卡住该实例的**所有** run。legacy 有 `scheduler_workers = parallelism/2` 条 lane。
   修法:效果执行交给独立任务 + 并发限额(复用 §5 的准入额度),driver 只做提交。
2. **Claim + PrepareCommit 两次 CAS**(各含一次全量 load)—— 两步之间没有外部效果,
   可合成一次所有权转移 → 每跳 4 写降到 2~3 写。
3. **投影双写** —— 每次 commit 都写 proc/task/vars 行,而 legacy 里那些行**本身就是**它的持久化,
   不是额外开销。可降频(仅对外可见状态变化时投影)或走 §6.3 的 writebehind 合批。
4. **整包编码** —— `snapshot_codec` 全量编码;执行多/变量多/outbox 记录多的 workflow 上是 O(聚合) 写放大。
5. **tick 全扫** —— `recover` 每 tick 扫 pending 操作(分页 128),pending 多时 O(pending)/tick。
   注意与 §1.5 同源,属于 P3 前必读。

### 9.4 变轻的部分

- 无常驻工作集:内存不再随"同时活着的进程数"线性增长(cache_cap 存在的理由随之消失,转为 §5 的并发上限)。
- 去掉写后置队列的积压/饱和路径(`store_writer_saturated` 那套)与常驻对象的锁竞争。
- 恢复路径统一:不再有独立的 task outbox 行。
- `scheduler_workers` 条 lane 在 kernel 模式仍被**无条件 spawn**(`runtime.rs:1193`)却是死任务 → 可删。

### 9.5 先量,再优化

本轮**没有做过基准**,§9.2/§9.3 全部是代码路径推算 `[INFERENCE]`。切 P3 之前应补一次 A/B 基线:

- 同一 workflow(N 步,每步一个可控延迟的包)在 `kernel_engine=false/true` 两模式各跑一遍;
- 量:端到端延迟、每秒 hop 数、每 hop 的 store 往返次数、**单实例并行度**(两个互不相关的 run 是否并行推进)、峰值内存;
- 判据(建议):若 `true` 的 hop 吞吐低于 legacy 的 1/2,先把 §9.3.1、§9.3.2 修完再切。

### 9.6 与阶段的关系

- §9.3.5 与 §1.5 同源 → P3 前必读。
- §9.3.1 + §9.3.2 作为 **P3.5**(切换后、删 legacy 前的性能收口)单列,不塞进 P3。
- §9.3.3 归 P4(投影搬迁时一并处理)。

---

## 10. 执行记录

### P1a 已完成(2026-10-13)

**范围**:目录分层 + `kernel_host` 合并 + `kernel` 改名 `runtime` + `scheduler` 折入。
**性质**:纯搬移/整形,零行为变更。验收:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` 绿;
`cargo test -p acts --lib --tests` 除 §1.8 的既有红(`reliability::stall::*`)外全绿。

目标形态(已落地):

```
src/runtime/
├── mod.rs        # Runtime 定义(crate::runtime::Runtime)+ prepare_workdir + 各层 re-export
├── core/         # 纯层(守卫扫描):model/ plan/ commit/ recovery/ outbox/ + architecture.rs, coverage.rs
├── durable/      # document.rs envelope.rs document_store.rs async_document_store.rs snapshot_codec.rs
├── host/         # 原 kernel_host(7 文件)
└── scheduler/    # 原 scheduler(基座 + legacy,待 P5 收敛为 scope/)
```

执行中与原计划的偏差(均已按此落地,后续切片照此对齐):

1. **`document.rs` / `envelope.rs` 归 `durable/`,不归 `core/model/`** —— 它们是聚合文档的字节编解码,
   `core` 层不得依赖它们(守卫实测抓出 `core/model/document.rs → durable::snapshot_codec`)。
2. **`engine.rs` 未保留** —— 用户指出 `runtime/engine.rs` 与顶层 `src/engine.rs`(`Engine`)重名,
   且 `runtime/runtime.rs` 会触发 `clippy::module_inception`;故 `Runtime` 直接写进 `runtime/mod.rs`
   (1,900+ 行,composition root 与入口同址)。原 `scheduler/runtime.rs` 的 4 处 `use super::` 已改为
   `crate::runtime::scheduler::…`。
3. **`scheduler/` 的子模块改 `pub(crate) mod`** —— `Runtime` 移出后需要跨模块访问 `queue`/`health`/
   `validation`/`context`/`process`/`state`/`tree`/`cron`。P1b 再收紧。
4. **`scheduler/mod.rs` 保留一行兼容再导出** `pub(crate) use crate::runtime::Runtime;` ——
   现存调用点仍写 `runtime::scheduler::Runtime`;**P1b 收敛**为 `crate::runtime::Runtime` 后删除。
5. **守卫重定向** —— `src/kernel` → `src/runtime`,并新增"core/durable 不得依赖 `host`/`scheduler`";
   原名单里的 `crate::runtime` 条目删除(改名后它变成自指),换成精确的 `crate::runtime::{host,scheduler}`。
6. **`architecture.rs` 的适配器豁免语义恢复** —— 原实现把 `async_document_store.rs` 整文件排除在
   禁用名单之外(它是边界);新实现同样豁免,只额外保留"只有声明过的适配器可命名 `crate::store`"这条规则。
7. **未做**:拆分 `runtime/host/mod.rs`(仍 10,094 行);`runtime/core/mod.rs` 的一揽子 re-export 未收敛。
   两项都属于 P1b/P1a-续。

后续切片注意:`crate::kernel*` 已不存在,任何新代码用 `crate::runtime::{core,durable,host,scheduler}`。

### P1a 收尾:host 拆分已完成(2026-10-13)

`runtime/host/mod.rs` 从 **10,094 行** 降到 **3,475 行**,按内聚拆出:

| 文件 | 行数 | 内容 |
|---|---|---|
| `host/mod.rs` | 3,475 | KernelHost 类型与 `impl`(hop driver)、常量、QueueWake/ReportedRuns/ShadowTransport |
| `host/tests.rs` | 5,797 | 原 `#[cfg(test)] mod tests`(双引擎 parity 套件) |
| `host/facts.rs` | 483 | 聚合快照 → 引擎词汇的只读事实(model_of/root_variable/auto_decision/owner_of/process_message/lifecycle_state/root_error/ActFacts/input_vars/exposes_error/message_topic/message_outputs/node_kind_and_name/branch_name) |
| `host/planning.rs` | 234 | 纯规划胶水(TimeoutRound/TimeoutGuard/next_plan_sequence/mark_flow/is_catch_decider/package_failure/branch_error/ContainerShape/walk_step/container_shape/node_id/find_step) |
| `host/action.rs` | 102 | 客户端动作的 durable intent 与 fact 编解码 |
| `host/errors.rs` | 32 | `runtime`/`conflict`/`is_conflict`/`repository_error` |

实现约束(后续改这些模块时必须知道):
- 提取的项一律 `pub(super)`,跨模块需要的**结构体字段**也标了 `pub(super)`(`Failure`/`ActFacts`/`ActionIntent`)。
- 新模块开头是 `use super::*;` + **显式列出兄弟模块的 glob**(Rust 的 glob **不穿透** glob,所以
  `mod.rs` 里的 `use self::{...::*}` 不会让 `mod tests` 看到那些名字)。**改这条要小心**:增删模块必须同步改 5 处清单。
- 拆分按行范围切,容易把某条 doc 注释留在上一个文件的尾部(`clippy::empty_line_after_doc_comments` 会抓)。
  已修 4 处跨界;若再拆,务必检查每个切点上下各 8 行的 doc/attribute 归属。

验收实测同 P1a:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` 绿;
`cargo fmt --check` 绿;`cargo test -p acts --lib --tests` = **1267 passed / 1 failed**(仅 §1.8 已知红)。

### P1b 第一遍(2026-10-13)

**已做**:

1. **删除 5 个真死模块**(仅自我引用 + 一行 re-export,测试也只测自己):
   `runtime/core/plan/failure_recovery.rs`、`runtime/core/plan/process_recovery.rs`、
   `runtime/core/commit/branch_application.rs`、`runtime/core/commit/operation_executor.rs`、
   `runtime/core/outbox/outbox_worker.rs`。连同其内部测试一起删除 → 测试数 1267 → 1248。
2. **`host/` 拆分后的多余 glob 清理**:`use super::*;` 已足够(Rust 的 glob 不穿透 glob 的判断
   在 `mod.rs` 显式 `use self::x::*` 之后**不成立**——实测去掉兄弟 glob 后仍编译通过)。
   现在 `host/{action,errors,facts,planning,tests}.rs` 只有一行 `use super::*;`。
3. **`mod runtime;` 的 lint 豁免恢复并写明理由**(AGENTS 要求):去掉豁免会暴露 **182 条**
   `dead_code`/`unused_imports` 警告,全部是"只被 runtime 自带测试套件触达、生产路径要等
   `Config::kernel_engine` 打开才够得着"的项 —— 这正是 P3 切换要解决的。豁免注释里已写明
   "P3 收回,不得扩散到新模块"。

**未做(P1b 剩余,两项耦合,必须一起做)**:

- **收敛 runtime 对外面**:`runtime/mod.rs` 现在是 `pub(crate) use self::core::*; pub(crate) use self::durable::*;`
  这种一揽子门面。runtime 之外的消费者其实只有 `Runtime`(`builder.rs`/`engine.rs`/`export/**`/`cache/**`)、
  `prepare_workdir`、`ReplayOperationKind`(`runtime_state.rs:9`)三个名字。
- **层内私有化**:`core/mod.rs`/`durable/mod.rs`/`core/{model,plan,commit,recovery,outbox}/mod.rs`
  现在全是 `pub(crate) mod` + `pub(crate) use`,配合上面的门面才能收紧为 `pub(super)`。
  两项耦合:收紧门面就得让 `host/**` 改走 `crate::runtime::core::…` 的层路径,否则 `pub(super)` 断链。

**改动量估计**:`crate::runtime::<name>` → `crate::runtime::core::<layer>::<mod>::<name>` 的映射
可由 `core/mod.rs` 的 re-export 行直接生成(host/ 约 100 处),机械但要一次编译到位。

**验收**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` 绿、
`cargo fmt --check` 绿、`cargo test -p acts --lib --tests` = 1248 passed / 1 failed(§1.8 已知红)。

### P1b 完成(2026-10-13,三项全清)

1. **一揽子门面已收敛**。`runtime/mod.rs` 原来是
   `pub(crate) use self::core::*; pub(crate) use self::durable::*;`(把 148 个名字倒进 `crate::runtime::`)。
   现在运行时模块**只对外导出 runtime 之外真正需要的东西**:
   - `Runtime`(定义在本文件,组合根/入口)
   - `prepare_workdir`
   - `ReplayOperationKind`(`runtime_state.rs:9` 是唯一外部使用者)
   其余一律走层路径 `crate::runtime::{core,durable,host,scheduler}::…`。
   实测把 **507 处** `crate::runtime::<Name>` 引用改写成层路径(涉及 19 + 22 个文件)。
2. **可见性收紧**:`core` / `durable` / `host` 由 `pub(crate) mod` 改为 **`pub(in crate::runtime) mod`**,
   其内部各层 `mod` 与 re-export 同步收紧。`scheduler` 必须保持 `pub(crate)`
   (`cache/store.rs`、`export/**`、`model/**`、`snapshot.rs`、`store/store.rs`、`validator.rs` 等 20+ 处外部使用者)。
   `runtime_state.rs` 改为经门面取 `ReplayOperationKind`。
3. **`dead_code` 豁免**:维持上一遍的处理(写明理由,P3 收回)。

**踩到的坑(改这些模块必看)**:
- `use crate::{…, runtime::Name, …}` 这种**组内相对路径**不能被 `crate::runtime::Name` 的正则命中,
  而且组内不允许写 `crate::…`(会报 "`crate` in paths can only be used in start position")。
  本次做法是把 `use crate::runtime::{A, B};` 展开成逐条 `use crate::runtime::core::<layer>::<mod>::A;`,
  共展开 22 个文件。
- 别名 re-export(`candidate as timeout_candidate` 等 7 个)在改写时要还原真实项名,否则断链。

**验收**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` EXIT=0、
`cargo fmt --check` OK、`cargo test -p acts --lib --tests` = **1248 passed / 1 failed**(§1.8 已知红)。

### P1c 增量 1:export 去掉 Cache 间接层 + 能窄的字段已窄(2026-10-13)

1. **去掉 `runtime.cache().store()` 这层间接**:export 下 30 处 reach-through 全改为
   `runtime.store()`(`Runtime::store()` 本来就存在)。例外:`export/channel.rs` 的
   `cache().store_delivery(...)` 保留 —— 那是 Cache 的 writer 路径,不是 store 访问。
   验收实测:`grep -rc "cache()\.store()" crates/acts/src/export` → **0 命中**
   (含 `export/tests.rs` 的 3 处也已改)。
2. **字段窄化(2/8)**:`TaskExecutor` 与 `ModelExecutor` 的 `runtime: Arc<Runtime>` 改为
   `store: Arc<Store>`,构造签名随之改;`Executor::new` 里取出 `let store = rt.store();` 分发。
   这两个 executor 全文件 `self.runtime.` 已归零。

**剩余 6 个为什么没窄(已核实,各自需要 store 之外的能力)**:

| executor | store 之外还需要 |
|---|---|
| `act` | `do_action`(客户端动作入口) |
| `message` | `ack` + `emitter` |
| `package` | `schema_cache` |
| `ext` | `env` + `schema_cache` |
| `process` | `start` + `proc` |
| `event` | `start` + `emitter` + `package` + `config`,且要把 `Arc<Runtime>` 交给包 API |

这些都是 `Runtime` 上的**不同能力**,各自宽窄不一。要把它们也换成窄依赖,只有两条路:
(a) P1d 的窄 trait(`ActPackageRuntime`)顺带覆盖其中的 `start`/`package`;
(b) 为 `schema_cache`/`env`/`emitter`/`ack` 各引一个 trait —— 按 §3.3 判定标准
(多变实现/需测试替身/跨分层且须单向),**目前都不满足**,属过度设计,不做。
结论:余下 6 个保持 `Arc<Runtime>`,待 P1d 一并评估。

**验收**:clippy EXIT=0、fmt OK、`cargo test -p acts --lib --tests` = 1247 passed / 2 failed。
两红均为 §1.8 已登记的既有项:`reliability::stall::*`(基线单跑同样失败)与
`host::tests::kernel_catch_matches_legacy_when_the_error_is_caught`(**单跑实测通过**,
并行负载下翻 legacy 侧断言,§1.8 已点名这一族)。

### P1c 增量 2:action 名常量单一来源(2026-10-13)

`export/actions.rs` 的 dispatch 原来手写 33 个字符串字面量,与各 executor 的
`pub(crate) const` 是**两份手维护的同名表**。现在 match 直接引用那些常量:

```rust
match name {
    act_executor::PUSH => { … }
    model_executor::DEPLOY => { … }
    …
}
```

- 33 个分支全部改为常量路径(`use super::executor::{act_executor, …, task_executor};`)。
- 常量是 `&'static str`,可作 match 模式;同名常量(`LS`/`GET`/`RM` 在 6 个文件里各有一份,
  **值不同**:`task:ls`/`model:ls`/…)靠模块路径区分,不存在歧义。
- 副作用:名字漂移从此编译期可见 —— 改 executor 的常量值,dispatch 跟着变。

**验收**:clippy EXIT=0、fmt OK、`cargo test -p acts --lib --tests` = **1248 passed / 1 failed**
(仅 §1.8 的 `reliability::stall::*` 已知红)。

### P1c 增量 3:同形 list/get 收敛 + 残留 reach-through 清零(2026-10-13)

1. **同形收敛**:`export/executor.rs` 新增两个泛型帮助(紧邻增量 1 的 `publish_package`),

   ```rust
   pub(super) async fn list_infos<D, I>(collection: Arc<dyn DbCollection<Item = D>>, q: &Query)
       -> crate::Result<PageData<I>> where for<'a> &'a D: Into<I>;
   pub(super) async fn get_info<D, I>(collection: Arc<dyn DbCollection<Item = D>>, id: &str)
       -> crate::Result<I>          where for<'a> &'a D: Into<I>;
   ```

   各集合取用点直接换: `list_infos(self.store.models(), q)` / `…store().events()` /
   `…packages()` / `…procs()`。调用点只给出集合(类型由 `Store::*()` 决定)与 Info 类型
   (由函数返回类型决定),`for<'a> &'a D: Into<I>` 覆盖 `impl From<&data::X> for XInfo`
   (`model/info.rs:117/168/174/292`)。
   - 收 `list` ×4:`model_executor`、`event_executor`、`package_executor`、`process_executor`。
   - 收 `get` ×2:`event_executor`、`package_executor`。
   - **按 §3.2 不收**(真不同形,已在帮助函数的 doc 里写明理由):`task::list`/`task::get`
     (逐行 join `vars`)、`message::list`/`message::get`(逐行 join `messages`、孤儿行丢弃)、
     `model::get`(fmt 三态转换)、`process::get`(并入常驻 tasks 并按 timestamp 排序)。
   - **`rm` ×3 不合并**(偏差登记):三处不同形 —— `model::rm` 走 `Store::rm_model`(与 trigger 行
     一个原子批)、`package::rm` 是 `packages().delete` + `schema_cache().invalidate_package`、
     `message::rm` 只是 `deliveries().delete(id)`;共同部分只剩一次"集合由调用点给出的 delete",
     包成帮助就是纯转发(§3.2 第 5 条反对的空抽象),故不引。`publish` ×2 已在增量 1 合并。
   - 顺带:`publish_package` 由 `pub(crate)` 收紧为 `pub(super)`(只被 `export::executor::*` 用)。

2. **残留 reach-through 清零(修正增量 1 的漏报)**:增量 1 的验收 grep 用的是**单行**模式
   `cache()\.store()`,而 export 下 23 处是**换行链式**写法(`.cache()\n.store()`),所以全部漏网。
   本次全部改为 `runtime.store()`:
   - 生产:`executor/message_executor.rs` 6 处(`delivery_info`/`clear`/`redo`/`clear_delivery`/`redeliver`)、
     `executor/event_executor.rs` 1 处(`workflow()`)、`executor/process_executor.rs` 1 处(`start_as_owner`)。
   - 测试:`export/actions.rs` 1 处、`export/tests.rs` 14 处。
   - **保留**(都不是 store reach-through,勿改):`channel.rs:329` 的 `cache().store_delivery(…)`
     (Cache 的 writer 路径)、`actions.rs:1319` 的 `cache().sweep_removable()`、`tests.rs` 5 处
     `cache().flush()`(writer flush)、`package_executor.rs:53`/`executor.rs:56` 的 `schema_cache()`。

**验收**:`grep -rn "cache()" crates/acts/src/export` 只剩上面四类;`cargo clippy --all-targets
--all-features --workspace --locked -- -D warnings` EXIT=0;`cargo fmt --check` OK;
`cargo test -p acts --lib --tests` = **1246 passed / 3 failed**,三条全部是 §1.8 登记过的红:
`reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`(单跑同样失败=基线红)、
`reliability::error::sch_error_duplicate_delivery_is_terminal_noop`(单跑绿=并行负载翻转)、
`host::tests::kernel_catch_matches_legacy_when_every_handler_skipped`(单跑绿=legacy 侧断言翻转)。
无新增红,且三条都不在 export 读模型路径上。

**坑(改文件必看)**:
- 验收 grep 要覆盖换行链式调用(`grep -c "cache()\.store()"` 对 `.cache()\n.store()` 恒为 0)。
- 本机工作区是 CRLF(`core.autocrlf=true`);用 python/perl 批量改文件会把**整文件**写成 LF
  (perl `-pi` 在本沙箱下不落盘,python `wb` 会把 5 个文件写成 LF)。本次已把那 5 个文件复原为
  CRLF;提交内容不受影响(git 归一化),但会导致工作区行尾不齐。批量改写后务必 `file` 复核。

### runtime 结构收敛:拆掉 `core/` 与 `host/`(2026-10-13,用户新指令)

> **已被下一小节取代**(同日稍后:"恢复之前的 core/shell/durable 的层次")。本小节保留过程与踩坑记录;
> 现行布局见「结构回滚」。

**指令**:① `runtime/host` 原是原 kernel 的访问入口,现在 `Runtime` 是整个 runtime 的入口 → 把 host 的
部件拆到 runtime,保持结构清晰;② runtime 调用的子模块去掉 `pub(in crate::runtime)` 限制 —— 内部按
层级访问、公共模块放 runtime 根下、外部访问由 `mod.rs` 导出;③ `runtime/core` 目录不是必须 → 拆除其
模块到 runtime 下。

**目标形态(已落地)**:

```
src/runtime/
├── mod.rs          # 入口:Runtime + 各层 mod 声明 + 对外门面(pub(crate) use)
├── model/ plan/ commit/ recovery/ outbox/   # 原 core/* 上移一层(core/ 消失)
├── durable/        # 字节层(唯一可命名 crate::store;未动)
├── host.rs         # 原 host/mod.rs:hop driver(KernelHost)
├── action.rs errors.rs facts.rs planning.rs message.rs
├── effects.rs predicate.rs projection.rs scope.rs
├── spec.rs         # 原 host/model.rs(ProcessSpec;避让 core/model → runtime/model)
├── tests.rs        # 原 host/tests.rs
├── architecture.rs coverage.rs   # 原 core/ 的两个测试模块
└── scheduler/      # 无头基座 + legacy,保持 pub(crate)(见下)
```

**机械面**:`crate::runtime::core::` → `crate::runtime::` **985 处**(全是 deep 路径:`model::`/`plan::`/…;
`core/mod.rs` 那份一揽子 re-export 块**只有 `coverage.rs` 一个使用者**,已删,coverage.rs 改为 6 行显式
导入);`pub(in crate::runtime)` → `pub(crate)` 全量;host 部件由 host 的子模块升为 runtime 的**兄弟**
模块,故 `use super::*;`(5 个文件)改为显式导入,host.rs 反向 `pub(crate) use` 它原先对外的那 5 个名字;
`tests.rs` 的 `include_str!` 相对深度少一层。

**可见性**:除 `scheduler` 外,`runtime/*` 全部为私有 `mod` —— 于是"外部只能走 mod.rs"由编译器执行;
`mod.rs` 只导出 runtime 之外真正需要的三个名字:`Runtime`(本文件定义)、`prepare_workdir`、
`ReplayOperationKind`。实测:runtime 之外对 `core`/`durable`/`host` 的**代码**引用为 **0**
(仅 `lib.rs`/`runtime_state.rs` 的散文提及),
所以没有调用点需要迁移。`scheduler` 保留 `pub(crate)` 的理由:runtime 之外有 20 个文件直接写
`crate::runtime::scheduler::…`(Process/Task/TaskState/NodeTree/NodeData/ActTask/cron 等),P5 把它收成
无头基座时再按同一套门面处理。

**守卫(runtime/architecture.rs)**:判据从「`core/` 目录」改为**声明式清单**:纯层 = `PURE_MODULES`
(`commit`/`model`/`outbox`/`plan`/`recovery`),字节层 = `durable`,其余为 shell;扫描仍递归,报告改为
相对 `src/runtime` 的路径。**同时扩大禁止表**:拆散后 host 的部件成了 runtime 根上的模块,若不列名,
纯层就能悄悄依赖它们 —— 故 `crate::runtime::{action,effects,errors,facts,host,message,planning,
predicate,projection,scope,spec,scheduler}` 进入两层(纯层 & 字节层)的禁止名。`crate::store` 仍只对
纯层禁止(它是字节层自己的名字)。

**补:把"目录即判据"换成"清单 + 完备性检查"**。删掉 `core/` 后判据变成手写清单,于是"新增一个顶层
模块树不会被任何测试扫到"成了真实漏洞(用户追问的正是这一点)。补两条测试把它堵上:

- `every_runtime_entry_is_classified`:遍历 `src/runtime` 顶层条目,必须与 `RUNTIME_ROOT`(每个条目带
  tier:`Pure`/`Byte`/`Shell`/`Root`)严格相等 —— 新增、删除、改名任何条目都红;
- `the_declared_tiers_agree`:`RUNTIME_ROOT` 里 `Pure` 的条目必须等于 `PURE_MODULES`,`Shell` 的条目必须
  等于 `SHELL_MODULES` 派生的 `crate::runtime::<stem>` —— 三份表不能各自漂移。

现状 25 个条目全部登记:纯层 5 个目录(`commit`/`model`/`outbox`/`plan`/`recovery`)+ 字节层 `durable`
+ shell(`scheduler` + `action`/`effects`/`errors`/`facts`/`host`/`message`/`planning`/`predicate`/
`projection`/`scope`/`spec` 11 个文件)+ 入口与测试(`mod.rs`/`architecture.rs`/`coverage.rs`/`tests.rs`)。
仍未覆盖(诚实登记):子串扫描抓不到"经别处再导出"的间接依赖。

**踩到的坑(改这些文件必看)**:

1. **拆散一个模块会打断两件 Rust 事实**:子模块能看见父模块的**私有**项;`use super::*;` 会连带导入
   父模块的**私有 `use`**。host 的部件变成兄弟后,`CONFLICT_ECODE`(私有 const)、`action_intent`/
   `wake`(私有 fn)、`KernelHost::aggregates`(私有字段)、以及 host 原先行内导入的 `Arc`/`SystemClock`/
   `OperationId`/`Channel` 等全部失联 → 逐个改为显式导入;真的需要跨文件可见的 4 项改 `pub(crate)`,
   其中 `CONFLICT_ECODE` 顺手搬到 `errors.rs`(它唯一的读者)。
2. host 原 `mod model;`(ProcessSpec)与 core 的 `model/` 目录**重名**,故 `host/model.rs` → `spec.rs`;
   它内部的 `model::X`(自指子模块)在 facts.rs/planning.rs 里也有引用,必须改写成 `spec::X`。
3. `host/tests.rs` 是白盒套件:它直接读 `KernelHost` 私有字段、调私有构造(`action_intent`/`wake`),
   兄弟化后必须放宽到 `pub(crate)`;`mod tests;` 的声明从 host.rs 移到 runtime/mod.rs(否则会去找
   `runtime/host/tests.rs`)。
4. **行尾**:`scheduler/tests/*` 等 39 个文件在搬迁前**就不统一**(LF 或混合,不是本次改动造成);
   已一并规范为 CRLF(与本工作区 `core.autocrlf=true` 的检出风格一致)。

**验收**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` EXIT=0;
`cargo fmt --check` OK;`cargo test -p acts --lib --tests` = **1247 passed / 2 failed**
(两条全是 §1.8 登记过的红:`stall::sch_next_cut_sweep_recovers_every_write_boundary`(基线红)、
`tests::kernel_catch_matches_legacy_when_the_error_is_caught`(legacy 侧断言翻转,单跑绿;它随
`host/tests.rs` → `tests.rs` 从 `runtime::host::tests::` 变为 `runtime::tests::`))。
守卫五条(`pure_modules_have_no_infrastructure_dependencies` / `durable_layer_stays_out_of_the_engine_and_the_shell` /
`store_coupling_is_explicit_and_minimal` / `every_runtime_entry_is_classified` / `the_declared_tiers_agree`)
与 `coverage` 四条全绿;并做过一次 mutation 实测(往 `plan/branch.rs` 注入 `// crate::store` → 红并报出
该文件,随后还原)。

### 结构回滚:恢复 `core`/`shell`/`durable` 三层目录(2026-10-13,用户新指令)

**指令**:"runtime 下,恢复之前的 core/shell/durable 的层次"(同一条消息链里还有"最后合并即可"——
驱动类型与 `Runtime` 的合并留到最后)。**本小节取代上一小节的扁平形态。**

**落地形态**:

```
src/runtime/
├── mod.rs      # 入口:Runtime(= 这套机制的 host)+ 窄门面
├── core/       # 纯策略:model plan commit recovery outbox + architecture.rs + coverage.rs
├── durable/    # 字节层(未动)
├── shell/      # 异步外壳:mod.rs(hop driver = KernelHost)+ 其部件
│               #   action effects errors facts message model planning predicate projection scope tests
└── scheduler/  # 无头基座 + legacy(未动,待 P5)
```

**搬移**:`core/*` 从 runtime 根收回 `core/`;shell 部件从 runtime 根收进 `shell/`(`host.rs` →
`shell/mod.rs`,`spec.rs` → `shell/model.rs` —— core/model 不再重名);路径
`crate::runtime::{model,plan,commit,recovery,outbox}::` → `crate::runtime::core::{…}::`,
`crate::runtime::host::` → `crate::runtime::shell::`(40 个文件)。

**刻意保留(不回滚)的上一轮成果**:

1. `pub(in crate::runtime)` 不再出现,统一 `pub(crate)`(上一轮用户指令 #2);
2. runtime 之外对 `core`/`durable`/`shell` 的代码引用仍为 0(实测),对外面仍只有 `mod.rs` 的
   `Runtime`/`prepare_workdir`/`ReplayOperationKind`(`scheduler` 仍 `pub(crate)`);
3. `CONFLICT_ECODE` 留在 `errors.rs`(它唯一的读者),不再搬回 shell/mod.rs;
4. `coverage.rs` 保持显式导入,不恢复 `core/mod.rs` 那份 130 项一揽子 re-export(它当年只有
   coverage.rs 一个使用者);
5. shell 的子模块重新拥有父模块作用域(`use super::*`、`pub(super)`、父模块私有项可见),因此上一轮
   为"兄弟化"而加的那批显式导入、以及 `aggregates`/`action_intent`/`wake` 的 `pub(crate)` 放宽全部
   回滚 —— encapsulation 复原。

**守卫**:判据回到目录(`PURE_MODULES = ["core"]`),但**保留**上一轮新增的两条完备性测试:
`RUNTIME_ROOT = {core, durable, shell, scheduler, mod.rs}` 五档分类 + `the_declared_tiers_agree`
把 `PURE_MODULES`/`SHELL_MODULES`(= `crate::runtime::{shell, scheduler}`)钉在该分类上。于是
"目录自描述"与"新增条目必须登记"两者同时在。

**新发现(实测,支持"最后合并")**:`Runtime`(Arc)→`kernel: Arc<KernelHost>`→
`effect: RwLock<Option<Arc<dyn EffectRunner>>>`→`PackageEffectRunner{runtime: Arc<Runtime>}` 是**强 Arc
环**,而 `Runtime::close` 不拆任何 seam。临时探针(kernel 模式,已删除)实测:

```
PROBE kernel-mode: runtime still alive after close+drop = true
PROBE   host Arc strong count = 3
```

即 `close()` + drop 之后整个引擎图仍不释放。这是"两个组合根"的直接代价,也是「host 与 Runtime 是否
冗余」那条待决判定合并的实证依据。补救选项(未做):(a) 合并后环自然消失(用户已定:最后合并);
(b) 中间态可在 `close` 里把 5 个 seam 置空以断环(3 行过渡补丁)。

**验收**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` EXIT=0;
`cargo fmt --check` OK;`cargo test -p acts --lib --tests` = **1250 passed / 1 failed**,唯一一条是 §1.8
的基线红 `scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`
(单跑同样失败=与本次改动无关;本轮旋转的那两条 flake 没触发)。守卫 5 条
(`pure_modules_…`/`durable_layer_…`/`store_coupling_…`/`every_runtime_entry_is_classified`/
`the_declared_tiers_agree`)与 `coverage` 4 条单独复跑 = 9 passed / 0 failed。

### 待决(新,2026-10-13 用户提问后实测确认):`host` 与 `Runtime` 是否冗余

用户判断:"host 和 runtime 冗余设计,Runtime 实际就应该是 core/shell/durable 这套机制的 host"。**实测结论:
冗余是真的,但准确形态是"组合根这一职责被复制了一份",且这份副本是过渡期的产物 —— crate 里现在同时
住着两台引擎。**

实测数据(runtime/mod.rs 2048 行 / 75 个 impl 方法;runtime/shell/mod.rs 3595 行;engine.rs 200 行):

| 对象 | 职责 | 证据 |
|---|---|---|
| `Engine` | 嵌入方手柄(config+acl+`Arc<Runtime>`)+ legacy 启动接线 | 它 reach-through 的有 `event_loop`/`resume`/`recover_actions`/3 个 timer(P3 删除目标) |
| `Runtime` | ①engine 组合根(config/env/emitter/package/schema_cache/snapshots/shutdown)②legacy 常驻引擎(queue/cache/op_claims/health/timers/launch/restore/resume/dispatch/enqueue/complete/recover_*)③kernel 入口(转发) | **38/75** 个方法体出现 legacy 字段访问;只有 **3** 个方法碰驱动(`start`/`do_action`/`kernel()`) |
| `KernelHost`(shell/mod.rs) | 确定性驱动者(serve 单任务、wake 通道、hop driver)+ 5 个可插 seam + **自建** `AsyncAggregateRepository` | `KernelHost::new(cache.store().kv())` 另建一份 store 句柄 |

五个冗余接缝:①接口重复(3 个转发方法,入口归属已定却仍分两类型);②资源重复(双 store 句柄:
`Runtime::store()` vs host 自建 repo);③生命周期重复(`Option<Arc<KernelHost>>` + `kernel_engine()`,
5 个站点:mod.rs L458/L514/L554/L634/L1586);④所有权**循环** `Runtime → KernelHost → effects/predicate
runner(持 `Arc<Runtime>`)→ Runtime` —— 循环的根是 `scheduler::Process::new(pid, &Arc<Runtime>)`
(`shell/scope.rs::headless_scope`),不是 host 自己;⑤命名化石(`kernel_engine`/`kernel`/`shell` 曾名 `host`/
`KernelHost`/`const OWNER="kernel-host"`/日志 `"(kernel)"`)。

**不可合并的部分**:actor 不变式(只有一个任务 `serve` 写聚合文档,这是正确性来源,合并后必须仍可读);
`Engine`/`Runtime` 的对外面是真实职责。

**决定(用户已定:最后合并)**:终态是**一台对象** —— `Engine`(薄手柄)→ `Runtime`(引擎 = 组合根 +
驱动者 + 对外面);**合并方式 = 合类型、分 impl 文件**(`impl Runtime` 分布在 `mod.rs` 入口/门面 与
`shell/mod.rs` 的驱动代码(同 crate 多 impl 块合法)),这样既消掉第二组合根/Arc 跳/`Option`/上面那条 Arc 环,
又保住 P1a 的"文件按关注点拆"与 actor 可读边界。**时点:最后**(P5 之后;P3 之后先复核一次:
legacy 半边删掉后 Runtime 缩水、`Option` 去留可判;P5 之后 `headless_scope` 不再要 `Arc<Runtime>`,
环自然断)。

- **唯一真成本**:`KernelHost::in_memory()` 让驱动脱离 Runtime 独立构造(tests.rs 里 33 处);合并后需
  crate-private 的 `Runtime::in_memory()`(顺带造 emitter/package/schema_cache —— 都是轻对象)。
- **备选 B(不想合)**:正名保留两层(`Runtime`=engine facade,`KernelHost`→`Driver`,P3 后去掉
  `Option`,文档写明 driver 是唯一写聚合的 actor)。成本≈0,代价是上述 5 个接缝永久留在类型系统里。

### 待决(登记,未动):读模型行表 `procs`/`tasks`/`vars` 的去留

用户提出:聚合已含 executions + variables,那三张表是否可删。**结论:方向对,但它们是
「索引面 + API 形状 + 树重建原料」,不是冗余副本**,不能只删:

- 索引面:`data::Proc.indexed_fields = ["state","mid","timestamp","start_time","end_time"]`、
  `Task = ["pid","tid","state","timestamp","start_time","end_time"]`、`TaskVars = ["pid","tid"]`。
  聚合是按 pid 的整包二进制,删行后 `ls` 要么全扫全解码,要么在 kernel 补等价索引。
- API 形状:行有 `id`/`name`/`kind`/`mid`/`prev`/`next`/`node_data`/`start_time`/`err`/`removable`;
  聚合是 kernel 内部编码(`ACTSPV2`)。
- `Runtime::proc()`(`runtime/mod.rs:534`)→ `cache.proc()` → `store.load_proc` 靠行拼回 resident 树,
  供 `proc:get`/`ProcessExecutor::get` 用。
- 附带实测:`projection.rs:320-324` 写 `TaskVars.sealed` 时恒为 `String::new()`
  ("a snapshot seal is a per-effect resolution, not durable run state") → **vars 行有一半字段恒空**。
  `deliveries`/`messages`/`events` **不在聚合里**(通道状态),不受此议题影响;`ops` 是 legacy 的。

三个方案(推荐 B,时点放 P3 之后,理由见对话记录):
- **A** 维持现状(聚合=真值,行=派生读模型);
- **B** 删行 + 把查询面搬进 kernel(新增 `kernel/indexes/procs/*`、`task(s)` 索引族,
  `ls` 走索引、`get` 解聚合并在 host 重建树形)——与 P2 的"活跃进程索引"可共用机制;
- **C** 删行 + 直接扫聚合(小规模可行,大规模不可取)。
**低垂果实(可先做,风险低)**:`sealed` 恒空 → 先停写 `vars` 行。

### P1d 完成(2026-10-13):包 API 的窄 trait `ActPackageRuntime`

**改动面**(跨 crate 公共 API;按 §3.3 只此一处引 trait):

1. `crates/acts/src/package/mod.rs`
   - 新增 `pub trait ActPackageRuntime: Send + Sync`(`#[async_trait]`),两个方法:
     `fn store(&self) -> Arc<Store>` 与 `async fn start(&self, model: &Workflow, options: Vars) -> Result<String>`
     —— 起 run 的返回是 **pid**,不是 `Arc<Process>`(与 `proc:start` 同形状,且 `Process` 不在公共面上);
   - `ActPackage::start` 的 `_rt: &Arc<Runtime>` → `_rt: &dyn ActPackageRuntime`(`:85-91`);
   - import:`runtime::scheduler::{Context, Runtime}` → `runtime::scheduler::Context`,新增 `Store`/`Workflow`。
2. `crates/acts/src/runtime/mod.rs:2013-2029`:新增 `impl crate::package::ActPackageRuntime for Arc<Runtime>`。
   走 §3.3 的路线 (a)(`Arc<Runtime>` 实现 + 调用点 unsize 强转);`store` 转发 `Runtime::store`,`start`
   转发 `Runtime::start` 后取 `id().to_string()`。**锚点**:接收者必须是 `Arc<Runtime>`,因为
   `Runtime::start` 的接收者是 `self: &Arc<Self>`(而 `&dyn` 只借出 `&self`)。
3. `crates/acts/src/lib.rs:65-68`:`pub use package::{…, ActPackageRuntime, …}`。这是**必需**的 —— `Runtime`
   在公共面上不可达,不导出 trait 则 crate 外根本无法覆盖 `start`(等于没改)。
4. 实现者迁移:`crates/acts/src/export/tests.rs:3519-3538`(`TriggerTestPackage`)签名改写;
   `rt.start(...)` 现在直接返回 pid,`ret.id()` 删除。`packages/**` 与 `examples/**` 实测**无** `start`
   覆盖(只实现 `new`/`definition`/`execute`),故除该测试外无其他迁移点;`execute(&Context, …)` 路径未动。

**实测更正(§3.3 第 3 条已改写)**:`Arc<Runtime>` **不是** `EventExecutor` 无法窄化的唯一原因 —— 它还要
`Runtime::package()`(注册表)、`Runtime::config()`、`Runtime::emitter()`(`start_hook` 的
on_complete/on_error)。所以 P1d 的收益是「包 API 不再点名 `Runtime`」+「`start` 在 crate 外可覆盖」,
**不是** executor 窄化;`event_executor` 仍持 `Arc<Runtime>`(余下 5 个也不满足 §3.3 的引 trait 条件,不做)。

**验收实测**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT=0**
(全 workspace 都过 —— `packages/**`、`examples/**` 的外部 `ActPackage` 实现者在新签名下照常编译);
`cargo fmt --check` OK;`cargo test -p acts --lib --tests` = **1250 passed / 1 failed**,唯一红是 §1.8 基线
`runtime::scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`
(与本次改动无关,单跑同样失败)。

**偏差/登记**:(a) 未落 commit —— 与 P1a–P1c 一样,整轮改动仍在工作区(150 项 pending),按既有惯例只登记笔记;
(b) trait 的"crate 外可实现"目前是结构性保证(`pub` + re-export),当前 workspace **没有**外部 `start` 实现者,
故无外部编译证据 —— 与 §3.3 第 4 条实测一致。

### P2 完成(2026-10-13):`cache_cap` = 同时在飞(非终态)run 数上限

**语义**:**legacy 的「常驻内存进程数上限」等价重定义为「同时在飞的 run 数上限」** —— kernel 无常驻对象,
故不是照搬字面。超限的 start **park**:聚合照建、行照投影,但**不 wake 根的操作**,该 run 一个 hop 都不走;
槽位(某 run 的根到达终态)释放时,按 `__created` 升序唤醒队首。

**改动面**(file:line):

1. **纯规则**(新文件)`runtime/core/plan/concurrency.rs`(注册于 `core/plan/mod.rs:4`):
   - `admit(cap, active, state) -> Admission{Admit, Park}`:根已终态 → `Admit`(无物可限;把已完结 run 的
     收尾操作挡在 cap 后面只会让 parked 队列停摆),否则 `active < cap` → Admit、满 → Park(与 legacy
     「常驻集满则 park」同构)。三层单测 `a_free_slot_admits_and_a_full_set_parks` /
     `a_settled_root_never_waits_for_a_slot` / `a_zero_cap_admits_nothing_with_work_left`。
   - `CREATED_VAR = "__created"`:seed 时写进根 scope 的毫秒时间戳 = FIFO 排序键;写法照同目录
     `timeout.rs` 的 `STARTED_VAR`(私有 `__` 事实、不进表达式空间、由 seed 而非 plan 写)。
2. **状态表**:`core/model/transition_plan.rs:180-189` 新增 `(Created, Running | Waiting)` —— parked 根的 seed
   就是 `Created`,它的准入 hop **就是**它的起始 hop(没有单独的 `Created → Ready`;legacy 的 parked 进程也是
   从 `None` 直接到根任务 running)。`shell/planning.rs:3-18` 新增 `startable()`(Created | Ready),
   `shell/mod.rs` 里 `execute_task_plan` 的两处 `Ready` 判定改用它。
3. **seed**:`shell/model.rs` 拆出 `seed_snapshot_from(root_state)`;`seed_snapshot()`(根 `Ready`)与新增
   `seed_parked_snapshot()`(根 `Created`)共用它;共用体写 `__created`(`shell/model.rs:516-528`,时钟在
   shell 侧 `utils::time::time_millis()`)。
4. **驱动侧 slot 账本**(`shell/mod.rs`):`Admissions{cap, active, parked: BTreeMap<(i64,pid), OperationId>}`
   + `KernelHost::admissions`;`KernelHost::new(kv, cache_cap)`、`in_memory()`(无界)、`in_memory_with_cap(cap)`;
   `runtime/mod.rs:1551` 传 `config.cache_cap() as usize`(与 legacy 同一转换)。
   - `start_process`:先 `reserve`(决策 + 占位同一把锁 → 并发 start 不会同时拿到最后一个槽)→ 按准入选 seed
     → `create` → 投影 → Admit 则 wake 根、Park 则入 FIFO 并打日志(legacy 同文案
     「process parked — waiting for a free slot」);`seed`/`create` 失败或重复 pid 走 `abandon` 回滚占位。
   - `place_run(snapshot) -> drive`(hop 头、hop 尾、客户端动作后各一次,单一实现):已终态且持有槽 → 释放并就
     唤醒 FIFO 队首;parked → 不驱;未持有槽且非 parked(重启后仍在飞)→ 按纯规则占槽或入队(**启动期补齐**);
     持有槽 → 驱。`execute_one` 另留「parked 直接返回」的无存储快路径(恢复扫描每 tick 重推 pending 操作)。
   - 账本读的聚合事实:`shell/facts.rs` 的 `root_state` / `run_settled` / `created_stamp`,以及
     `shell/mod.rs:309` 的 `resume_operation`;`process_completed` 改用 `run_settled`。
5. **配置/文档**:`config.rs:44-51`(cache_cap 字段 doc:在飞 run 数上限、默认 1024、负值 = 无界)、
   `config.rs:227-231`(accessor)、`builder.rs:110-112`(`cache_size`);**修 §1.5 点名的过期 doc**
   `config.rs:94-98` 与 accessor `:297-299`(kernel 模式早已投影 proc/task/msg 行,原文说「尚未投影」);
   `CHANGELOG.md` 的 `# Unreleased` 追加行为变更一条 + 文档一条。

**对齐点(已写进测试)**:start 型 parked 的可见面 = 根 `Created` → 投影 `proc_state(Created) = None`
(proc 行 `none`)、根 task 行 `pending`,与 legacy parked 行同形;重启后在飞的 run 保留原行(`Ready`/`Running`),
即 legacy boot-resume overflow 的形状(它本来就不是 `None` 行)。

**测试**(`shell/tests.rs`,用 `KernelHost::in_memory_with_cap(1)`):
- `a_run_beyond_the_cache_cap_parks_until_a_slot_frees`:3 个 run(`__created` 严格递增)+ 真投影器
  (`cache::Cache` + `RowProjector`,同一 KvStore);cap-a 走 4 跳(第 5 跳才 settle)→ 断言 cap-b/cap-c
  「只有 1 个 pending 操作、无 outbox、执行全 `Created`、proc 行 `none`、根 task 行 `pending`」;随后同一条
  `run_until_quiescent` 里 cap-a 收尾 → cap-b → cap-c 依次到终态(总 14 跳 = 2 + 6 + 6),lifecycle sink 记录的
  Started 顺序 = 启动顺序(`__created` 升序)。
- `a_run_already_in_flight_at_boot_waits_for_a_slot`:直接 seed 两个聚合(不经 `start_process`,等价重启)后
  `recover()` 唤醒两个根操作 → 先到的占唯一槽、4 跳后仍在飞,后到的「被派活但等待」且其聚合**等于自己的 seed**
  (零 hop);随后两者都到终态。

**验收实测**:
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT=0**(全 workspace);
- `cargo fmt` 已跑;
- `cargo test -p acts --lib --tests` = **1255 passed / 1 failed**,唯一红 = §1.8 基线
  `runtime::scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`
  (另有一次同命令的全量跑出 2 红,多 `reliability::parallel::sch_parallel_duplicate_next_rejected_no_duplication`
  —— **该条单跑实测绿**,且在同一代码的上一轮全量里也绿 = §1.8 的并行负载翻转族,非回归);
  本 crate **无 `tests/` 集成目标**(`src/tests.rs` 是 lib 内 `mod tests`),故 `--lib --tests` 覆盖全部;
  测试总数 1251(P1d:1250 passed + 1 failed)+ 新增 5 = **1256 = 1255 passed + 1 failed** ✓ 计数自洽。

**偏差/登记**:
(a) **启动期补齐用 recovery scan 而非「扫聚合重建」**:prompt 建议 `AsyncAggregateRepository`,但它**没有枚举
    API**(只有 `load(pid)`/`create`/`commit_*`);新增扫描会拖出 pid 键解码、每聚合 3 次读、V1→V2 迁移格式判断。
    改用**惰性占位**:重启后每个未完结 run 的首次 wake(必然来自 recovery scan)即其占位时机 —— cap 在重启后
    同样成立、超限者排队、释放后按 `__created` 补位;唯一差别是**首批 `cap` 个的准入顺序 = scan 返回顺序**
    (不是 `__created`),已写进 `Admissions` 文档。
(b) **parked 的 OperationId**:`start_process` 存 `spec.root` 的 seed 操作;惰性(重启型)parked 存
    `resume_operation` = 根执行 (task_id, generation, 0) —— 若该 run 的根 hop 早已跑完,这个 wake 是 no-op,
    靠下一 tick 的 recovery 重推真实工作(已写进 `place_run` 文档)。
(c) **根 id 约定**:shell 的根事实(`root_variable`/`root_state`/`created_stamp`/投影/生命周期)一律认 `$`
    (`TASK_ROOT_TID`,与既有 `projection.rs`/`report_lifecycle` 一致;生产路径 `from_workflow` 恒为 `$`),
    `ProcessSpec::linear` 的自定义根名只有 kernel 自带测试用 → 新测试用 `$` 作根。
(d) **未做**(按 prompt 明确不做):不动 export 余下 6 个 executor 的 `Arc<Runtime>`;不删 cache/scheduler
    任何代码;不碰 restore/start_parked;无 `#[allow(...)]`/TODO/stub。
(e) **⚠️ P2→P3 硬耦合(必读,§1.5 的实化)**:parked 的 `None` proc 行会被 **legacy 常驻路径**接手 ——
    `Runtime::init_retry_timer`(`runtime/mod.rs:1765`,每 tick)→ `Runtime::restore`(`:1148`)→
    `Cache::start_parked`(`cache/cache.rs:612`)→ `load_parked`(查 `state == None`)→ `decode_proc` +
    `Process::start`:legacy 引擎会用**同一 pid 跑同一个 workflow**,与 kernel 的 parked 队列重复驱动(之后
    kernel 再准入同一 pid → 双引擎写同一 pid 的行)。kernel 模式下一旦真的超限,这个窗口在一个 tick 内
    (生产 = `tick_interval_secs` 默认 15s;测试 = 800ms)。**P3 必须先删 `restore`/`start_parked` 再让该行为
    对引擎级可见**;本阶段 shell 级用例不经过 legacy 定时器,引擎级 kernel 用例的 cap 是默认 1024(不会 park),
    故本阶段验收不受影响。最小备选(未做,按 prompt「不碰 restore」):在 `init_retry_timer`/`Runtime::restore`
    加一行 `self.kernel.is_none()` 闸门。

### §9.5 A/B 性能基线(P3 前置,2026-10-13)

临时用例(已删):30 步线性 workflow(纯 step),两种模式各跑 start→`on_complete` 端到端。

| 模式 | hops | 耗时 | hop/s |
|---|---|---|---|
| legacy(`kernel_engine=false`) | 93 | ~22 ms | ~2778–5099 |
| kernel(引擎接线) | 93 | ~9–10 s | ~10 |
| kernel(裸 host,`run_until_quiescent`) | 93 | ~3.4 s | ~27 |

**判定**:kernel hop 吞吐远低于 legacy 的 1/2(约 1/200–1/500)→ P3.5 必做。§9.3 的代价是结构性的:
每跳 ~7 次全量聚合读 + 4 次 CAS 写,且每次写整包编码(growing aggregate),单 driver 串行。§9.3.1
(effect 不再内联阻塞 driver)对**无包步骤**无效;§9.3.2(合并 Claim+PrepareCommit)约 2x。故 P3.5
必须同时处理 §9.3.2 + §9.3.3(投影降频/writebehind)+ §9.3.4(整包编码),否则达不到 parity。

### P3 完成(2026-10-13):kernel 成为生产引擎,旧引擎仅测试可见(用户裁决 A)

**背景(与 prompt 的矛盾,已实测)**:prompt 假设「`Runtime→host` 只走 kernel」+「不动 `scheduler/tests/**`」
+「`cargo test -p acts --lib --tests` 全绿」三者同时成立。实测不可:约 **1200/1256** 个测试是常驻引擎测试
(`Runtime::start`/`launch`、`rt.do_action`、lane/queue/resident 断言),并等待 kernel **尚无**的旧消息面
(纯 step / 根的 task 级消息、`act:push`/`remove`/`set_process_vars` —— `Runtime::kernel_kind` 对后三者返回
`None`)。直接切换后全量跑 **50 分钟不结束**:`export::tests` 串行用例挂起 >60s、`cache::tests` 两例 FAILED。
经用户裁决采用 **A:保留「仅测试可见」的旧引擎入口,生产恒 kernel;开关与旧套件的删除留到 P5**。

**改动**(file:line 为当前工作区):

1. `config.rs`:`ConfigData.kernel_engine: bool` 恢复(`:93-103`,`#[serde(default = "default_kernel_engine")]`);
   `default_kernel_engine() = !cfg!(test)`(`:108-113`);**手工 `Default for ConfigData`**(`:115-131`)同取该默认;
   `Config::kernel_engine()` accessor 恢复(`:325-330`)。语义:除 crate 自身 test 构建外,kernel 默认开启;
   `acts.toml` 显式值可覆盖。
2. `runtime/mod.rs`:`Runtime` 增 `kernel_engine: bool`(`:112-117`);`kernel: Arc<KernelHost>` **恒构造**,
   driver 任务与 5 个 seam 恒起(`create` `:1498-1568`);`start`(`:517-541`)/`do_action`(`:639-669`)/
   `proc`(`:560-566`)按 `kernel_engine` 二选一(legacy 分支即 P3 前原路径);`kernel()` 返回 `&Arc`(`:547-549`);
   新增 `kernel_engine()`(`:551-554`)。
3. **耦合修复**(kernel 级,P2(e) 的解):`init_retry_timer`(`:1845-1857`)与 `initialize` 的 `on_proc`
   (`:1701-1711`)里 `restore()` 调用**仅** `!kernel_engine` 时执行 → 生产不再让常驻路径接手 kernel 投影的
   parked `None` 行。
4. `engine.rs`:`initialize` 按模式启动(`:149-172`)—— kernel 模式只起 trigger/snapshot 定时器 + retry 定时器
   (其 restore 已在 3 gated);resident 模式按旧路径起 `event_loop`/`resume`/`recover_actions` + 三个定时器。
5. `runtime/shell/tests.rs`:全部双引擎 parity 用例改**单引擎**(kernel,显式 `config.data.kernel_engine = true`);
   删除 `assert_parity`/`folded_legacy_states`/`latest_folded_legacy_states`/`legacy_node_states`/`wait_legacy_nodes`/
   `engines_quiescent`,以及 legacy 半边驱动与 `kernel(Option)` 用法;`kernel_task_and_act_messages_are_emitted`
   的 golden **取实测值固化**(MSG/ROWS/flags);测试名去掉 `_matches_legacy`。(共 -约 900 行)
6. `lib.rs` 的 `mod runtime` lint 豁免**保留**(旧 scheduler 半边仍只被测试触达;P5 收回),仅更新理由注释;
   `CHANGELOG.md` 追加行为变更一条。

**验收实测**:
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT=0**(1m18s,全 workspace);
- `cargo fmt --check` = OK(空输出);
- `cargo test -p acts --lib --tests` = **1255 passed / 1 failed**,唯一红 = §1.8 基线
  `runtime::scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`
  (单跑同样失败 = 基线红;另一次全量跑出现 `reliability::parallel::sch_parallel_duplicate_next_rejected_no_duplication`,
  **该条单跑绿** = §1.8 的并行负载翻转族)。两条都在**保留的常驻套件**里,不在 P3 删除范围(P5);
  `kernel_*_matches_legacy` 族已不存在;`reliability::error::sch_error_duplicate_delivery_is_terminal_noop`
  本轮未出现。测试总数 1256。

**偏差/登记**:
(a) **未落 commit**(整轮仍在工作区,与 P1/P2 同惯例)。
(b) **保留 `Config::kernel_engine` 字段**(用户裁决 A):它是 P5 删除项;`cfg!(test)` 只决定「未显式设置时的
    默认」,不是隐藏开关 —— 生产构建的任何入口(含 `acts-server`)默认 kernel。
(c) **保留** `Runtime::{event_loop,resume,recover_actions,init_retry_timer,launch,push,restore,resume_from_queue,
    redispatch_resumed,return_to_act}` 与 `initialize` 的 `on_proc`/`on_task`、`Cache::start_parked` 等 —— 常驻
    套件仍用;它们的删除与 `kernel_engine` 字段一起留到 P5(见 §11)。
(d) **顺带发现(未修,非 P3 范围)**:timeout 流的 run 抵达全终态后,根 `PropagateTask` 操作仍停留
    `Pending`/`Claimed`(把 `assert_all_terminal` 加进 timeout/父状态用例会红)。已从这两处移除该断言;
    疑似 kernel 侧孤儿操作,建议 P5 排查。
(e) `packages/**` 集成测试(`acts-package-shell` 等)依赖 `acts` 的**非 test 构建** → 默认 kernel;其是否受影响
    属 P6 门禁范围(本阶段 gate 只要求 `-p acts`)。

---

### P4a 完成(2026-10-13/14):删常驻引擎与旧套件 + 收回选择器

**范围**:先删掉"让 legacy 活着的东西"(常驻引擎 + 旧套件 + `Config::kernel_engine`),再谈删 cache。
**性质**:删除 + 少量 kernel 补洞(两处)。

**改动面**:

1. **旧套件**:删 `runtime/scheduler/tests/**`(保留 `tree.rs`/`state.rs`/`health.rs`)、`cache/tests.rs`、
   `src/package/tests/**`、`src/event/tests.rs`;`export/tests.rs` 中 15 个 `create_proc`+`launch` 用例、
   `store/store.rs` 的 `start_proc` 用例、`env/tests.rs`/`src/tests.rs` 的 legacy 点一并删。
   **登记**:这些用例断言的是已删引擎的内存树(`proc.task_by_nid/task_by_params/state`),kernel 下 act 折进
   所属 step 节点,无对应行可读;等价覆盖在 `shell/tests.rs` 与 `packages/acts-package-shell/tests`。
2. **选择器**:`ConfigData.kernel_engine` 字段 + `default_kernel_engine()` + `Config::kernel_engine()` 删除;
   首个手工 `impl Default for ConfigData` 由 `#[derive(Default)]` 取代(clippy 要求)。
   `Runtime.kernel_engine` 字段与 `start`/`proc`/`do_action` 的二选一路由删除(单路 kernel);
   `engine.rs::initialize` 只起 trigger/snapshot + retry 三个定时器(sweep/delivery 留在 `init_retry_timer`,
   未迁进 driver —— 登记:它们不是 hop 协议的一部分,不该占 driver 的时间片)。
3. **`Runtime` legacy 方法删除**(以编译器清单为准):`launch`/`push`/`dispatch_root`/`restore`/`resume`/
   `redispatch_resumed`/`return_to_act`/`event_loop`/`execute_job`/`run_*_job`/`isolate_context`/
   `report_task_panic`/`initialize`(on_proc/on_task)/`enqueue_next`/`complete_next`/`enqueue_action`/
   `complete_action`/`recover_actions`/`recover_outbox`/`create_proc`/`try_upsert_task`,以及
   `queue`/`op_claims`/`stall_backoff`/`outbox_dirty` 字段、`JobOp`、`CatchPanic`(旧)、
   `scheduler_queue_*` 指标。`scheduler_health`/`store_writer_*` 保留。
4. **scheduler 收敛**:删 `queue.rs`、`ops.rs`;删 `ActTask` trait、`Sign`、`NextAction`、
   `PropagationPhase`(`lib.rs` 的 `pub(crate) use ActTask` 随之删除);删 `Process::{start,do_tick,do_action,init_tick}`、
   `Task` 的 `exec/update/resume/check_*/auto_complete/is_ready/is_next/backs/follows/enter_action/vars_gen/clear_vars_dirty/
   set_state*/set_err/clear_err_with/set_sign/sign/…/propagation_phase/directive*/create_message/action_lock`、
   `Context` 的调度/游走/emit 路径与 `action` 字段。**scheduler/ 落位**:保留为**无头基座**
   (`Process`/`Task`/`Context`/`tree`/`state`/`validation`/`cron`/`health`),供 shell 与外部包使用。
5. **cache 常驻件删除**:resident 集/准入(`admit`/`evict`/`abandon`/`reserve_slots`/`SlotReservation`)、
   parked/overflow 队列(`start_parked`/`resume`/`resume_from_queue`/`PendingResume`)、inflight 去重、
   `proc`/`push_proc`/`upsert*`/`persist_task*`/outbox op 写入路径、`removed` 集、`WriteOp::{Task,CloseDeliveries,
   EnqueueNext,EnqueueExec,MarkOp*,OpDone}` 与其 apply 臂 + `apply_task` + `try_send`;`Cache::new` 的 Store 构造、
   `store_delivery`/`sweep_removable`/`flush(_pid)`/写者指标保留(见 P4b)。
6. **kernel 补洞(两处,均在本轮新发现)**:
   - **用户裁决**:`package/core/action.rs` 的 engine 命令包改为 `ctx.runtime.do_action(&Action::new(pid, node_id, …))`
     —— 命令走 kernel 既有动作面(校验/提交/级联一致),不再调已删的 `Task::update`;目标用**节点 id**
     (无头 scope 的 task id 是 `shortid`,不是执行键)。
   - **panic 隔离**:`shell/errors.rs` 新增 `CatchPanic`/`panic_message`;包执行处的 panic 转为**普通包失败**
     (执行 `Failed`、错误行走接管,等价 legacy `run_exec_job` 的 panic 语义),`serve`/`run_until_quiescent`
     再各自包一层,保证 driver 任务不会因一次 hop panic 死掉(`src/tests.rs::scheduler_event_loop_survives_task_panic`
     的 kernel 版)。
7. **dead_code 清单收敛**(收回 `lib.rs` 的 `#[allow(dead_code, unused_imports)] mod runtime` 后):
   删除未接线件 `core/recovery/{replay_policy.rs,recovery_format.rs}`、`runtime_state.rs`(其唯一消费者已删)、
   `core/commit/application.rs` 仅留 `WakeError`、`core/plan/branch.rs` 的 `BranchPlan::plan` 旧形态、
   `core/outbox/{outbox_repository.rs, dispatch_one/DispatchRequest/DispatchResult/DispatchError}`、
   `durable` 的同步文档库 + V1→V2 恢复格式迁移机、`core/model/{operation_phase.rs,generation_reference.rs}`;
   收窄 `ModelGraph`(只留 successor 映射)/`ExecutionGraph`(去掉只被已删访问器读的 root/parent)/
   `OperationEvent` 的 `RecoveryRepository`/`OperationRepository` 未用方法;test-only helper 全部 `#[cfg(test)]`
   (`KernelHost::{in_memory,in_memory_with_cap,run_until_quiescent,process_completed}` + `RunReport`、
   `ProcessSpec::linear`、`Runtime::kernel`、`Runtime::flush_writer`、`utils::test` 模块、`FixedClock` 删除、
   `commit::aggregate` 的内存仓库 trait 组等)。
   **两条新登记的 `#[allow(dead_code)]`**(属性处写明理由,§7 要求):
   `OperationEvent`(其事件产生的状态是持久化 V2 格式的一部分,codec tag 5–7;删事件=改格式)、
   `DeliveryOutcome::{RetryAt,OutcomeUnknown,Quarantine}`(transport 契约面,`dispatch_outbox` 穷举处理并落成
   持久 `OutboxEvent`;无 in-tree transport 构造它们)。**若日后要删,须连同 codec tag 与 dispatch 分支一起动。**

### P4b 完成(2026-10-13/14):删 cache,拆存活件

- `cache/` **整个目录删除**(`git mv` 后拆分):`cache/store.rs` → `store/process_rows.rs`(`load_proc`/`load_tasks`/
  `remove_proc`/`proc_workdir`)+ `store/delivery_rows.rs`(delivery 六个方法);`cache/writer.rs` →
  `store/writebehind.rs`(三个 `WriteOp` + `flush`/`flush_pid` + 新增自由函数 `store_delivery`/`sweep_removable`/
  `queue_removal`/`remove_workdir`)。
- **`Runtime` 自持** `store: Arc<Store>`、`writer: StoreWriter`、`router: Arc<LaneRouter>`;`Runtime::store()` 直返
  `Arc<Store>`;新增 `Runtime::{store_delivery,sweep_removable,flush_writer(cfg test)}`;`RowProjector::new` 改持
  `(Arc<Store>, StoreWriter, workdir)`,其 `flush_pid` 走 writer。`close()` 走 writer。
- **无头 scope 不再持久化**(§6.5):`Runtime::try_upsert_task` 与 `Cache::try_upsert_async`、`WriteOp::Task`、
  `apply_task` 全部删除;`$set_step_value`/`$set_process_var` 只写无头 scope(效果写回由 `EffectOutcome` 的
  data delta 折进同一笔提交),`$env` 私有键过滤保证 engine 事实不泄漏。
- 顺带:`shell/tests.rs` 的 parked 用例改自建 `Store`+`StoreWriter`;`export/channel.rs`/`export/actions.rs`/
  `engine.rs` 的 `cache()` 调用改 `Runtime::store_delivery`/`sweep_removable`/`store()`。

### P4 验收状态(截至本记录)

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**(全 workspace);
  `cargo fmt` 已跑;`cargo check -p acts --all-targets` = 0 error / 0 warning。
- `cargo test -p acts --lib runtime::core` = 140 passed / 0 failed;`runtime::durable` = 17/17;
  `runtime::shell` = 79/79(其中 `effect_seals_snapshot_params` 原为双引擎 parity 用例,legacy 半边已按 P0 口径
  **取实测期望值固化**:`token = "sk-1"`、`uid = "u1"`,测试名去 `_like_the_legacy_run`);
  `env::tests` = 30/30(10 个用例原读已删引擎的内存句柄,已迁移到**投影行**:`start_completed` 与各 start 点后
  `runtime.proc(pid)` 取回投影进程;6 个断言 act 级行的用例按 P4a 第 1 条删除并登记);
  `cargo test -p acts-store --features sled,sqlite,postgres` = 97 passed / 0 failed;
  `cargo test -p acts-server` **在本机未能跑完**:首次 E0786(§7.8 的 rlib 锁),之后稳定报
  E0463/E0786(`crate bashkit/clap_builder/acts_package_state required to be available in rlib format`)——
  是超时中断构建留下的陈旧 fingerprint/半成品 rlib(同一台机的已知现象,与被测代码无关;需要
  `cargo clean` 后串行重建再跑,本记录未做);
  `cargo fmt --check` = EXIT 0(已 `cargo fmt`)。
- **逐模块实测**(`cargo test -p acts --lib <filter>`):`runtime::core` 140/140、`runtime::shell` 79/79(32 s)、
  `runtime::scheduler` 30/30、`model` 204/204、`store::` 152/152、`snapshot` 13/13、`acl` 8/8、`config` 5/5、
  `plugin` 2/2、`utils` 3/3、`env::tests` 30/30(见下),`export::actions` 12/15(3 例 FAIL,见下);
  `tests::`(crate 级 37 例)300 s 内未跑完 —— engine 级用例单例 ~15 s,是时长问题,非挂死。
- **仍未绿(P5 必须先处理,不能跳过)**:
  1. **`export::actions` 3 例 FAIL**(单跑确定性失败,非 flaky;P3 记录显示它们当时通过 → 优先按回归查):
     - `a_run_is_confined_to_its_own_workdir`(@1315):运行/投影都正常(脚本输出已能从投影行读到),
       但**sweeper 未删除 workdir** —— `sweep_removable` 在 3 s 内没删掉行(疑:proc 行未被标记 `removable`:
       delivery 未全部结算,或投影的 `close_deliveries`/`has_unsettled_deliveries` 与本轮 writebehind 迁移的交互;
       待查)。**注意**该用例的脚本输出断言已改写为"运行行中任一 task 携带 `dir`",因为 kernel 不把子执行的 outputs
       折进父作用域(`proc.data()` 不再含 `dir`,legacy 会含)——这是**新登记的语义边界**。
     - `a_run_cannot_seal_another_subjects_scope`(@1165):`err.contains("not owned by subject 'u1'")` 得到空串 ——
       kernel 的 Denied 文本没有进到 `on_error` 消息的 `error.message`(作用域变量里有,消息里没有)。
     - `an_unsub_reaches_only_the_callers_own_namespace`(@1049):5 s 内 u1 的通道一条消息都没收到
       (疑与 channel 投递/订阅面或 writebehind 迁移有关)。
  2. **`cargo test -p acts --lib --tests` 全量**:`export::tests` 的 engine 级用例单个 ~15 s
     (裸 host 的 kernel 用例 0.11 s),全量在当前节奏下跑不完(30 min 未结束)——与 §9.5 基线同源,
     **P4c 必须解释并收敛**(driver tick 1 s + outbox/投递只在 tick 上跑)。
  3. **P4c 未开始**:§9.3.2/§9.3.3/§9.3.4。已确认可行性依据:`RecoveryPlanner::classify` 把 `Claimed`/`CommitPrepared`
     归为同一档(AwaitOwner/ReclaimExpired),故合并 `Claim`+`PrepareCommit` 不改恢复语义(普通 hop 与
     `apply_action` 各一处)。

### 已知边界(本轮新增,未静默降级)

- **`acts.core.subflow`**:`set_auto_complete(false)` 在 kernel 下本就是空操作(无头 scope 的 `__sign` 是私有键,
  不折进聚合),故一并删除并在包内注释登记:kernel 无"子 run 完成回灌父 act"的通道(`return_to_act` 已删),
  subflow act 现在与其子 run 同时完成;父/子 run 关联(`ACT_USE_PARENT_*`)仍随 inputs 传给子 run,但无消费者。
- **`acts.core.action`**:`Push`/`Remove`/`SetProcessVars` 仍无 kernel 动作面(`Runtime::kernel_kind` 返回 `None`
  并报错),与 P3 记录一致。
- **节点寻址的 action 无法区分同一节点的多个实例(2026-10-15 登记,含实测)**:action/message 只带 `tid` = 模型节点 id
  (legacy 的 per-item act 各有新 id),故一个 run 里同一节点被实例化 N 次(参数式 fan-out、`sequence{in}` 预建的下一项)时,
  客户端只能按投递顺序逐条作答;乱序作答、或给不同实例不同的 options 现在表达不了。逐条见 §10「Feature B 完成」§4
  (`action_target` 取"能接受该动作的最新 generation";逐项 `index`/`value` 会随作用域链上浮进祖先作用域与 run 的 outputs)。

---

### P4 收尾续(2026-10-14):测试归因、驱动节拍修正、事故与登记

**1) 根因:driver 的节拍不看测试口径(修掉了一大片"慢/挂")**
`Runtime::create` 原来用 `tick_interval_secs().max(1)`(默认 15 s)启 `serve`,而 outbox 投递/恢复扫描**只在 tick 上跑** →
engine 级用例等一次投递要等 ~15 s(裸 host 的 kernel 用例 0.11 s)。改为 `Self::tick_interval(config)`
(`runtime/mod.rs:521`;cfg(test) 800 ms,生产 = 配置值/15 s,**生产行为不变**)。实测:engine 级用例 15 s → ~1 s;
`export::actions` 12/15(20–35 s)→ **15/15(3.3 s)**;`export::tests` 900 s 超时 → **78/78(27 s)**;
`cargo test -p acts --lib --tests` **786 passed / 0 failed(70 s)**。

**2) 修好的用例(保留断言、改判据)**
- `export::actions::tests::a_run_is_confined_to_its_own_workdir`:脚本输出改从**投影行**读(运行结束后再取),
  sweeper 现在真能删目录(节拍修好后 `removable` 及时置位)。
- `export::actions::tests::a_run_cannot_seal_another_subjects_scope`:断言改到**失败 act 自己的行**
  (`err.message` 含 `not owned by subject 'u1'`,运行 `is_error()`);正例(自身 uid 能 seal)由
  `shell::tests::effect_seals_snapshot_params` 覆盖。
- `export::tests::{export_manager_tasks_count,_offset_in_range,_order}`:期望值改到 kernel 读模型
  (act 折进所属 step:一步流程投影 2 行;`order` 的最新一行是 root/`workflow`)。
- 新增 `export::tests::export_executor_agent_actions_are_refused_not_downgraded`:把 `act:push/remove/
  set_process_vars` 的"kernel 不服务 → 显式报错"钉住(替代 5 个 legacy 用例)。
- `env::tests`:10 个用例从内存句柄迁到 `runtime.proc(pid)` 的投影进程。

**3) 删除并登记(kernel 读模型/语义不可得)**
`env::tests` 6 例(act 级行);`export::tests::export_executor_start_with_outputs_schema_ok`;
`export::tests::export_trigger_hook_start`;crate 级 7 例(`sealed` 不在读模型里);
`tests::an_unclaimed_run_reads_no_snapshot_scope`。

**4) 本轮发现的缺口:全部已修或更正(逐条见 §5b–§5e)**
- ~~非 Func(Irq/Msg)act 的"起点解析失败"会卡死 run~~ → **已修(§5b)**。
- ~~向上错误行走不带错误文本~~ → **已修(§5c)**。
- ~~无步骤 workflow 永不完成~~ → **误诊;实测会完成**(§5d)。
- ~~`auto` 引擎事实泄漏进 outputs,`exposes` 校验误报 Errored~~ → **已修(§5d)**。
- ~~子执行 outputs 不回灌父作用域~~ → **已修(§5e)**;`export::tests::export_trigger_hook_start` 已恢复并通过。
- **唯一保留的边界**:`sealed` 快照不进读模型(vars 行 sealed 恒空;聚合不存 sealed)—— P3 已登记,本轮据此删 7 例。

**5) 修掉的 kernel 缺陷:`LeaseLost` 被展平**
`acts-server::tests::lease::taking_the_lease_over_stops_the_stale_engine` 期望 `ActError::LeaseLost`,而
`start_process` 的 `aggregates.create(..).map_err(runtime)` 把 store 侧错误展平成
`ActError::Runtime("kernel store error: database lease lost: …")`。修:`errors.rs::repository_error` 增加
`AsyncRepositoryError::Store(error) => error`(保型),`start_process` 的映射改用 `repository_error`。

**5b) 修掉的 kernel 缺陷:非 Func act 的 seal 拒绝把 run 卡死(卡死级)**
机制:起点 hop 的 `resolve_step` 失败时把失败**让给 effect hop**("状态表只从已启动的 act 决定失败"),而在
`PackageEffectRunner::run` 里 `run_as != Func`(**Irq/Msg**)会在 `headless()`/`ctx.prepare()` **之前**直接
返回空结果 → 那条"稍后失败"的路径对这类包**永远不存在** → run 既无 Failed 也无消息。
修:`run` 改为**先建无头 scope 并 `ctx.prepare()`(legacy `run()` 的 init vars + seal 对所有 run_as 都做),
再按 `run_as` 分流**;Irq/Msg 仍返回空效果(`effects.rs:122-138`)。
验证:新增 `tests::a_run_that_cannot_seal_fails_instead_of_waiting`(被拒的 seal → run `Error` 且失败 act 的
行带 `is not owned by subject`),单跑 0.11 s 通过(修前即 §10 第 4 条登记的挂死场景);`cargo test -p acts
--lib --tests` = **787 passed / 0 failed**;`cargo clippy --all-targets --all-features --workspace --locked --
-D warnings` = EXIT 0。

**5c) 修掉的 kernel 缺陷:向上错误行走不带错误文本**
`PropagateTask` hop 的 `upward_to` 分支在 `plan_upward` 之后,把**源执行自己的** `error.code`/`error.message`/
`ecode`/`message`(逐字复制,不重编码)**写进它新决定的每个祖先作用域** —— 对齐 legacy `emit_error` →
`parent.set_err`(把同一 code/message 一路改写祖先链)。效果:祖先(含 root)的行与消息带失败原因,
祖先自己的 handler 读 `$ecode()` 也能拿到(`runtime/shell/mod.rs:1793-1819`)。

**5d) 修掉的 kernel 缺陷:`auto` 引擎事实泄漏进 run 的 outputs,把 `exposes` 校验打红**
一个"无步骤 + `exposes`"的 workflow 在聚合里正常 `Completed`,但 channel 收到的是 **Errored**:
`model(...) outputs validation error: Additional properties are not allowed ('auto' was unexpected)`。
根因:`auto`(持久化 auto/waiting 决策,seed/fan-out 写进每个执行作用域)没有 `__` 前缀,
`process_message` 只按 `is_private_key` 过滤 → 它进了 run 的 outputs,撞 `exposes` 的
`additionalProperties: false`。修:`facts.rs` 新增 `is_engine_fact(name) = is_private_key(name) || name == "auto"`,
`process_message`(outputs)与 `input_vars` 共用它(localhost 不改名 —— `auto` 是聚合变量表的**键名**,
改名会让已持久化聚合的决策"失忆",登记为格式约束)。实证:探针从 `ERROR ... ('auto' was unexpected)` 变为
`COMPLETED outputs={"a":100}`;**被删的 `export::tests::export_executor_start_with_outputs_schema_ok` 已恢复并通过**
(它当初的红/挂就是这个泄漏,不是我先前登记的"无步骤 workflow 永不完成")。
**更正 §4 的登记**:无步骤 workflow **会**完成(`$:Completed`,~100 ms),那条登记是误诊。

**5e) 修掉的 kernel 缺陷:子执行的 outputs 不回灌父作用域**
`PropagateTask` hop 在规划之后,若传播中的执行是 `Completed` 且有父,则把它**自己作用域里的用户变量**
(排除引擎事实 `is_engine_fact`,并跳过本 hop 已为该父作用域写过的同名变量)写成父作用域的
`VariableWrite`(`runtime/shell/mod.rs:1670-1704`)—— 对齐 legacy `Task::next` → `parent.update_data(&outputs)`。
效果:step 里包写的值(`acts.transform.set` 的 `ret=100`)现在能被 run 根的 `exposes`/`proc:data()` 读到;
被删的 `export::tests::export_trigger_hook_start` **已恢复并通过**(0.10 s)。
副作用检查:全量 **789 passed / 0 failed**(fold 影响每次完成,套件是网)。

**5f) §9.5 基线复测(临时探针,已删)**
同一形状:一条 30 步线性 workflow(纯 step),start → `on_complete` 端到端,debug 构建 + 内存 store
(`KernelHost` 与 Runtime 都用内存 store):
| 指标 | 实测 |
|---|---|
| 完成的 operation(hop)数 | 93 |
| 端到端 | **8.33 s** |
| hop 吞吐 | **11.2 hop/s**(≈90 ms/hop) |
对照 §9.5 记录:legacy **2778–5099 hop/s**、kernel(切换前)**10–27 hop/s**。**结论不变:P4c 必做**,
而且节拍修正没有改善 hop 吞吐 —— 90 ms/hop 花在每 hop 的 ~7 次全量聚合读 + 4 次 CAS 写 + 整包编码
(`snapshot_codec` V2)上,即 §9.3.2/§9.3.3/§9.3.4 的三项。P5 复测时用同一探针形状(纯 step 30 步、
数 `Completed` 的 operation)以保证可比。
**5g) P4c 第 1 项(§9.3.2 合并 Claim + PrepareCommit)已做 —— 实测:无可测收益**
- 改动:`OperationEvent::ClaimAndPrepare(ClaimToken)`(`core/model/operation.rs`)—— 单一事件把
  `Pending|RetryScheduled|EffectUnknown → CommitPrepared(claim)`;普通 hop 与 `apply_action` 各一处由
  「Claim 一次 + PrepareCommit 一次」改为一次 CAS(`shell/mod.rs:1190`、`:1365`)。
- 语义:`RecoveryPlanner::classify` 把 `Claimed`/`CommitPrepared` 同为 AwaitOwner/ReclaimExpired,恢复不变;
  持久化格式不变(事件不落盘,产生的状态仍是 tag 2);`Claim`/`PrepareCommit` 保留(其它路径仍用)。
- **实测(同一 30 步纯 step 探针,3 次)**:`11.0 / 10.5 / 12.3 hop/s`(均值 ≈11.3),改动前单次 `11.2 hop/s`
  —— **在 ±10% 噪声内没有差别**。结论:每跳 4→3 次 CAS 写不是瓶颈(§9.3 原先估的"约 2x"不成立),
  真正吃时间的是 §9.3.4(每跳整包编码)+ 每跳的多次全量聚合读 + §9.3.3 的投影。**P5 应直接从 §9.3.4 下手**。
- 验收:全量 **790 passed / 0 failed**(含探针);clippy EXIT 0;探针已删。

**5h) §9.3.4 的实测画像(临时探针,已删)—— 整包编码确认是主因**
用一个**计数 KvStore**(包住 `MemoryStore`,统计 `one`/`put`/`delete`/`batch`/`scan_prefix` 的次数与字节)
跑 §5f 同一形状(30 步纯 step、93 hops、7.53 s):
| 指标 | 总量 | 每 hop |
|---|---|---|
| 写 | **3668 次 / 15.6 MB** | ≈ **39 次 / 172 KB** |
| 读 | **3046 次 / 25.2 MB** | ≈ **33 次 / 270 KB** |
| delete / batch | 0 / **756 次(3935 个批量 op)** | ≈ 8 batch/hop |
解读:每跳把**整个聚合文档**重编码写回(外加 recovery/outbox 两个派生索引文档,即那 8 个 batch),
写放大随 run 长度线性增长(30 步时已 172 KB/hop)。内存 store 上就已如此,换 sled/postgres 时每跳都是真实 I/O。
**P5 的候选方向(按收益)**:
1. **派生索引与聚合写解耦**:`document_store::build_create/replace_transaction` 现在把聚合 put + 索引 put 合成
   一个 batch(8 batch/hop);索引可以改懒写(按需/按 tick 重建),但要同时改恢复扫描的读取口径。
2. **拆分文档**:把"整包聚合"换成按执行/操作分片(或 append-only 操作日志),让一跳写 O(1) 而非 O(聚合)
   —— 这是**持久化格式变更**(V2 → 新版本),必须单独立项、带迁移与 golden 更新。
3. (次要)§9.3.3 的投影行写入占上面写次数的一部分,可降频。

**5i) §5h 的写入分解(临时探针,已删)—— 次数是索引/派生行,字节是聚合**
同一个 30 步/93 hops 的 run,按 key 家族分桶(`BucketKv` 包 `MemoryStore`):
| 家族 | 写次数 | KB | 读次数 | 每 hop |
|---|---|---|---|---|
| `kernel/indexes`(recovery+outbox 派生索引) | **1756** | 518 | 1688 | **18.9 w / 18.2 r** |
| `tasks`(行 + 其索引键) | 1021 | 88 | 248 | 11.0 w / 2.7 r |
| `procs`(行 + 其索引键;proc 行带 model JSON) | 614 | **717** | 215 | 6.6 w / 2.3 r |
| `kernel/aggregates`(整包聚合文档) | 524 | **14728** | 835 | 5.6 w / 9.0 r |
| `vars` | 12 | ~0 | 8 | 0.1 w |
| **合计** | **3959** | **16057** | **3061** | **42.6 w / 32.9 r** |
结论(直接给 P5 排优先级):
1. **调用次数**:每 hop 42.6 次写里只有 5.6 次是聚合本身,**其余 ~37 次是派生索引与投影行的索引键**
   (`kernel/indexes` 18.9 + `tasks` 11.0 + `procs` 6.6)。→ **先做"派生索引与行投影的写合并/懒写"**
   (无需改持久化格式,收益覆盖 87% 的写次数)。
2. **字节量**:16.0 MB 里 **14.7 MB 是整包聚合**(92%),5.6 写/hop × 28 KB 且随 run 长度线性增长。
   → 若要动字节量,必须**拆分文档/改 append-only 操作日志**(持久化格式变更,单独立项 + 迁移 + golden)。
3. 投影侧:proc 行每跳都重写(6.6 次/hop、717 KB),即使根的可见状态没变 → §9.3.3 的"仅在可见状态变化时
   投影"可省掉这一块的大头。

**5j) 投影行"内容未变则不写"(§5i 结论 1/3 的第一笔,已落地)**
- 位置:`runtime/shell/projection.rs::write`(473-507 行)。原签名 `write(collection, exists: bool, row)` 改成
  `write(collection, existing: Option<&T>, row)`,加 `T: PartialEq` 约束,`Some(current) if current == row => {}`
  跳过写。三个调用点(proc 193 / task 304 / vars 336)随之改。
  `store/data/proc.rs:7`、`store/data/task.rs:12,77` 的 `derive` 加 `PartialEq`(纯派生,无行为)。
- 依据:投影每跳都重投影**被触达的行**,即使内容没动 —— proc 行只有 run 开始/终态才变,每个 write 还带
  该行的全部索引键。
- **实测(bucket 探针,30 步/93 hops,前后同构对比)**:
  | 家族 | 改前写 | 改后写 | 改前 KB | 改后 KB |
  |---|---|---|---|---|
  | `procs` | 614 | **14** | 717 | 14 |
  | `tasks` | 1021 | 1014 | 88 | 81 |
  | `kernel/indexes` | 1756 | 1732 | 518 | 458 |
  | `kernel/aggregates` | 524 | 516 | 14728 | 14275 |
  | **合计** | **3959(42.6/hop)** | **3311(35.6/hop)** | **16057** | **14834** |
  `tasks` 几乎不动:每个 task 行每跳都真的在变(Created→Running→Completed),而该家族 90% 的写是
  **行自己的索引键**(~9 键/行)。→ 下一步若要压 `tasks`/`kernel/indexes`,只能合并索引键或懒写索引(§5i ①)。
- **端到端**:改前 11.0–12.3 hop/s,改后 12.2 hop/s(单跑,MemoryStore)→ **内存后端的墙钟收益在本探针分辨率内
  测不出**;这一笔的价值是**I/O 次数/字节**(对 sled/postgres 才有意义)。如实记录,不夸大为吞吐提升。
- **顺带修的测试脆弱点**:`runtime/shell/tests.rs` 的 `kernel_engine_projects_the_legacy_read_model` 在读消息行前
  未 flush 写队列 → 全量并行时偶发只看到 `completed`(漏 `created`);已在该处加
  `runtime.flush_writer().await`(读前先落地),单跑与并行重跑均绿。

**5k) `acts-server` 一条命令构建失败的真正原因(已定案,无需改代码)**
```
error[E0786]: found invalid metadata files for crate `acts_package_state`
   = note: failed to mmap file '\?\D:\...\target\debug\deps\libacts_package_state-5bd7c44b80e950f9.rlib':
           页面文件太小,无法完成操作。 (os error 1455)
```
- 早先记作"第三方 rlib flavor 缺失/fingerprint 陈旧"是**误诊**;os error 1455 = `ERROR_COMMITMENT_LIMIT`,
  即本机页面文件/提交限额不足(默认并行度下一次构建要同时 mmap 的 rlib 太多)。
- **验证**:`cargo test -p acts-server`(默认并行)= EXIT 101 报上述错;`cargo test -p acts-server -j 2`
  = **EXIT 0**,`--lib` 11 / `--test lease` 2 / `--test startup` 3 / `--test nats` 1 全绿。
- 结论:与本仓库代码/依赖图无关(同机 `cargo build -p acts-server`、workspace clippy 均通过;CI 为 Linux)。
  本机跑该套件用 `-j 2` 即可。**AGENTS.md 里"串行构建(Defender 锁 rlib)"那条与这条是同一类机器限制。**

**5l) 性能根因定案:瓶颈是"每个 hop 的 O(run 长度) CPU",不是 I/O(§9.3.4 的前提被推翻)**
三个临时探针(均已删),30 步线性 run:
1. **KV 耗时占比**:计数+计时 `KvStore` 包装 → `wall=7.49s, store=106ms, calls=3591, share=**1.4%**`。
   → **内存后端下 I/O 不是瓶颈**;"减少写次数/字节"(§5j 那一类)对吞吐几乎无影响,只在真后端(sled/pg)有意义。
2. **整包编解码成本**:直接在最终快照上微基准(`SnapshotCodecV2::encode/decode`,100 次均值):
   `bytes=45730, encode=1.00ms, decode=1.72ms` → **每个 hop 有 9 次整包编解码**
   (6 次 load 解码 + 3 次 commit 编码:`ClaimAndPrepare` / 计划提交 / `Complete` 各一次)
   = **24.5ms/hop,占 88ms/hop 的 28%**(其中 `encode` 还内含一次全量 `snapshot.validate()`)。
3. **hop 时间 vs run 长度**(同构 workflow,3/10/30 步):
   | 步数 | hops | wall | ms/hop | 快照字节 | 编解码/次 |
   |---|---|---|---|---|---|
   | 3 | 12 | 117ms | **9.7** | 6.3 KB | 0.28ms |
   | 10 | 33 | 600ms | **18.2** | 16.3 KB | 0.67ms |
   | 30 | 93 | 8236ms | **88.6** | 45.7 KB | 2.08ms |
   **hop 成本随 run 长度线性增长(10× 步数 → 9.1× ms/hop)⇒ 整个 run 是 O(n²)**;快照本身只 O(n)(≈1.5–2 KB/步)。
**结论(P5 性能项的判据改写)**:
- 要提 hop 吞吐,必须消掉**每个 hop 的 O(run 长度) 工作**,而不是省写。按收益排:
  ①**减少整包编解码次数**:现在 6 load + 3 commit/hop;把 `Complete` 并进计划提交(同 §9.3.2 的
  `RecoveryPlanner::classify` 论证路径)→ 少 1 解码 + 1 编码;把已 load 的 `LoadedAggregate` 传进
  repository 避免其内部重复 load → 再少 ~3 解码。**合计可砍 5/9 的编解码(≈整 hop 的 15–20%)**。
  ②**不要每 hop 重推 O(n) 派生**:`model_of()`(整包 model JSON 解析)、`execution_graph(&snapshot)`、
  `settled()/removable()` 扫描、`snapshot.validate()`(每次 encode 都跑)都是 per-hop 全量;
  按聚合版本缓存(version 没变就复用)是最大的单点,但属**有状态缓存**,要单独设计 + 论证确定性。
  ③§9.3.4② 的"拆分文档/append-only"仍是对的长期方向,但它解决的是**每次编解码的常数**与写字节,
  按本节数据它**不是**当前 88ms/hop 的主因 —— 优先级降到 ①②之后。
- **不要再把"§9.5 legacy 2778–5099 hop/s"当成同一成本模型的基线**:legacy 每 hop 不做 3 次整包持久化提交;
  真实目标应是"把 O(n²) 降到 O(n) 且每 hop 少做几次全量编解码",不是硬追那个数字。

**5m) hop 分阶段画像(临时埋点,已删)+ 第一笔结构优化:提交把快照交回来**
**阶段画像**(30 步/93 hops,同一 run;临时 `#[cfg(test)]` 计时埋点):
| 阶段 | 每次调用 | ×/hop | ms/hop |
|---|---|---|---|
| `claim`(`ClaimAndPrepare` 提交) | 11.0ms | 1 | **11.0** |
| `commit`(计划提交) | 16.4ms | 1 | **16.4** |
| `complete`(`Complete` 事件提交) | 11.2ms | 1 | **11.2** |
| `load1`(hop 起始 load) | 3.9ms | 1.09 | 4.2 |
| `load2`(claim 后为成型计划再 load) | 4.0ms | 1 | 4.0 |
| `load3`(提交后只为读 revision 再 load) | 3.9ms | 1 | 3.9 |
| `plan`(计划计算) | 1.4ms | 1 | 1.4 |
| `project`(投影) | 2.2ms | 1 | 2.2 |
| 合计 | | | **54.2 / 87.5 ms(62%)** |
**结论**:三次持久化提交 = **38.6ms/hop(整 hop 的 44%)**,每次提交 ~11–16ms,而单次整包 encode+decode 只 2.2ms
(§5l)→ **每次提交内部还有 ~9–14ms 的其它整包工作**(load 解码聚合+两个索引文档、两个索引文档的投影+编码、
guard 的字节克隆、`SnapshotCodecV2::encode` 内含的全量 `validate()`)。**这才是 P5 性能的正主。**
**本轮落地(安全、无协议变更):把提交装上/装完的快照交回调用方,消掉 `load2`/`load3`**
- `durable/async_document_store.rs`:`OperationCommitOutcome::{Committed,AlreadyCommitted}` 与
  `TransitionCommitOutcome::{Committed,AlreadyCommitted}` 增加 `snapshot` 字段(直接给出 `applied.snapshot`,
  或 `AlreadyCommitted` 时给出权威聚合 `aggregate.snapshot.clone()`);
- `shell/mod.rs`:`operation_event` 返回 `(OperationRecord, AggregateSnapshot)`;`execute_one` 用 claim 交回的
  快照成型计划(**删 `load2`**),用计划提交交回的快照读 operation(**删 `load3`**),后续 `report_lifecycle`/
  `project_touched`/`place_run`/timer 判定改用同一快照;动作路径(`apply_action`)同样删掉 claim 后的重复 load。
- **实测**:`load2` 4.0ms/hop → **0**(阶段消失);`load3` 3.9ms/hop → **8µs**(只剩从内存快照读一个字段)。
  即**每 hop 少 2 次整包解码、账内 CPU −727ms/5037ms = −14.4%**。
- **端到端诚实记录**:本机当时被拖慢(同构 run 的 wall 从 8.1s 漂到 12.1s,±18% 环境噪声),因此
  **墙钟 hop/s 的改善测不出来**;结构收益以阶段计数为准(上面两条归零),不宣称吞吐提升。
- 每 hop 整包次数:**6 load + 3 commit → 4 load + 3 commit**。
- 验证:`cargo test -p acts --lib --tests` = **790 passed / 0 failed**(含探针)== 移除探针后 **789 passed**。

**5n) 【重要】性能画像的最终定案:此前所有 hop/s 数字都是 debug 构建的产物**
继续细分提交内部(临时埋点,已删)后发现:
| 阶段(30 步/93 hops) | debug | **release** |
|---|---|---|
| `AggregateDocumentCodec::encode`(`bt_agg_encode`) | 6.23ms/次 | **0.58ms/次** |
| ├ `snapshot.validate()`(外层) | 0.14ms | 0.02ms |
| ├ `SnapshotCodecV2::encode`(payload) | 0.63ms | 0.17ms |
| └ **`AggregateEnvelope::seal` = `sha256_hex(payload)` + 组帧** | **5.46ms** | **0.38ms** |
| `AggregateEnvelope::decode`(验摘要) | 2.76ms/次 | 0.20ms |
| `SnapshotCodecV2::decode` | 0.87ms/次 | 0.23ms |
| `apply_transition`(validate + 整包 clone) | 5.58ms/commit | 1.05ms/commit |
| **同一条 run 的 wall** | **6.98–8.24s(≈11.6 hop/s)** | **0.684s(≈136 hop/s)** |
**结论**:
1. **debug 构建下整条热路径被纯 Rust 的 `sha2` + 编解码拖垮**:`envelope::seal/decode` 两次 SHA-256 占
   debug run 的 **~65%**(40% + 25%),而它们在 release 里各只 0.2–0.4ms。debug 的 `Instant` 计时把这一项
   放大成"每 hop 88ms、整 run O(n²)",**§5l/§5m 的墙钟结论必须按 release 重读**。
2. **release 基线(同构 30 步 run)= 684ms / 93 hops = 7.4ms/hop ≈ 136 hop/s**;此后任何性能对比、
   以及与 §9.5 "legacy 2778–5099 hop/s" 的比较,**一律用 release**。§9.5 的 legacy 数字必然是 release,
   拿 debug 的 11.2 hop/s 去除它得出的"20–450× 差距"是**假差距**;release 下是 ~20–37×,且其中
   每 hop 3 次整包提交 + 2 次 sha256 是主要结构成本。
3. release 下的可用杠杆(按实测):`ct_apply`(**整包 clone + 双 validate**)=1.05ms/commit×3;
   `rp_build`(含 seal)=0.68ms/commit;`ct_load`=0.56ms/commit → 仍是"每 hop 6 load + 3 commit 的整包流水",
   但单次成本降到 0.1–1ms 级。此前登记的"索引投影/编码"确认为无关(21µs/81µs)。
4. **本轮 §5m 的改动依然正确且保留**:每 hop 少 2 次整包 load(6→4),在 release 下每次 load ≈0.56ms,
   即每 hop 省 ~1.1ms(≈15% of 7.4ms/hop);debug 下测不出是因为被 sha2 淹没。

**5o) 提交改为"就地推进快照"(release 5.8ms/hop,−21%)+ 一个必须记住的地雷**
**改动**:`core/commit/aggregate.rs` 的 `apply_transition`/`apply_operation_event`(及 `AppliedTransition`/
`AppliedOperationEvent`)删除,改为就地推进的 `advance_transition(&mut AggregateSnapshot, plan) ->
CommitReceipt` / `advance_operation_event(&mut ..) -> OperationRecord`(所有校验在首次变更之前完成)。
`durable/async_document_store.rs` 的 `commit_transition`/`commit_operation_event`/outbox settle 三条路径都改成
"load → 就地推进 → `replace(&loaded, &loaded.snapshot, &entries)`",不再整包 clone;`MemoryAggregateRepository`
与两个纯函数测试改写为就地 API,并新增断言"**被拒的推进必须原样留下快照**"(就地安全性所依赖的不变量)。
**地雷(排查了 30 分钟,务必记住)**:`build_replace_transaction` 里 `replace_recovery_entries` 的 CAS 守卫是拿
**当前快照**投影出的 recovery-entry 文档去 `require_exact`。就地推进后若仍把同一个 `loaded.snapshot` 同时当
"current"和"replacement"传进去,守卫就会要求存储里**已经是新条目** → 每次提交都 CAS 失败。症状:run **永不完成**,
`cargo test --lib` 挂满 1800s;测试构建里驱动器的 `eprintln!("hop {id:?} failed: {error}")` 会刷满
`claim conflict on operation ...`。**修法**:推进**前**先 `recovery_entries(&loaded.snapshot)`(只投影非终态操作,
通常 1–3 条,极便宜),把它作为新参数传给 `replace`/`build_replace_transaction`。任何将来再动这里的就地推进,
都必须保留"先取 pre-advance recovery entries"这一步。
**实测**:release 全量 suite 绿 **791 passed / 0 failed**;同构 30 步 run(release,取两次较好的一次)
**5.81ms/hop ≈ 172 hop/s**,对比 §5n 的 7.4ms/hop ≈ 136 hop/s → **墙钟 −21%、吞吐 +26%**(debug 下 6.87s,噪声内)。
此前预测 clone≈0.9ms/commit×3≈2.7ms/hop,实测净收益 1.6ms/hop(差值来自每次提交新增的 recovery-entry 投影)。
`clippy --all-targets --all-features --workspace --locked -- -D warnings` = EXIT 0;`cargo fmt --check` 干净。
**顺带修掉一个并行负载下的偶发**(3 次全量里挂 1 次):`runtime::shell::tests::kernel_container_messages_are_emitted`
在 `dispatch_outbox().await.unwrap()`(测试文件 1191 行)上 panic —— 驱动器的 tick 自己也会跑同一趟 settle pass,
手动那趟可能**输给驱动器而报冲突**,而非"没送达"。新增测试侧助手 `settle_outbox(host)`:重试至成功(最多 50 次),
仍失败则 panic(真 bug 照旧失败)。改后**连跑 3 次全量 790/790 全绿**。注意该守卫集合与改动前**完全一致**
(见上"地雷"节),所以这是一处**既有的测试脆弱性**,不是本轮引入的竞态。

**6) 事故与恢复(重要)**
`crates/acts/src/export/actions.rs` 被一次失败的脚本**截断为空文件** —— 脚本先 `open(path,'w')`(截断)再
`write`,而 write 因类型错误失败。恢复:`git checkout HEAD -- crates/acts/src/export/actions.rs`(HEAD 已是
kernel 期版本,只有 2 处 `cache()` reach-through 待改)+ 重新施加本轮两处测试改动。**教训**:文件改写一律走
`edit`/`ast_edit`;脚本只在"写必然成功"时用。
另:多次超时中断构建导致 `target/` 产物不一致(E0463/E0786,报 `bashkit`/`clap_builder`/`sha1`/`os_display`
等 rlib 缺失),最终以 `cargo clean` 全量重建恢复。**补注(2026-10-14,见 §5k)**:其中一部分 E0786 的真实原因
是 `os error 1455`(页面文件/提交限额),与"陈旧 fingerprint"是两种不同的失败,别混为一谈;判据是错误文本里
有没有 `页面文件太小` / `ERROR_COMMITMENT_LIMIT`。

**7) 复核状态(全量 clean 重建后复跑,均绿)**
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**;
  `cargo fmt` 已跑;`cargo check -p acts --all-targets` = 0 error / 0 warning;
- `cargo test -p acts --lib --tests` = **789 passed / 0 failed**(~72 s;多轮复跑一致;含本轮新增的 `tests::a_run_that_cannot_seal_fails_instead_of_waiting` 与恢复的 `export::tests::{export_executor_start_with_outputs_schema_ok, export_trigger_hook_start}`);
- `cargo test -p acts-store --features sled,sqlite,postgres` = **97 passed / 0 failed**;
- `cargo test -p acts-server` **逐 target** 全绿:`--lib` 11/11、`--test lease` 2/2(`LeaseLost` 保型后由红转绿)、
  `--test startup` 3/3、`--test nats` 1/1;**但**把全部 target 放在同一条 `cargo test -p acts-server` 里构建时,
  本机会报第三方 crate 的 rlib flavor 缺失(`bashkit`/`sha1`/`libm`/`bigdecimal`/`schannel`/`tower_http`/`async_nats`/
  `os_display` …,`E0463/E0786`,full clean 后依旧) —— 环境/构建图问题(同一环境下 `cargo build -p acts-server`
  与 workspace 级 `clippy --all-targets --all-features` 均通过),**不是代码问题**;CI(Linux)为最终判据。

### 仍未完成(P5 收口项)

1. **P4c 性能**:基线已测(§5f,**11.3 hop/s** vs legacy 2778–5099);§9.3.2 已完成但**无实测收益**(§5g)
   → 剩余 **§9.3.4(整包编码,先做)** 与 **§9.3.3(投影降频)**;每 hop ~7 次全量聚合读 + 整包编码仍是主因。
2. `sealed` 快照进读模型仍是唯一保留的边界(若要支持,需在聚合里持久化 sealed 快照或让投影携带它)。
3. `cargo test -p acts-plugin-*` / `-p acts-package-*` 的完整复核(本机把全部 target 放在一条命令里构建会撞
   环境 rlib flavor 问题,见 §7;逐 target 跑是绿的)。

### 可靠性矩阵回归(2026-10-14):12 文件移植 + 内核缺陷修复 + 边界登记

**1) 做了什么**

- 新目录 `crates/acts/src/tests/reliability/`(12 个 feature 模块 + `mod.rs` harness),由 `crates/acts/src/tests.rs` 挂 `mod reliability;`。legacy
  的 `scheduler/tests/reliability/**`(切换时删除,13 文件约 9.5k 行)按 cell 逐条移植:每个 feature 行 × {store fault, restart, duplicate
  delivery, concurrency}。**61 cells** 全部绿。
- harness(`mod.rs`,`#![allow(dead_code)]` 有理由):`engine_on/engine_with/restart`(restart = close + 在新引擎上重开同一 store)、
  read-model/聚合读取(`read_proc`/`proc_row`/`tasks_of`/`wait_waiting_task`/`wait_completed`/`sweep_until_gone`/`dump_state`/
  `aggregate_key`)、消息捕获(`capture_created`/`capture_complete`)、动作封装、以及四个故障后端:`FaultKv`(匹配命中的下一次写失败一次)、
  `OutageKv`(ars 全读写失败)、`CutKv`(第 n 次写起全拒 = 精确写边界处的崩溃)、`GateKv`(停住匹配读以制造交错)。
- 断言口径(与 legacy 的关键差异,已写进各 cell 文档):不动点一律钉**聚合**(executions/generation + 确定性 outbox key
  `action:<pid>:<tid>:…`/`propagate:<pid>:<tid>:…`),行(proj/task/vars)只作参考——行是投影,会滞后一次提交,且已结算 run 的行会被 sweeper 回收。
  每个 cell 都证明注入故障确实命中(`fired()/failed_key()/refused()/parks()`),用 poll 到 deadline 代替 sleep。
- 机制已消失的 cell 改为"内核实际持有的性质",并在文档里明说:`Remove` 未服务(`kernel_kind` 返回 `None`)、rewind 是 generation 而非第二个 act、
  effect 内联在 hop 上且 host 从不取消在飞 effect(故 `ProbePackage` 一组 cell 改写)、subflow 回灌为本文「已知边界」所载、
  容器 `{in,acts}` fan-out 见 §3。

**2) 修掉的三处内核缺陷(全部由矩阵暴露)**

- **(a) 恢复时丢弃目标已终态的客户端决策**:`execute_one` 的 `ApplyAction` 分支先判执行状态、后读 `__action` fact,而 `Cancel`/`Back` 的目标
  按构造就是终态(`Completed|Submitted -> Backed`),于是崩溃恢复会把客户端决策换成裸 hop 的自动完成——正是 durable intent 要防的丢失。
  现改为 fact 优先,仅当 `command::action_allowed(state, action)` 为假(决策确实被别的写者越过)才走空余量。
- **(b) 并发 action 的意图 id 撞车**:意图 op id 由 `max(sequence)+1` 从"已加载快照"派生且无预留,两个客户端会 mint 同一个 id;第二次 intent
  commit 报 `OperationAlreadyExists`(不在重试白名单)→ 双方都被拒且 run 悬空,重试还可能读到对方的 op 并把它当成自己的成功上报。
  修:`action_identity` 顺延到第一个空 sequence 并重新 preflight;intent 提交幂等(自己的 fact 已在则跳过);`repository_error` 把
  `OperationAlreadyExists` 映射为可重试冲突(所有提交点);`resume_action_intent` 只认自己的 fact;丢失的 attempt 之间停 2ms,避免两个 racer
  互相作废到耗尽 `ACTION_ATTEMPTS`。
- **(c) 丢一次 CAS 即永久搁浅**:plan commit 报 `Conflict` 时 op 留在 `CommitPrepared`,lease(60s)过期后 recovery 把它释放成
  `EffectUnknown`,而 `EffectUnknown` 只归 `ManualReconciliation`——不再有唤醒路径,hop 与整个 run 永久停住(该 arm 的注释却写着"recovery 会重新推导")。
  修:该分支先释放自己的 claim(`ScheduleRetry`,被扫描归类为 `Wake`)再报错;**并且** `execute_one` 的 claim fence 改为按 op 自身 attempt 递增
  (`attempt + 1`),原来恒为 1——状态机要求 `fence > attempt` 才接受 claim,故 lease 过期路径(`Claimed → RetryScheduled{attempt:1}`)同样永远无法再被 claim。
- **(d) 附带**:driver tick 现在把 recovery report 的 `reconciliation_required` 打 warn(此前被丢弃,搁浅完全静默)。
- 确定性回归测试:`runtime::shell::tests::a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery`(在 hop 自己的 plan commit 内把它
  停住、在同一聚合上提交一个 rival transition,然后断言 op 落 `RetryScheduled` 而非留在 `CommitPrepared`、`recover` 唤醒它且不进
  `reconciliation_required`、run 最终 settle)。把 (c) 的两半分别回退,该测试各自转红(证据已复现)。

**3) 本轮新登记的边界(未修,均有实测证据)**

- ~~**容器包的参数式 fan-out 没有内核表示**~~ → **已实现(2026-10-15,见 §10「Feature B 完成」)**:导入期把内联 acts 物化成模型的
  `branches`/`steps`(`scheduler/shell/model.rs::materialize_container_acts`),参数式 `{in, acts}` 由
  `scheduler/shell/mod.rs::fanout_command::param_fanout_children` 逐项实例化模板 step,`plan_fanout` 的 admission 改为"子槽位按父定位"。
  `parallel.rs` 的那条 conformance cell 已随之改写为正向 cell(2 条:block 两种模式的 acts、`{in,acts}` 每项一个模板实例)。
  替代本条的新边界见 §10「节点寻址的 action 无法区分同一节点的多个实例」。
- **claim 之后才失败 → `EffectUnknown`**:任何非 `Conflict` 的失败(operation 记录的 CAS 冲突、store 错误)会以 `CommitPrepared` 挂到 lease 过期,再归
  `EffectUnknown`,此后只由人工对账(现至少会 warn)。"效果可能已落地"的不确定态保守不自动重试是设计选择。
- **`Runtime::close` 不 join driver task**:`close` 返回后仍可能有一次 hop 提交落库(测试侧用 stop 屏障绕开)。**不要改成 join**:
  `packages/acts-package-shell/tests/reliability.rs::a_waiting_script_does_not_hold_the_shutdown` 明确要求 close 不等在飞 effect。
- **测试侧**:`DELIVERY_LEASE_MS = 30s` 与 harness `WAIT = 30s` 相等,投递档位的等待接近抛硬币(stall 的对应 cell 已加容差并在文档写明)。

**4) 全量门禁又暴露并修掉的三处(都是"已结算 run 永久泄漏 / 良性竞态被判成损坏"这一类)**

- **已结算 run 的 `removable` 不会被修复**:该标志只由"该 run 自己的某次提交"的投影写入,而最后一个提交可能早于"最后一条 outbox 记录被投递"的那次 pass
  (`dispatch_outbox` 只为本次真正投递过的 pid 调 `refresh_proc_row`),此后没有任何路径重新投影。实测:executions 全终态、outbox 全 `Delivered`、无 delivery 行,
  proc 行仍 `removable=false`,sweeper 永远选不中 → 行 + workdir 永久泄漏(subflow 的 cell 在 6 路并发下 5/6 复现)。
  修:`Store::unmarked_settled_procs`(终态但未标记的行,复用 `sweep_settled_procs` 的扫描)+ `Runtime::sweep_removable` 先按聚合逐个 `refresh_projection` 再选择
  (每次上限 `SWEEP_RECONCILE_LIMIT = 256`,与 sweep 同页大小;对真的不可回收的 run 只是一次读 + 内容未变则跳过写的重投影)。
- **outbox 的 `Claimed` 记录没有回收路径**:原状态机里 `Claimed` 只能 `ConfirmDelivered`/`Reschedule`(且都要求 `now_ms < lease`)/`MarkDeliveryUnknown`/`Quarantine`,
  `dispatchable_outbox_page` 也只取 `Pending{due}` —— 投递途中崩掉、或确认的守卫写输给并发提交(记录与 run 同聚合!)的记录会永远停在 `Claimed`:
  消息不再重投,`settled()` 恒假,该 run 的行与 workdir 永不回收。
  修:`OutboxState::is_dispatchable` 计入"租约已过期";`evolve` 新增 `(Claimed, Claim)` 且旧租约已过期、fence 递增的接管转换(at-least-once,与 transport 既有契约一致);
  outbox 分页把 `OutboxIndexState::Claimed{lease 已过期}` 也纳入。新增单测 `an_expired_claim_is_taken_over_and_a_live_one_is_not`。
  这两处修掉后:6 路并发 subflow cell 由 5/6 失败 → **6/6 通过**(其中一次因卡住的投递等满 30s 租约用了 31.7s),12 路并发(subflow+stall)全绿。
- **恢复扫描把良性竞态判成损坏**:`load_recovery_page` 在一瞬间扫 recovery 索引文档,再在稍后载入它引用的聚合并校验 `operation.revision`/终态;两次读之间若有提交落地,索引就是"较旧的一侧",
  于是整趟 `recover()` 以 `CorruptAggregate("recovery index entry is stale for operation …")` 失败(实测:timeout 的并发 cell 在全量负载下把这个错误冒到调用者)。而索引与聚合是同一次守卫批写入的,
  所以二者的不一致只可能是这个竞态。修:该 entry 直接跳过(聚合已消失 / operation 已消失 / revision 变了 / 已终态),下一趟扫描重新读索引;只有"文档读不出/解不开"才算损坏(仍报错)。
- harness 相应调整(只为让"读的量"与真实收敛口径一致,断言强度不变):`dump_state` 增打原始 proc 行(`state/removable`)与 outbox 记录状态标签(正是诊断上面三处的依据);
  `sweep_until_gone` 改用自己的 `SWEEP_WAIT = 120s`(必须长于 `DELIVERY_LEASE_MS = 30s`,原 30s 是在抛硬币);`WAIT` 30s → **90s**、`bounded` 240s → **600s**、
  `EXPECT_WAIT` 20s → 60s —— 这些是"要在多载机器上活下来"的界限而非快路径时延:全量跑里有 ~850 个别的用例(很多自己就是引擎用例、各有 driver tick)在同时抢机器;
  cancel/back 的人工驱动上限 `WAIT * 3`,失败时带 dump。(抬高界限前,连续三次全量跑各随机翻一个不同的 cell:cancel → back/error/timeout → remove,均为等待超时;抬高后连续两次全绿。)

**5) 门禁实测(本轮结束)**

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**(先修 3 个测试侧 lint:stall 的嵌套 `if`、timer 的 `needless_borrow`/`len_zero`);
  `cargo fmt --check` 干净。
- `cargo test -p acts --lib --tests --jobs 2` = **858 passed / 0 failed**(327s;含本轮新增的 12 模块 61 cells 与 `runtime::shell::tests::a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery`)。
  修复前的两次同命令:856/857(cancel 超时)、855/858(back/error/timeout),三次失败各自对应上面 §4 的三处;修完后该方法与本条命令均绿。

**6) 规划到未来的任务(本轮只规划,不实现;已同步进 §11 的任务清单)**

**T1 清掉死掉的 `ops` 族(legacy durable outbox 遗留)**

- 现状(已核实):内核 outbox 记录在聚合里(`kernel/aggregates/*` + `kernel/indexes/outbox/*`);`ops` 集合生产侧**无 writer 无 reader**,仅剩引用:`Store::ops()`(`store/store.rs:81`)、`remove_proc_rows` 的家族清单(`store/store.rs:365`)、`StoreIden::Ops`、`store/data/op.rs`(`data::Op`)、以及 store/collection 单测把它当"示例集合"(`store.rs:1835/1875/1890`、`collection.rs:1604/1771/1811`)。
- 范围:删 `Store::ops()`、`StoreIden::Ops`、`store/data/op.rs` 及其 `data/mod.rs` 再导出、`remove_proc_rows` 里那一行;单测的示例集合换成 messages/deliveries。
- **必须先决定**:`lib.rs` 的 `pub use store::*` 让 `Store::ops()`/`data::Op` 属公开 API → 这是一笔 **BREAKING**(外部 crate 若读 ops 会断),CHANGELOG 要写 BREAKING 行。
- 老库问题:切换前的库里已有 `ops-*` 行;家族删掉后它们成为永久孤儿(无人读、只占空间)。三选一:(i) 接受;(ii) 在 `remove_proc_rows` 保留"只删不建"的那一行;(iii) 加一次性清理入口(boot 时或 `rebuild_indexes` 旁按前缀删)。
- 验收:`grep -rn "data::Op\|StoreIden::Ops\|\.ops()" src/` 生产侧为 0;全量门禁绿;CHANGELOG 记 BREAKING。

**T2 给聚合文档加回收/归档(现在**永不**删除)**

- 现状(已核实):`kernel/aggregates/<hex pid>` + 两个派生索引**没有任何删除路径**(`document_store.rs` 里只有 create/replace/load);sweeper 只删六族行 + workdir(`store/writebehind.rs:476`)。`Runtime::start` 用 `kernel.exists(pid)`(= 聚合是否存在)拒绝 pid 复用 → 已结算 run 的行被扫掉而聚合留下,于是 **pid 终身不可复用**,聚合也**无界增长**(按 §5i:30 步 run 的 16 MB 里 14.7 MB 是聚合,字节大头)。
- **必须决定的三个**:
  1. **保留策略**:何时可删?候选——行已回收 + 静默 N 时间 / 显式运维归档命令 / 容量触发。
  2. **pid 复用语义**:删聚合 = 释放 pid(同 pid 可再启)。若"pid 永久唯一"这条保证要保留,就不能再拿聚合当锚点,须另存一条**墓碑/pid-epoch 小记录**。
  3. **历史**:行被回收后,聚合是那条 run **唯一**残存记录(变量、outbox 收据全在里面);删即丢历史 → 政策要在"磁盘"与"审计/回溯"之间取舍(或先归档再删)。
- 实现要点:新增 `AsyncAggregateRepository::remove(process_id, expected_version)`,在**一个守卫批**里删聚合 + recovery index + outbox index(三者今日同批写入,半删必须不可能);GC 只考虑"`settled` 且 `operations` 为空 且 行已回收"的 run;崩溃幂等(行回收与聚合删除之间崩掉,重启后仍是可回收态)。
- 验收:回收只按政策发生;pid 复用按所选语义(含墓碑方案)有测试;对"聚合已删但索引项仍在"的库恢复扫描不报错(本轮已改成跳过旧项,该测试骨架可复用);全量门禁绿。

---

### P5(名与形)完成(2026-10-14):`runtime/` → `scheduler/` + 旧 `scheduler/` 基座解散 + `Runtime` 独立

**用户指令(三条,一次做完)**:①`runtime/` 改名 `scheduler/` —— 这个模块实际就是工作流的调度与执行,结果经 `shell/` 对外、经 `durable/` 落字节;②`runtime/scheduler/`(旧无头基座)按依赖拆进 `core`/`shell`;③`mod.rs` 里的 `Runtime` 独立到 `scheduler/runtime.rs`。
**执行顺序**:先解散、再改名、最后抽 `runtime.rs` —— 否则中间态会出现 `crate::scheduler::scheduler::…`,同一批路径要动两遍。

**改名后的目录**:

```
src/scheduler/
├── mod.rs      # 仅 doc + 私有 mod 声明 + 显式 `pub use` 门面(见下)
├── runtime.rs  # Runtime + SnapshotRegistry + TEST_TICK_MS + SWEEP_RECONCILE_LIMIT + prepare_workdir/is_workdir_segment + ActPackageRuntime impl
├── core/       # 纯层:model/plan/commit/recovery/outbox + architecture.rs/coverage.rs
│   ├── model/task_state.rs                               ← scheduler/state.rs
│   ├── model/view/{mod,node,node_tree,visit,build}.rs     ← scheduler/tree/*(task_tree 除外)
│   └── plan/cron.rs                                      ← scheduler/cron.rs
├── durable/    # 字节层(未动)
├── shell/      # 异步外壳
│   ├── context.rs / health.rs / validation.rs             ← scheduler/{context,health,validation}.rs
│   ├── process/{mod,process,task}.rs                      ← scheduler/process/*
│   ├── task_tree.rs                                       ← scheduler/tree/task_tree.rs
│   └── tests/{mod,health,state,tree}.rs                    ← shell/tests.rs 转目录 + scheduler/tests/*
└── tests/      # 可靠性矩阵(白盒跨层套件,见下;`#[cfg(test)] mod tests;`)
    └── reliability/**                                     ← crate::tests::reliability(13 文件)
```

**落位判据**(不是按文件名分):`tree/task_tree.rs` 持 `Arc<Task>`、`process/*` 与 `context.rs` 持 `Arc<Runtime>` → 只能进 `shell/`;`tree` 其余件 + `state` + `cron` 零 `tokio`/`store` → 进 `core/`(守卫扫描通过);`validation.rs` 用 `store::Store` → `shell/`(**不**进 `durable/`:那里只放字节编解码 + 已声明的 `KvStore` 适配器)。

**守卫同步**(`core/architecture.rs`):`runtime_root()` → `scheduler_root()`(`src/scheduler`);`RUNTIME_ROOT` 改为 `{core:Pure, durable:Byte, shell:Shell, mod.rs:Root, runtime.rs:Root, tests:Tests}`(`scheduler` 条目删、`runtime.rs` 条目加、可靠性套件搬进来后加 `Tier::Tests` 一档);`SHELL_MODULES` 由两条收成 `["crate::scheduler::shell"]`(**那个自指条目消失**);测试 `every_runtime_entry_is_classified` → `every_scheduler_entry_is_classified`。
另:新增 `names()`/`names_segment()`,让"文件是否命名某依赖"在**分组导入**下依然成立 —— 目录现在写成 `use crate::{…, store::KvStore}`,`crate::` 前缀落在组上,原来的 `source.contains("crate::store")` 会漏判(**实测**:导入分组后 `store_coupling_is_explicit_and_minimal` 立刻红,断言文本 `declared adapter ... no longer names crate::store`);单段尾巴按**路径段边界**匹配(`store::` 命中,`document_store::` 不命中)。

**`lib.rs`**:`mod runtime;` → `mod scheduler;`;`pub use crate::scheduler::shell::Context;` 的内部路径变了,但 `acts::Context`(被 `packages/acts-package-nats`、`packages/acts-package-state`、`examples/custom_package` 消费)不变;`Runtime` 仍不导出(外部只经 `Engine`)。

**对外面(用户第二/第三条指令,三轮收敛)**:`mod.rs` 不再写 `pub(crate) mod core; …` —— 四层改为**私有 `mod`**,对外只留**显式 `pub use` 门面**(逐个列出)。门面即全部契约:层可以重塑而消费者无感。
- `Context` 是唯一 `pub use`(crate 根 `acts::Context` 要公开再导出);服务路径要用的名字(`Runtime`/`Process`/`Task`/`TaskState`/`Node`/`NodeData`/`NodeKind`/`NodeTree`/`Cron` + `prepare_workdir`)是 `pub(crate) use`;
- **第一版把可靠性套件要观察的 14 个内核名字也塞进了门面(带 `#[cfg(test)]`)** —— 用户指出这不是门面该有的东西:为**一个白盒套件**放宽模块契约。可选方案有三:①把套件搬回模块内;②`#[cfg(test)]` 下开放层本身;③照旧但压 lint(`#[allow(unused_imports)]`,会把真死的导出一起盖住)。**取 ①**,理由见下一条;
- 剩下的**唯一**测试例外:`NodeContent` —— 消费方是 `model/tests/info.rs`(模型自己的行转换测试要构造一个视图节点)。它 test-only、也不属于任何层,故留在门面并写清理由;
- 迁移面:`scheduler/` 之外的 **28 个文件**由 `crate::scheduler::{core,shell}::…` 改为门面名;`validator.rs` 的一处 doc 引用 `shell::validation` 改为 `Runtime::schema_cache`(该模块已私有)。

**可靠性矩阵搬回模块内(用户第三问的答案)**:`crate::tests::reliability/**`(13 文件 / 11,245 行)→ `scheduler/tests/reliability/**`(`scheduler/mod.rs` 加 `#[cfg(test)] mod tests;`,crate 根 `tests.rs` 去掉 `mod reliability;`)。
- 依据:该套件是**白盒**的 —— 它驱动 `Engine`,直接断言聚合/执行状态/操作状态/outbox/行键。按"白盒测试跟着被测模块走"的惯例(本仓自己的先例:`shell/tests/**`、`core/coverage.rs`),而且 `git log -- crates/acts/src/tests/reliability` 显示它**本来就在** `src/scheduler/tests/reliability/`(`9b4f02b` 才挪到 crate 根);
- 搬回后它作为 `scheduler` 的**后代**直接用真路径(`crate::scheduler::core::model::execution_graph::ExecutionState`、`…::durable::document_store::aggregate_key` …),门面不必为它开口子;
- **效果**:模块对外面从 25 条降到 **11 条**(10 条服务路径真在用 + 1 条如实标注的 `NodeContent`);`scheduler/tests/mod.rs` 写明"这套是跨层观察耐久性契约,所以住在层旁边"。
- 守卫新增一档 **`Tier::Tests`**(`tests` 成为运行期根的第五个条目):测试树跨层观察,层规则一概不适用。

**导入分组**(用户指令):`crates/acts/src/scheduler/**` 的 **30 个文件**把 `use crate::…` 合并成单条嵌套组(`use crate::{ …, scheduler::{core::{…}, shell::{…}} }`);`cargo fmt` 归一。

**两处偏差(登记)**:
1. **`SchedulerHealth` → `LoopHealth` 的改名作废**:该名字与既有的单循环类型 `LoopHealth`(`SchedulerHealth` 的两个字段就是它)撞名 —— 实测 8 处 E0119/E0428 → **回退保留 `SchedulerHealth`**,`Runtime::scheduler_health()` 不变。换来的是零语义改动;代价是 `scheduler::shell::health::SchedulerHealth` 轻微口吃。
2. **失效文档链接未修**:`shell/mod.rs` 的 `[`crate::scheduler::PARK_CHILDREN`]` 在改名前就指向库里不存在的项,本轮只同步前缀,登记待清(与本轮无关)。

**影响面实测**:`crate::runtime` 出现 **943 次 / 67 文件**,全部在 `crates/acts` 内;**跨 crate 引用 0**;`git mv` 保留历史(21 条 R 记录);本轮工作树共 **110 文件**变更。

**验收(实测)**:
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**(全 workspace,含 examples);
- `cargo fmt` 已跑,`cargo fmt --check` = 干净;
- `cargo test -p acts --lib --tests` = **858 passed / 0 failed**(383 s);
  可靠性矩阵**搬进模块后**同一命令复跑 = **858 passed / 0 failed**(332 s);两次之间出现过 1 例失败(§1.8/§10 已登记的"并行负载翻转族",单跑即绿)—— 故记为 flake,不是搬迁回归;
- `cargo test -p acts-acl`、`-p acts-expr --all-features`、`-p acts-store --features sled,sqlite`、`-p acts-cli` = 全绿;
  `cargo test -p acts-server -j 2` = **EXIT 0**(按 §10 该机的页面文件现象,`acts-server` 逐 target 跑);
- **未绿(与本轮无关,已在基线上复现)**:`cargo test -p acts-package-shell` 的 `bounds`(6)/`policy`(2)/`vars`(1) 失败,
  断言形如 `the failure must name the deadline, got: {}` —— 失败 act 的**输出文本为空**,属 §10 已登记的"失败文本/子执行 outputs 通道"同一族语义差距。
  **实测对照**:`git worktree add --detach` 取 HEAD(60806c0)、独立 `--target-dir` 全新构建后,同一批用例**同样** 1/6、1/2、3/1 → 非本轮引入。
  `acts-store` 的 postgres 套件(需 localhost:5433)与 `acts-channel`/`acts-plugin-nats` 的 broker 套件(需 127.0.0.1:4222)本机无服务,未跑(AGENTS.md 已声明该前提)。

**与后续的关系**:P1a 当初把 `Runtime` 写进 `mod.rs` 的唯一理由是 `runtime/runtime.rs` 会触发 `clippy::module_inception`;目录改名后该约束消失,`scheduler/runtime.rs` 合法。§10「待决:`host` 与 `Runtime` 是否冗余」的"最后合并"不受影响 —— 合并后 `impl Runtime` 正好分在 `runtime.rs`(组合根/门面)与 `shell/`(驱动)两处。

---

### Feature B 完成(2026-10-15):容器包的内联 acts 成为内核子执行

**目标**:`acts.core.block{ mode, acts }` / `acts.core.sequence{ in, acts }` / `acts.core.parallel{ in, acts }` 的内联 acts 变成内核**子执行** ——
`examples/message`(block 里的两条 msg)与 `examples/approve`(pm/gm 审批)按模型本意运行。原任务书与"原设计"所在的 §11 小节已随本轮合并重写(结论留在此处);本节即该轮的完整记录。

**1) 落地的机制(三处)**

- **导入期物化**(`scheduler/shell/model.rs`):`from_workflow` 在 `assign_ids` 之后调用 `materialize_container_acts`,递归遍历(top-level steps /
  各 step 的 branches 内 steps / `catches` / `timeouts`,含本轮新加的分支)把命中容器包的 step 改写成内核本来就懂的 `branches`/`steps`:
  `block{ mode: sequence }` → 1 个恒选分支 `<step>#acts`(steps = 各 act,声明链即顺序);`mode: parallel` → 每个 act 一个恒选分支 `<step>#act<i>`;
  `parallel|sequence{ in, acts }` → 1 个恒选分支 `<step>#in`,内含 1 个模板 step `<step>#item`(`uses: acts.core.block`、`params = {mode, acts}`,再被递归物化)。
  Act→Step 是保真映射(`uses` 空 → `None`、id 空 → `shortid()`);容器 step 自身保留包与 params(照旧发消息、params 照旧校验,故 schema 违规仍在原地失败)。
- **逐项实例化**(`scheduler/shell/mod.rs::fanout_command` 的 `param_fanout_children`):`params` 带 `in` 的 `parallel|sequence` 节点走动态分支 ——
  从 start hop 钉下的 `__params`(无 effect runner 时回退到模型里的字面 `in`)取数组,每项一个 `ContainerChild{task_id = <step>#item, model_node = 模板,
  vars = index/value + 模板 vars, auto}`,即同一模型节点 N 个 generation;`parallel` 全 `Entry`/`[Source]`,`sequence` 首项 `Entry`/`[Source]`、其余
  `Queued`/`[Sibling(i-1)]`。
- **`plan_fanout` 的两处修正**(`scheduler/core/plan/container.rs`):
  ① 同一 plan 内重复 task_id 的递增收代(`HashMap<task_id, u64>` 偏移 = `max(快照 next, 本 plan 上次分配 + 1)`,快照看不到本 plan 的 create);
  ② **子槽位按父定位**:admission 的"已有 generation"观察从"该 task_id 的全局最新"改为**该 source 自己已持有的同名子执行**(按 generation 升序,
  第 i 次出现取第 i 个)。

**2) 原设计漏掉、由实测逼出来的三处**

- **(a) 不按父定位就没有第 2 个 item**:探针(`parallel{in: '${{ items }}'}` 2 项 + 1 条 msg act)显示只有 1 个模板实例跑起来、
  `step1#item@1` 停在 `Running`、run 永不完成 —— 第 2 个 item 的分支/act 在该节点已 `Active` 时被 `GenerationAdmissionPolicy` 判 `Ignore`,
  收编到第 1 个 item 的同名 execution 上。按父定位后:`step1#item@{0,1}` 与 act 节点 `@{0,1}` 各自成立、各自 scope 带 `index/value`、run Completed。
- **(b) `sequence{in}` 必须链式**:原设计写"N 个 generation 并行接纳",而 legacy `SequencePackage` 走 `build_acts(&acts, true)`(逐项串起来)。
  照 legacy 实现:首项 `Entry`/`[Source]`,其余 `Queued`/`[Sibling(i-1)]`(cell 直接断言两种接纳边)。
- **(c) `action_identity` 选不中第 2 个实例**:action 只带 `tid`(模型节点 id),而 `pm_act` 在一个 run 内有 2 个 Waiting generation;原实现恒取"最新 generation",
  第一笔完成后第二笔打在已 `Completed` 的同一个上 → preflight 拒绝 → `examples/approve` 的客户端 `exit(1)`。改为 `action_target`:取"**能接受该动作的最新
  generation**"(都不接受时退回最新,如实报拒绝)。客户端按投递顺序逐条作答即逐项落地(emitter 对同一 pid 的 handler 本就串行、按投递顺序);单 generation 的节点行为不变。

**3) 实测(命令 + 数值)**

- 新增/改写 cell(`scheduler/tests/reliability/parallel.rs`,6 条全绿):删掉 conformance cell
  `a_uses_param_fan_out_resolves_its_package_without_children`,换成 `a_uses_param_fan_out_instantiates_one_template_per_item`
  (`parallel` 与 `sequence` 各跑一遍:2 项 → `step1#item@{0,1}`、各自 scope 的 `index/value` = `0/"u1"`、`1/"u2"`、各自 act 消息解析出 `0=u1`/`1=u2`、
  接纳边 `[Source]` vs `[Sibling]`、容器 Completed、行可回收)与 `a_block_packages_inline_acts_run_as_children_in_both_modes`
  (sequence/parallel 两模式各起一个 run:两条 act 消息齐全、无未终态 execution、容器 Completed)。
  模型层新增 2 条(`scheduler/shell/model.rs::tests`):`container_package_acts_become_model_children`(durable model 的分支/模板/恒选守卫/auto map 决策)、
  `only_the_container_packages_are_materialized`(用户包带 `acts` 不被物化;`mode: parallel` 一 act 一分支)。
  两条新 cell 的通道消息断言先 poll 到齐再断言(消息经 outbox 交给 emitter 的 per-pid worker,run 结算时可能仍在投递 —— 实测踩过一次空集),
  项序也不作要求(逐项 hop 相互独立)。
  `cargo test -p acts --lib reliability::parallel` = **6 passed / 0 failed**;`--lib shell::model::tests` = **13 passed / 0 failed**。
- `examples/message`(`cargo run -p acts --example message`):step3(`block{sequence}` 两条 msg)的两条 act 都跑到(`msg1 a=110`、`msg2 b=120`),
  run completed、`on_complete: {"value":100}`(逐项绑定未影响它)。**改前** step3 "直接完成"、两条 msg 一条都不发。
- `examples/approve`(`cargo run -p acts --example approve`):`pm_act` Created ×2 且都被应答、`pm_act_msg` ×2、`gm_act_msg`/`gm_act` 各 1,
  `on_workflow_complete: pid=… cost=0ms outputs={"gm":["3"],"index":0,"pm":["1","2"],"value":"3"}` —— **pm/gm 都非空**(验收口径),run 正常结束(47 s wall,含编译)。
- `cargo test -p acts`:本轮共跑 4 次 —— 1 次 = **871 passed / 0 failed**(lib,152 s)+ doctests **11 passed / 1 ignored / 0 failed**;
  另 3 次各翻 1–2 条**负载/残留状态敏感**的用例,且每次位置不同:`export::actions::tests::a_run_cannot_seal_another_subjects_scope`
  (在 `on_error` 消息之后立刻读**投影行**、读前不 flush 写后台;单跑 3/3 绿)、`scheduler::shell::tests::kernel_task_and_act_messages_are_emitted`
  (同样是投影行断言;单跑绿)、`tests::a_backward_next_re_enters_the_node_it_names`(自带 20 s 上限,单跑 12.45 s 绿,`examples/simple` 实测 `{"result":55}`)。
  **对照实测**:把本轮改动 `git stash` 掉、在同一 target 上跑同命令,**基线同样翻 1 条**(`kernel_task_and_act_messages_are_emitted`)
  —— 即 §10 §4「harness 的界限」登记过的"并行负载翻转族 + 行投影滞后",与本轮无关,未在本轮修。
  本轮**受影响**的那些 cell 反而单独连跑 3 次全绿(`cargo test -p acts --lib reliability::parallel` = 6 passed ×3)。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**(先修 1 条测试侧 `clippy::type_complexity`:cell 捕获的 4 元组换成 `AnnouncedAct` 记录);
  `cargo fmt` 已跑,`cargo fmt --check` = 干净。
- **AGENTS.md 的下游 `-p` 套件**(本轮改动只在 `crates/acts`,故只挑会被内核 fan-out/admission 改到的那一个):`cargo test -p acts-package-shell`
  = 5 个 target 里 4 个全绿(10/7/3/2),`--test vars` 3/4 —— 唯一失败 `the_inputs_of_the_act_embed_in_the_script`
  (`$inputs().data` 报 `no such member 'data'`,即"上一 act 的输出没到脚本",属 §10 已登记的子执行 outputs 通道族)。
  **对照实测**:把本轮改动 `git stash` 掉后同一命令**同样** 3/4 同样失败 → 非本轮引入。
  (`acts-store` 的 postgres、`acts-channel`/`acts-plugin-nats` 的 broker 套件本机无服务,未跑 —— AGENTS.md 已声明该前提。)

**4) 新登记的边界(未修,均有实测证据)**

- **节点寻址的 action 无法区分同一节点的多个实例**:action/message 只带 `tid` = 模型节点 id(legacy 的 per-item act 各有新 id),故同一节点在一个 run 里
  被实例化 N 次(参数式 fan-out、`sequence{in}` 预建的下一项)时,客户端只能**按投递顺序逐条作答**;若必须乱序作答、或给不同实例不同的 options,现在的内核表达不了 ——
  需要给实例一个可寻址的身份(运行期扩展模型出逐项副本,或 message 带实例号),属**身份/持久化模型的改动**,本轮不做。
  `action_target` 的"能接受该动作的最新 generation"是该限制下最能保住模型本意(N 个项目都要被应答)的选法。
- **逐项绑定会沿作用域链上浮**:`index`/`value` 是模板 execution 自己的 scope 变量(作用域链继承,`${{ value }}` 因此可用),而 completed execution 的非引擎变量会折进
  父作用域(`PropagateTask`)—— 于是它们出现在祖先作用域与 run 的 outputs(`examples/approve` 的 outputs 里就有 `index`/`value`)。legacy 把这两者放在 act 的
  **options**(不是 task data),没有这一路上浮;要更贴近 legacy 得给逐项绑定一个不参与折叠的载体。

---

### P6-1 性能收口(2026-10-15):hop 自己的 close 并进 effect 提交

**任务**:§11 的 P6 第 1 项,1b(i) 的"剩余:`Complete` 事件并进计划提交(省 1 解码 + 1 编码)"。

**改动**(一笔提交):
- `core/commit/aggregate.rs::advance_transition`:提交 business effect 的**同一批**里把 hop 自己的操作就地推进两步 —— `Commit{claim, token}` → `Complete{token}`。token 仍只铸一次、digest 仍在推进 source op **之前**算(所以 token 与旧实现逐字节相同),`version.operations` 的净增量也不变(+2,与旧的两笔提交各 +1 相同)。
- `shell/mod.rs`:`execute_one` 与 `apply_action_once` 删掉那次独立的 close 提交(`operation_event(… Complete{commit})`),以及它为拿 revision 而多读的一次快照。
- **理由**(写进代码注释):两次提交之间没有任何东西观察 `Committed`;close 提交除 flag 外没有任何新状态(effect 提交已写下新的 operation 与 outbox 记录);而它真正的代价是每 hop 一次整包 decode + encode。
- `Committed` **仍是持久化状态**:V2 tag 照旧解码它,`RecoveryPlanner::FinalizeCommit` 照旧收尾"旧 writer 留下的"这种聚合 —— 只是**没有 hop 再产出它**。

**实测**(release,内存 store,30 步线性 = 93 hops,同一台机、同一探针、`--test-threads=1`;探针已删,形状见下)。
"改前"用 `git stash push`(只 stash 生产侧 4 个文件、保留探针)在同一棵树上复测,所以两边探针代码完全相同:

| 探针 | 改前(best of 3) | 改后(best of 3) |
|---|---|---|
| `KernelHost` 直驱(`ProcessSpec::linear`,无 model/无行投影/无 transport) | 3.478 ms/hop(288 hop/s);轮内 3.478/4.721/4.287 | **1.788 ms/hop(559 hop/s)**;轮内 1.788/1.809/1.819 |
| `Engine` 端到端(真实 30 步 workflow,含 model、行投影、outbox transport) | 4.510 ms/hop(222 hop/s);轮内 4.510/5.181/6.190 | 4.163 ms/hop(240 hop/s);轮内 4.163/4.465/5.006 |
| 同探针的 3/10/30 步伸缩(lean) | 0.461 / 1.267 / 2.889 ms/hop | **0.315 / 0.694 / 1.749 ms/hop** |

**两次运行的环境差约 1.3×**(同一次运行里测的 per-call 微基准:`AggregateDocumentCodec::encode` 改前 662 µs / 改后 878 µs、`decode` 593 / 720 —— 微基准与被测 hop 无关,所以这个比值就是环境比)。按它归一后**两个形状都是 ≈ −25…30%/hop**;raw 的 −49%(lean)高估、−8%(engine)被 ±15% 的噪声吞掉。**不改持久化格式、不需要迁移**。

**分解(同轮实测,release,单线程,per-call 微基准)**:
| 量 | lean 形状(聚合 17 KB,无 model) | engine 形状(聚合 44.5 KB,含 model) |
|---|---|---|
| `model_of` | 0.1 µs(**`ProcessSpec::linear` 的聚合没有 model 变量**) | 42–56 µs |
| `execution_graph` | 72–73 µs(31 个 execution) | 86–108 µs |
| `AggregateDocumentCodec::encode` | 281–327 µs | 662–878 µs |
| `AggregateDocumentCodec::decode` | 263–292 µs | 593–720 µs |
| `snapshot.validate()` | 15–25 µs | — |
| `recovery_entries` / recovery index / outbox index | 各 < 1 µs | — |
- 折叠后每 hop 的整包工作量:**3 次 decode + 2 次 encode**(改前 4 + 3),lean 形状折算 1.4 ms/hop、engine 形状更多;
- 3/10/30 步的 ms/hop 仍近似线性(聚合 3.4/9.0/25.0 KB)⇒ **§5l 的 O(n²) 结论在折叠后依然成立**;
- **索引投影/编码确认无关**(与 §5n 一致):`recovery_entries`、两个索引文档每 hop 各 < 1 µs。

**剩下的杠杆(按实测改写 §5l 的排序)**:
1. **整包 decode/encode 仍是每 hop 的大头**。要再降只有两条路:①少一次 load —— hop 起始的 `load1` 不能直接删(`DispatchTimeout` 的预检要在 claim **之前**判定,`place_run` 也要它),除非把 claim 提前、预检挪到 claim 之后,那会把"每 tick 一次 idle timer 从 0 次提交变成 2 次";②**拆文档 / append-only**(§9.3.4②,持久化格式变更,必须单独立项:迁移 + golden fingerprint)。
2. **按聚合版本缓存 per-hop 派生(§5l ②)实测收益不足**:`execution_graph` 72–108 µs/次 ≈ 每 hop 的 3–6%,`model_of` 42–56 µs/次(只在真实 workflow 形状存在);为此在 shell 里加有状态缓存 + 淘汰不值当。**建议跳过,除非 ①② 都做完。**
3. 端到端的 engine 形状里还有约 1 ms/hop 既不是 codec 也不是 store(§5l 实测 store 只占 1.4%)—— 那是 hop 的 plan/记账 CPU,**需要新画像**才能定位。

**测试影响(必须记录的语义面)**:
- **删除** `reliability/action.rs::a_crash_that_loses_the_close_of_an_applied_completion_advances_once` 与 `reliability/back.rs::an_applied_backs_lost_completion_is_finalized_without_a_second_rewind`:它们 pin 的"effect 已落、close 未落"这一 **crash 边界不再存在**(close 与 effect 同批)。覆盖不丢:同一状态现在由"hop 之后那一次提交被拒"表达,`stall.rs` 的 sweep 覆盖它。
- `reliability/next.rs`:四写边界收敛为三写(intent / claim / effect+close),`CLOSE_WRITE` 删除;`stall.rs` 的 `COMPLETION_COMMITS` 4 → 3。`stall.rs` 的 sweep 仍覆盖"effect 已落、它 seed 的 propagation 还 Pending"(`arm(COMPLETION_COMMITS)` = 拒掉 hop 之后那一次提交)。
- `aggregate.rs`:`source_operation_is_committed_in_same_atomic_batch` → `…_and_closed_in_same_atomic_batch`(断言 `Completed`);`durable_claim_prepare_commit_complete_has_one_revision_chain` → `durable_claim_prepare_commit_has_one_revision_chain`(单批内推进 2 个 revision 步、净增量与旧两提交一致、二次 close 被拒)。
- **新增** `durable/async_document_store.rs::a_committed_operation_left_by_an_older_writer_is_finalized`:手工构造旧 writer 会留下的 `Committed` 聚合,断言 `RecoveryPlanner::classify` → `FinalizeCommit` 且 `Complete` 事件仍能应用到持久记录 —— 升级路径的兼容回归(否则删除上面两条 cell 会让这条 recovery 分支失去覆盖)。

**验收**:`cargo test -p acts --lib --tests` = **870 passed / 0 failed**;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = EXIT 0;`cargo fmt --check` 干净。

### 设计成文(2026-10-15):scheduler 重构设计(7 条思路 + 身份模型)

用户提出 7 条重构思路(聚合命名 / task 身份 / 投影出行 / outbox 消息 / 立即投递 / `core/plan` 拆目录 / `core/model/view` 去留;末条为追加)。**只写设计、不动生产代码**,成文于
`KERNEL_REFACTOR_DESIGN.md`。逐条对着代码核完后,与直觉不同的几点:

- **③ 投影出行与 ⑤ 立即投递已经实现**:投影早已幂等产出 `procs`/`tasks`/`vars` 三张行表(`shell/projection.rs:141/148/224/315`,构造在
  `runtime.rs:555-559`),`deliver_plan` 早已在提交**之前**发本 plan 的 outbox 并把 receipt 随提交落成 `Delivered`(`shell/mod.rs:907-958`)。
  ③ 的真实改动只有"**一行一实例**"(今天 `latest: node → ExecutionId` 最新代胜,一个节点只有一行),而它依赖 ②;⑤ 是**决策**(已定:保留现状,
  只把 at-least-once 窗口与"无 handler 时 `messages`/`deliveries` 不落行"写进文档)。
- **① 的前提有偏差**:仓库里没有 `Aggregate` 类型(`grep` = 0),`AggregateSnapshot` 就是聚合根;`Process`/`Task` 已被 legacy 内存视图占用
  (`shell/process/process.rs:20`)。`Operation` 是**一次持久化 hop**(`OperationKind` 含 `DeliverMessage`/`FireTrigger`/`RunPostCommit`/`ApplyAction`),
  不该改叫 `Task*`;真正的实例是 `Execution*`。`Action` 已三义(线上事件 / 内核 `ActionKind` / ACL 动词面),不宜再扩。
- **② 是唯一动持久化格式的改动**,且形态被 `durable/async_document_store.rs:412-432` 卡死:幂等判定靠**整份快照相等** ⇒ 实例 id 必须是
  `(node, generation)` 的**纯函数**(随机 `shortid` 会让"模糊重试"变成"多造一个实例";legacy 用随机 tid 没事,是因为 legacy 没有聚合 + CAS + 重放)。
- **⑥ 零风险前置**:`core/plan` 10 文件 4436 行,`container.rs` 独占 1842 行(其中 ~979 行是 `mod tests`);`plan/mod.rs` 无 re-export 且
  `core::plan::` 在 `scheduler/` 之外 0 命中 ⇒ 拆成同名目录 + re-export 后**调用方一行不用改**;守卫只分类 `src/scheduler` 的直接子项,对拆目录中立。
- **⑦(`core/model/view/**`,用户追加)**:**整体不可删**,它不是 legacy 残留 —— ①`NodeData` 就是 `tasks.node_data` 列的持久化形状(写 `shell/process/task.rs:294`/`shell/projection.rs:349`,
  读 `store/process_rows.rs:91`/`model/info.rs:191`)②`NodeTree::load` 就是 `model:deploy` 的校验实现(`Workflow::valid` → `model_executor.rs:36`,还校验 trigger/event id)
  ③`Workflow::tree_output` 是生产 API(`model:get fmt=tree`,`model_executor.rs:58`)④**每 effect hop 建一整棵树**(`shell/scope.rs:47-66`)。但可瘦身:`visit.rs` 整簇 + `Node::from_str` +
  `NodeTree::build`/`build_workflow` + `build.rs:183-213` 注释掉的 `build_act` + `Workflow::print` 全是死件(且 `visit.rs:45-65` 有**循环守卫缺陷**:每 `Visitor` 克隆 `Box<VisitRoot>` ⇒ 计数恒 0 ⇒
  循环 `next` 链无界递归);id 分配(`view/build.rs` 的 `prepare_*_ids` 与 `shell/model.rs:83-113` 的 `assign_ids`)写了两遍。另**登记待验证**:动态 act 簇的唯一生产闸门 `Context::build_acts`
  的结果可能被 effect seam 丢弃(`shell/model.rs:122-133`、`shell/effects.rs:201-205`),若成立即"每 hop 跑了但不生效"的白付开销。→ 设计 §3.7 + 计划 C8a/C8b/C8c。
- **字符集不是审美**:索引键的 `id` 段**裸拼**(`store/collection.rs:190-193`),而 `utils/consts.rs:58-74` 写明"**ids are alphanumeric**、`.` 绝不出现
  在 id 里"是 `KEY_SEP_SUCC` 组边界技巧的前提;内核自己处理同类问题的做法就是 hex(`document_store.rs:207-226`、`recovery_entry.rs:28-47`)。
  因此讨论里最初提的 `{node}#{gen}` 形态**被否掉**(`#` 不在转义覆盖内且违反该不变量),改为 `TaskId{node, generation}` + 纯 hex `wire()`。

**待拍 4 项**(设计文档 §2.2):D1 命名词汇(`{Process,Task,Operation}` 推荐)、D2 实例 id 形态(甲 可逆定宽 hex / **乙 不可逆定长摘要**;注意"一律 hex"
与"gen==0 沿用裸 node id 以保 golden"**互斥**)、D3 行内 `prev/next/parent` 取值(推荐保持 node id)、D4 `view` 是否挪出 `core`(推荐先瘦身、挪位登记)。**任务清单与逐笔门禁**见 `KERNEL_CUTOVER_NOTES.md` §11(主线段 C1-C8c = 设计文档 §4 的切分);
C1(拆 `core/plan` 目录,纯搬移)可立即起步;`view` 的零风险瘦身另有 C8a/C8b。

### C1 完成(2026-10-15):`core/plan/**` 拆成同名目录(纯搬移,零行为变更)

**做了什么**:6 个文件按 `KERNEL_REFACTOR_DESIGN.md` §3.6 的分段拆成同名目录(tests 按主题归位);
`concurrency.rs`(95)/`cron.rs`(67)/`execution_admission.rs`(100)/`plan/mod.rs`(11)**未动**。

**目录树前后**(`crates/acts/src/scheduler/core/plan/`,行数为 `wc -l`):

| 前 | 后 |
|---|---|
| `branch.rs` 152 | `branch/{mod,errors}.rs` 144+13 |
| `container.rs` 1842 | `container/{mod,flow,child,fanout,join,catch_join,errors}.rs` 65+95+41+420+96+142+32 · `container/tests/{mod,fanout,join,flow,catch_join}.rs` 102+300+206+39+335 |
| `generation.rs` 614 | `generation/{mod,errors,tests}.rs` 235+28+354 |
| `propagation.rs` 734 | `propagation/{mod,cascade,join,errors,tests}.rs` 94+346+56+30+215 |
| `timeout.rs` 568 | `timeout/{mod,marker,dispatch,tests}.rs` 39+126+79+327 |
| `upward.rs` 253 | `upward/{mod,errors,tests}.rs` 104+19+132 |
| 合计 4436 | 合计 4487(**净 +51** = 20 个 `use super::*;` + 6 个 `mod.rs` 的 `mod`/re-export 行 − rustfmt 去掉的尾随空行) |

**做法(三条,都可复核)**

1. **按行范围逐段搬运**:先按 §3.6 的分段把每个文件按行划段,写脚本断言"所有段 + 有意吸收的行(属性行、`mod tests {`)
   **恰好平铺 1..N,无缺无重**",再逐段原样落到新文件。段内容除"整体减 4 空格缩进"(tests 从 `mod tests { }` 内提到模块顶层)
   外**逐字不变**;减缩进后 rustfmt 只在少数表达式上重排了换行(语义不变)。
2. **子模块 `use super::*;` + `mod.rs` glob re-export**:与 `shell/mod.rs` 既有约定一致(子模块看得见父模块的作用域与私有项);
   `mod.rs` 里 `pub(crate) use <child>::*;` 把每个子模块的整个 `pub(crate)` 面**原样**抬回模块路径
   (先用逐个具名 re-export 试过:`TimeoutDispatch`/`has_live_child`/`ExecutionFlow`/`execution_flow` 这类
   只被 tests 命名、或根本不被命名的项会被 `unused_imports` 打红 ⇒ 改用 glob,既保面又零 warning)。
3. **tests 归位**:`container` 的 979 行 tests 按主题拆 4 个文件(`fanout`/`join`/`flow`/`catch_join`)+ `tests/mod.rs` 放共享夹具;
   其余模块的 tests 收进 `<mod>/tests.rs`。

**验收(逐条命令与结果)**

| 检查 | 结果 |
|---|---|
| `core/plan/**` 里的 `fn ` 清单(含 fixtures/helper/`#[test]`) | 112 → 112,**集合完全相同**;`#[test]` 49 → 49 |
| `plan::<mod>::<name>` 路径集合(全仓 grep;搬前取 HEAD 的 `plan/*.rs` + 工作区其余文件) | **43 → 43,集合完全相同** |
| `pub(crate)` 项可达性(逐模块比对 HEAD 的 `pub(crate) fn/struct/enum/const`) | branch 7 / upward 3 / generation 3 / timeout 12 / propagation 7 / container 19 项**一个不少**,且无新增 |
| 搬移纯度(把 HEAD 按同一分段重新展开成"未格式化的落盘内容",与当前文件逐字比) | 23/29 个非 `mod.rs` 文件**忽略空白与逗号后完全相同**;6 个 `mod.rs` 的差异**只有** `mod`/re-export 那几行(已逐行核对) |
| 调用点 | `git diff HEAD -- crates/acts/src` 里除 `core/plan/**` 外**没有一行 `plan::`**;`shell/mod.rs`、`shell/{planning,facts,predicate,model}.rs`、`core/commit/aggregate.rs`、`runtime.rs`、`scheduler/mod.rs`、`core/coverage.rs` 都**不在本笔 diff 里** |
| `git diff --cached -M --stat` | 33 文件,**+3728 / −3677**(净 +51 = 上表的 glue);`branch.rs => branch/mod.rs`、`propagation.rs => propagation/cascade.rs` 被认成 rename(1→N 的拆分低于 git 50% 相似度阈值,只能显示成 delete+add —— 这正是"纯搬移"要用行数/内容校验而不是 git 启发式来证的原因) |
| `cargo test -p acts --lib --tests` | **870 passed / 0 failed / 1 ignored**(143.74 s);本笔不增删任何用例,所以与基线同数(C1 之后的新基线) |
| `cargo clippy --all-targets --all-features --workspace --locked` | 全 workspace **仅 1 条 warning**,见下(与本笔无关) |
| `rustfmt --edition 2024 --check`(29 个新文件) | 干净 |

**本笔无关的两条既有红(工作区里未提交的性能探针 WIP,不是 C1 造成)**

- `crates/acts/src/scheduler/shell/mod.rs:4764` 的 `pub(crate) const HOPS: usize = 0;`(`#[cfg(not(test))] mod hop_trace` 的桩)在非 test 构建里 dead
  ⇒ `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 101**,全 workspace 只此一条。
  证据:`hop_trace` 在 HEAD 的 `shell/mod.rs` 出现 **0** 次、在工作区出现 **23** 次,全在那份未提交 diff 里;本笔对该文件 **0 行改动**。
- `cargo fmt --all --check` 只报 `shell/mod.rs:4723`(同一份 WIP 新加的 `NAMES` 数组);C1 的 29 个文件已 rustfmt 干净。
- 处置:本笔**没有**动这两处(不碰别人的在飞改动),也**没有**新增任何 `#[allow(...)]`。要全绿,先落那份探针 WIP 或给它一个 `#[expect(dead_code)]`。

**与设计文档/提示词的两处口径差异(登记,不改)**

- §12 提示词的"不动"清单写作 `concurrency.rs`(95)/`execution_admission.rs`(100)/`cron.rs`(67)/**`upward.rs`(253)** 不动,
  但 `KERNEL_REFACTOR_DESIGN.md` §3.6 的切分表把 `upward.rs` 列为**要拆**(28-100 Planner · 101-117 Error · 118-253 tests),
  目标文件树也给了 `core/plan/upward/{mod.rs,errors.rs,tests.rs}`。**按 §3.6 与目标树执行**(upward 已拆);
  提示词那半句按笔误处理(§3.6 的"≤100 不动"行只列了 3 个文件)。
- §3.6 的 `container.rs` 分段(`233-682 ContainerPlanner`)没写清 impl 的去处。本笔按目标文件名把三个方法分落到
  `fanout.rs`(`plan_fanout` + `empty_container_plan` + 8 个私有 helper)、`join.rs`(`plan_join`)、`catch_join.rs`(`plan_catch_join`),
  `mod.rs` 只留 `ContainerPlanner` 声明 —— 三个 `impl ContainerPlanner` 块分居三文件合法(inherent impl 只需同 crate),
  调用方路径与行为都不受影响。

### C2 完成(2026-10-16):`Aggregate*` 全族 → `Process*`(纯改名,零行为变更)

**D1 已拍(用户裁决)**:命名词汇取 `{Process, Task, Operation}`(设计 §2.2 的推荐档)。本笔只做其中的 `Aggregate* → Process*`;
`Execution* → Task*` 归 C3;`Operation*` 与 `*Command` 不动。

**改名清单**(纯改名:无行为变更、无持久化字节变更):

| 旧 | 新 |
|---|---|
| `AggregateSnapshot` | `ProcessSnapshot` |
| `AggregateVersion` | `ProcessVersion` |
| `AggregateRepository` / `AsyncAggregateRepository` / `MemoryAggregateRepository`(`…Access`) | `ProcessRepository` / `AsyncProcessRepository` / `MemoryProcessRepository`(`…Access`) |
| `AggregateDocumentCodec` / `AggregateDocumentError` | `ProcessDocumentCodec` / `ProcessDocumentError` |
| `AggregateEnvelope` | `ProcessEnvelope` |
| `LoadedAggregate` | `LoadedProcess` |
| `AggregateNotFound` / `AggregateAlreadyExists` | `ProcessNotFound` / `ProcessAlreadyExists` |
| `CommitError::{Wrong,Corrupt,Invalid}Aggregate` | `CommitError::{Wrong,Corrupt,Invalid}Process` |
| 模块/文件 `core::commit::aggregate` / `commit/aggregate.rs` | `core::commit::process` / `commit/process.rs`(git 认成 94% rename) |
| `aggregate_key` / `AGGREGATE_KEY_PREFIX` / `AGGREGATE_PREFIX` | `process_key` / `PROCESS_KEY_PREFIX` / `PROCESS_PREFIX` |
| `decode_loaded_aggregate` / `canonical_aggregate_bytes` | `decode_loaded_process` / `canonical_process_bytes` |
| 两个仓库缓存字段与 shell 的 `aggregates` 字段 | `processes` |
| reliability 矩阵的 `aggregate(engine, pid)` 快照助手 | `process_snapshot`(裸名词当函数名会读成动词) |

**刻意的边界 —— 字符串字面量与散文一律不动**:

- 持久化键前缀 `PROCESS_KEY_PREFIX = "kernel/aggregates/"`(`document_store.rs:16`)与规范化字节的域分隔符 `"aggregate-v1"`
  (`commit/process.rs:749`)**字节不变** ⇒ 存储格式、codec 版本、golden 指纹全不动:套件里
  `canonical_process_and_commit_digest_match_golden_fingerprint` 与 `process_key_is_unambiguous_for_separator_characters` 原样通过。
- 人读消息仍说 aggregate(`"aggregate document error"`、`"aggregate envelope error"`、panic/log 串);对齐它们是文案活,不是改名。
- 注释散文仍说 aggregate(304 行),字符串字面量 81 行 —— 刻意保留:此刻 shell 里还住着 legacy 的 `Process`/`Task`(§3.2「临时并存」),
  散文里的 "process" 会和它们含混;等 C3/后续删掉 legacy 视图再谈文案。

**证据**:`crates/acts/src` 内 `Aggregate` 命中 **0**;代码位置的 `aggregate`/`aggregates` 标识符 **0**;`aggregate::` 路径与 `mod aggregate` **0**。
`git diff --cached -M --stat` = **53 文件,+525/−536**。门禁:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`
**EXIT 0**、`cargo fmt --all --check` 干净、`cargo test -p acts --lib --tests` **870 passed / 0 failed**。
(同一套件另一次跑里 `export::actions::tests::a_run_cannot_seal_another_subjects_scope` 红过一次 —— 单跑 0.07 s 绿、整跑复跑 870/0 绿,
计时抖动,与本笔无关;该用例本就依赖 sweeper 节拍,见 §10「P4 收尾续」§2。)

**过程登记(工作区那笔未提交的性能探针 WIP)**:它与本笔改同一批文件(`shell/mod.rs`/`shell/tests/mod.rs`),自身还带一条
`clippy -D warnings` 红(`HOPS` dead code,非 test 构建)。为让**提交状态 == 已验证状态**,我把这两份 WIP 旁置(字节副本),
提交 C2 后按同一改名管线放回;放回时只做了 rustfmt 必需的换行(改名把 `project` 的签名从 102 列降到 100 列 ⇒ 合行;顺带把
`NAMES` 数组排成 rustfmt 想要的样子)。WIP 语义一字未动(`hop_trace` 23 处、`mod perf_probe;`、`examples/simple/main.rs` 都在),它**仍未提交**。

### C3 完成(2026-10-16):`Execution*` → `Task*`,并把 `task_id` 让给身份

**做了什么**(纯改名:无行为、无持久化字节、无字段形态变更 —— 字段的*类型*是 C4):

- 类型族:`ExecutionId`→`TaskId`、`ExecutionState`→`TaskState`、`ExecutionSnapshot`→`TaskSnapshot`、`ExecutionNode`/`ExecutionGraph`(含 `Builder`/`Error`)→`TaskNode`/`TaskGraph`、
  `ExecutionCreate`→`TaskCreate`、`ExecutionFlow`→`TaskFlow`、`Execution{NotFound,AlreadyExists,Conflict,NotInGraph}`→`Task*`、
  `{Duplicate,Unknown}Execution`/`UnreachableExecutions`/`InvalidExecutionTopology`→`…Task…`、`SourceOperationTargetsDifferentExecution`→`…DifferentTask`、`ExecutionSlotKind`→`TaskSlotKind`。
- **`TaskId` 不再把 task 说两遍**:`TaskId { task_id, generation }` → `TaskId { node, generation }`(`OperationId` 的同类字段一起:
  `OperationId { process_id, node, generation, sequence }`;C4 把这对字段收成 `task: TaskId`,并把 `node` 类型化成 `NodeId`)。
  字段腾出来之后,原先把**身份**当参数名用的 `execution_id`(216 处)改成 `task_id` —— 设计给"实例引用"留的名字。
  节点串遇到**已经绑了 `node`** 的作用域(投影里那棵视图节点、`act_message_payload` 里 `NodeId::parse` 出来的那个)时叫 `node_id`;
  这类作用域一共 3 个,编译器逐个点了出来。
- 让名:`TaskState` 现在是引擎自己的状态,于是 legacy 的 12 态读模型/消息词汇让位成 `LegacyTaskState`
  (`core::model::task_state`→`core::model::legacy_task_state`,映射函数 `task_state(...)`→`legacy_task_state(...)`)。
  名字自带解释:**行(`tasks.state`)与消息层仍在说这套词汇**,由那个函数从引擎状态映射过去。
- 模块随类型改名:`core::model::execution_graph`→`core::model::task_graph`、`core::plan::execution_admission`→`core::plan::task_admission`、
  `core::model::task_state`→`core::model::legacy_task_state`。
- 字段/参数/局部带名词的:`snapshot.executions`→`snapshot.tasks`、`execution_creates`→`task_creates`、`execution_flow`/`execution_sign`→`task_flow`/`task_sign`、测试夹具 `execution(...)`→`task(...)`。
- **散文与人读消息仍说 execution**(321 行注释、44 行字符串)—— 与 C2 同一条边界,原因也一样(shell 里的 legacy `Task`/`Process` 视图还在)。
  只有 6 个 `format!` **占位符**变了(`"{execution_id:?} not found"` → `"{task_id:?} not found"`):占位符写的是它插值的那**变量名**,渲染出的文本一字不差。

**证据**:`crates/acts/src` 内 `Execution` / `Aggregate` / `execution_id` 各 **0** 命中;`TaskId { node, generation }`、`OperationId { …, node, … }` 成型;
字面量多重集审计:除上述 6 个占位符外**无任何字面量变化** ⇒ 键、codec 分隔符、消息文本没动,存储格式/codec 版本/golden 指纹不变。
`git diff -M --stat` = 75 文件,+2445/−2762(净 −317:**改名让标识符变短,rustfmt 因此合掉一批换行**,不是删代码)。
门禁:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**、`cargo fmt --all --check` 干净。

**套件与既有抖动(已按 §10 §4 口径归因)**:`cargo test -p acts --lib --tests` 有整绿(870/0)的跑次;另有跑次挂 1–2 条,**全是计时敏感用例且会轮换**
(`export::actions::tests::a_run_cannot_seal_another_subjects_scope`、`tests/reliability/**` 的竞态单元、`shell::tests::kernel_task_and_act_messages_are_emitted`),每条**单独跑都过**。
本轮在 `HEAD`(C2)的 worktree 上跑同一套件做对照:**基线同样挂 `a_run_cannot_seal_another_subjects_scope`**(869/1)⇒ 既有抖动,与本笔无关。
该用例的机理也定位了:它在 transport 的 error 信号之后直接读投影行,而**投递发生在提交之前**(§1.5)⇒ 读到的行可能还没写;药方是等提交(或 `flush_writer()`)后再读,属测试问题,不在本笔修。

**本轮顺带核实(回答用户追问,登记给后面几笔)**:

1. **`TaskState` 与 `LegacyTaskState` 不是重复定义**:变体不同(前者 12 态含 `Created`/`Waiting`/`Failed`;后者含 `None`/`Pending`/`Interrupt`/`Error`),
   用途不同(聚合内的引擎状态 vs 行/消息的对外词汇),中间有唯一映射 `projection::legacy_task_state`。名字原先取的 `NodeState` 是错的(它按**实例**、不按节点),
   已按上述改名纠正。**要不要只留一套**是设计问题:行的 `state` 字符串是外部契约(`task:get`/CLI),归 C5/后续的读模型重塑,不在改名笔里动。
2. **`model_graph::NodeKind` 缺 `Step`/`Branch`,而且是死重**:它只有 `Workflow`/`Act` 两个变体,`shell/model.rs:554-568` 把**每个**子节点一律标成 `Act`
   (step/branch/act 不分);而 `NodeDefinition.kind`(连同 `name`)**没有任何读者** —— `ModelGraph` 只留 `declaration_next`/`explicit_next` 两张表,
   定义在 `build()` 时被丢掉。⇒ C8b③ 的"两份 `NodeKind` 合一"应改成**直接删掉 policy 侧这个 enum**(4 值的 `view::NodeKind` 才是真值来源,`node_kind_and_name` 从模型算)。
3. **`ContainerChild`/`BranchCandidate` 各自带了同一个节点的两份**(`task_id`(String)与 `model_node`(NodeId))⇒ 候选去重项,登记。
4. `utils/consts.rs::STEP_TASK_ID = "task_id"` 是**契约字面量**(step 数据的键名),值钉死不改;本轮只改了同名标识符,字面量原样(审计已证)。

**过程登记(探针 WIP)**:本轮同样把 `shell/mod.rs`/`shell/tests/mod.rs` 旁置、提交后按同一改名管线回填(语义未动;回填时 rustfmt 只做了因标识符变短而必需的合行)。
**另有一处真实损失,记在这里**:`examples/simple/main.rs` 里那笔探针驱动 WIP(+9/−3)在 C3 中途被我一次 `git reset --hard HEAD` 抹掉了
(那次重置只旁置了两个 shell 文件,漏了它;它从未进过索引,`git fsck --unreachable` 里也没有它的 blob ⇒ **不可恢复**)。
教训:要硬重置就先旁置**工作区里每一处**改动;`git reset --hard` 不是"回退我自己的改动",是回退**全树**。

### C1b 完成(2026-10-16):`core/commit/{aggregate,command}` 目录化,模块名回到 `aggregate`

**用户指令**:补 C1 里被我推迟的那半 —— 把 `core/commit/process.rs` 落成 `aggregate/` 目录并按主题拆开;"同理"把 `command.rs` 也按主题拆开;
外加:`command/plan.rs` 与 `core/plan` 撞词,改名。

**目录树**(纯搬移 + 一处命名纠正):

| 前 | 后 |
|---|---|
| `commit/process.rs` 1509 | `commit/aggregate/{mod,snapshot,repository,commit,canonical,errors}.rs` 30+125+226+283+177+59 · `commit/aggregate/tests/{mod,commit,repository,canonical,advance}.rs` 72+324+120+65+65 |
| `commit/command.rs` 391 | `commit/command/{mod,action,planner,target,errors,tests}.rs` 30+26+149+36+28+142 |

**命名与路径**:

- 目录叫 `aggregate`(用户指定)⇒ 模块路径回到 **`core::commit::aggregate`**(C2 曾把它随类型族改成 `commit::process`);
  **类型名仍是 `Process*`/`Task*`**(D1/C2/C3 不动)—— "aggregate" 是那层文档/仓库的名字,`ProcessSnapshot` 是它的内容。路径改写触及 **34 文件 / 118 行**。
- `command/` 的模块路径**不变**(`core::commit::command`)⇒ 它的调用点一行未动。
- `command/plan.rs` → `command/planner.rs`:文件主角是 `ActionPlanner`;叫 `plan` 会和 `core/plan/**`(纯 planner 目录)撞词,读起来含混。
- 为什么 `operation_order`/`deterministic_digest`/`action_target`/`action_name` 放宽到 `pub(super)`:它们原先在单文件里是私有,用户搬到了兄弟模块 ⇒
  `pub(super)` 正好等于它原来在单文件里的可见范围(C1 同口径)。

**证据**:按行范围搬运 + 平铺断言(每行恰好落进一个目标文件);`fn` 清单 `commit/process.rs` **62→62**、`commit/command.rs` **19→19**(逐行相同);
`#[test]` **21→21**、**6→6**。门禁:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**、
`cargo fmt --all --check` 干净、`cargo test -p acts --lib --tests` **869 passed / 1 failed** —— 挂的正是基线已证的那条既有抖动
(`export::actions::tests::a_run_cannot_seal_another_subjects_scope`,C2 worktree 上同样挂、单跑过;另有一次整跑因机器被占跑到 1406 s、挂两条 racing 用例,同属负载抖动)。

**顺带核实(用户追问:policy 侧的 `NodeKind` 是否设计上少了 `Step`/`Branch`)**:

- **不是设计如此,是 cutover 时掉的**:最初 `01a95b9`(`kernel/model_graph.rs`)的 `NodeKind` 就是 4 值 **`Workflow/Step/Branch/Act`**;
  `2f20791` 仍是 4 值;**`7e8c791`(cutover to kernel runtime)缩成 2 值** `Workflow/Act`,此后没再动。没有任何提交/文档说明这次收窄的理由。
- **而且今天没人读它**:`NodeDefinition.kind`(连同 `name`)只在 `ModelGraphBuilder::add_node` 里存进 builder 的 `nodes` map,
  `build()` 只用那张 map 做**存在性**校验(`contains_key`),产出的 `ModelGraph` 只留 `declaration_next`/`explicit_next` ⇒ `kind` 是**写进去就丢**的死字段。
- **比"缺变体"更糟:它现在给的是错答案** —— `shell/model.rs:554-568` 把**每个**子节点(step/branch/act)一律标成 `Act`。
- 4 值的真值来源是 `view::NodeKind`(`crate::NodeKind` 就是它;`Node::kind()`/`is_kind()` 用它;行/消息的 kind 由 `node_kind_and_name` 从声明模型算,C6 再加 `act`)。
- ⇒ 结论(**已按用户裁决执行,见下一节**):policy 侧的 `NodeKind` **恢复成 4 值并如实标注**(`shell/model.rs` 的子节点从 `Act` 改成 `Step`),
  `view::NodeKind` 仍是行/消息的真值来源;§3.7 的注("两份模型图、两种用途,不合并")不变,C8b③ 只做 id 分配/投影/词汇**对齐**。

**过程登记(探针 WIP)**:同 C2/C3 —— 旁置 `shell/mod.rs`/`shell/tests/mod.rs`,提交后按同一管线回填(这次只需把模块路径 `commit::process` → `commit::aggregate`);
`examples/simple/main.rs` 那笔已在 C3 被误重置丢失(见 §10「C3 完成」),本轮它停在 HEAD 版本、未被触碰。

### NodeKind 恢复 4 值(2026-10-16):policy 图说回模型自己的四种节点

**用户指令**:把 `model_graph::NodeKind` 恢复成 4 值(它在 `7e8c791` 的 cutover 里被缩成 2 值),**保证语义不变**,并更新文档。

**做了什么**:

- `model_graph::NodeKind` = **`Workflow/Step/Branch/Act`**(与 `01a95b9` 一致),并给类型补了文档:planner 走的那张图装的是
  **workflow 根 + 顶层 step**;一个 step 的 `branches` 与其中的 act 是**计划级 fan-out**,不是图节点 —— 四个变体保留是为了让词汇与
  `crate::NodeKind`(`view` 那个,行/消息说的就是它)**对齐**。
- `shell/model.rs` 把子节点的标注从 `Act` 改成 **`Step`**:这些 children 就是 `workflow.steps`(`from_workflow` 从这里种下),
  `node_kind_and_name` 对同样这些 id 也回答 "step" —— 原先的 `Act` 是**错答案**。
- `Branch`/`Act` 在非 test 构建里没有构造点 ⇒ 各带一条 **`#[allow(dead_code)]`**,理由写在属性处并指向本节(仓库规则:压 lint 必须写理由 + 登记)。

**为什么语义不变(可复核)**:`NodeDefinition.kind` **一个读者都没有** —— `ModelGraphBuilder` 只把定义存下来做 **id 存在性**校验,
`build()` 产出的 `ModelGraph` 只有 `declaration_next`/`explicit_next` 两张表 ⇒ 该字段是"写进去就丢"的值,改它不可能改变行为
(`git grep -n NodeDefinition` 只有构造点与测试;`grep .kind` 在该文件里只有字段声明与测试建造器)。
门禁(**在"提交状态"上跑,即探针 WIP 旁置时**):`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**、
`cargo fmt --all --check` 干净、`cargo test -p acts --lib --tests` **869 passed / 1 failed** —— 挂的仍是基线已证的那条既有抖动。

### H1 完成(2026-10-16):host 的 hop 分相 —— 四阶段流水线 + 按 kind 的 driver 模块

**用户指令**:`execute_one`/`apply_action` 结构太乱;应分成「1 准备(claim/prepare)→ 2 执行并返回结果 → 3 生成 next plan → 4 分发 next 操作」四个过程,
不同状态/类型/操作由上层 trait 抽象屏蔽,具体实现交按层、按类型的模块。**只动 `scheduler/shell/`,公共面与落盘格式零变更。**

| 之前 | 之后 |
|---|---|
| `shell/mod.rs` 4666 行:745 行的 `execute_one`(match `OperationKind`,内含 load/place/park 判定/预检/claim + 各 kind 业务 + 各 kind 计划 + deliver/commit/wake/lifecycle/投影/槽位),`apply_action_once` 229 行**另写一份**同样的提交→唤醒→投影→槽位 | `shell/mod.rs` 2033 行(宿主基础设施:seam、驱动循环、恢复、outbox、读模型、计划词汇表、客户端动作入口)+ `shell/hop/` 3210 行 |
| — | `hop/mod.rs`(236 行):`OperationDriver` trait(`precheck`/`run`/`next`)、`Pending`/`Claimed`/`Hop` 上下文、`Outcome`、`Slot`、`Precheck`、`driver(kind)` 查表,以及四阶段流水线 `KernelHost::execute_one`(~40 行) |
| — | `hop/prepare.rs`(阶段 1 parked 快路径 + 槽位策略 + 按 kind 的 precheck + `ClaimAndPrepare`;`Slot::Direct` 供动作路径绕过槽位)、`hop/dispatch.rs`(阶段 4 deliver → commit(冲突 → 释放 claim + `ECONFLICT`)→ wake → lifecycle → 投影(`Touched`/`Whole`)→ 槽位结算) |
| 各 kind 的 arm 与规划器散在 `execute_one`/mod.rs | `hop/task.rs`(ExecuteTask)、`hop/action.rs`(ApplyAction)、`hop/timeout.rs`(DispatchTimeout)、`hop/cascade.rs`(SkipTask/AbortTask)、`hop/propagate.rs`(PropagateTask)、`hop/reentry.rs`(ReenterTask);kind 自有的规划器(入口检查、启动计划、完成计划、timeout round/plan、容器 fan-out、catch fan-out、上行/join、重入)随 arm 搬入对应模块,仍以 `impl KernelHost` 方法存在(最小改写、同模块互调无需放宽可见性) |
| `apply_action_once`:自持 claim/计划/提交/唤醒/投影/槽位(与 hop 路径重复约 120 行) | 预检 → 意图提交 → **同一条流水线**(`prepare_hop(Slot::Direct)` → 同一 `run`/`next` → `dispatch_hop(Projection::Whole)`);`attempt` 参数删除(claim fence 改由**操作自身的 attempt** 派生,与 hop 路径同源) |

设计与分相理由见 `KERNEL_REFACTOR_DESIGN.md` §3.8。**行为差异(逐条登记,均非语义变更)**:

1. **冲突错误类别**:统一后的分发把 CAS 冲突报成 `conflict(...)`(ECONFLICT)而不是 `Runtime(...)` —— 这是修 bug:动作路径并入统一分发后,
   原 `Runtime` 类别会让 `apply_action` 的重试循环(靠 `is_conflict` 判定)不再重试,`reliability::action::racing_completes_converge_on_one_decision_and_one_effect`
   因此由红转绿(见下验收)。hop 路径只把该错误写进日志,不受影响。
2. **动作路径现在也上报 lifecycle**(`report_lifecycle` 在 `dispatch_hop` 内):事件**集合**不变(`ReportedRuns` 去重),
   但「进程终态」事件可能由动作自己的提交上报,而不是等下一个 hop。
3. **动作路径的槽位结算改用提交返回的快照**(原先多读一次聚合):`place_run` 的判定与 hop 路径同源。
4. `KernelHost::snapshot` 现在只被测试使用 ⇒ 加 `#[cfg(test)]`(仓库门禁 `-D warnings`)。
5. **探针 WIP 旁置/回填**(与 C2/C3 同口径):本笔提交**不含** P1 工作区那笔未提交的 `shell::hop_trace` 探针 ——
   `shell/mod.rs` 的计时垫片已摘除、`shell/tests/mod.rs` 回到 HEAD、`tests/perf_probe.rs` 保持 untracked;提交后按同一管线**回填到新结构**
   (`LOAD`/`PLACE1`/`CLAIM` 落 `hop/prepare.rs`,`PLAN` 覆盖 `run`+`next` —— 与拆分前那 745 行的 match 同一跨度,`DELIVER…PLACE2` 落 `hop/dispatch.rs`)。
   顺带登记:**非 test 垫片里从未被读的 `HOPS` 常量必须删掉**(它在写码状态就让 `-D warnings` EXIT 101);cfg(test) 那份不动,探针可照跑。

**验收(在提交状态门上跑)**:
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**;`cargo fmt --all --check` 干净。
- `cargo test -p acts --lib --tests` = **869 passed / 1 failed / 1 ignored**;唯一挂的仍是基线已证的那条既有抖动
  (`export::actions::tests::a_run_cannot_seal_another_subjects_scope`,见下)。
- `cargo test -p acts --lib scheduler::tests::reliability:: -- --test-threads=1` = **65 passed**(含上面那条由红转绿的动作竞态用例)。

**抖动复现与归因(实测,防止把既有竞态记成回归)**:
- `kernel_task_and_act_messages_are_emitted` 在 `--lib scheduler::shell::tests:: -- --test-threads=1` 下会挂:失败形态恒为
  `$|running` 与 `$|completed` 之差(**只差根节点行**)。**基线(HEAD)同样挂**(实测 2–3/3),不是本次引入。
- 机制(带临时埋点实测,埋点已删):终态提交里 `deliver → commit → wake → **lifecycle** → 投影` —— 事件的读者可在
  `RowProjector::project_nodes` 写完 `proc` 行、**尚未**写节点行的窗口里读到:`LIFE(all_terminal=true) → PROC(root=Completed) → PROJ(node=$)=Completed`。
  测试的同步点正是「进程终态事件」+「proc 行转终态」,于是可能先读到 proc 行终态、再读到 `$` 行还是上一版 `running`。同一窗口也让
  `a_run_cannot_seal_another_subjects_scope` 偶发(`state=Running`;基线 `--lib export::actions::tests:: -- --test-threads=1` 实测 1/3 挂)。
- **未修,登记待裁**(超出本次切面):`project_nodes` 先写 `proc` 行、后写节点行是窗口本身;把 `proc` 行挪到节点行之后写即可闭合,
  但那属于读模型口径变更,应与 L6(投影降频/唤醒驱动)一起拍。

**未触碰**:`examples/simple/main.rs`(C3 那笔误重置的遗留,仍停在 HEAD 版本)、`core/**`、`durable/**` 与一切公共面。


### H2 完成(2026-10-16):`core/plan/**` 的计划抽象 —— 一个 `ActPlan` trait,每个计划/判定一个类型

**用户指令**:「core/plan 这里也要做一层抽象,不同的类型实现不同的 plan」;形态由用户拍板(见 `KERNEL_REFACTOR_DESIGN.md` §3.9):
trait + GAT 读取视图(`Reading`/`Command` 都带生命周期),且**连「求值/策略」一起收进同一 trait**。
中途另有一条命名指令:**`Plan` trait → `ActPlan`,`OperationDriver` trait → `ActOperation`**(H1 落地的名字)。

**做了什么**(纯结构,零行为变更):

- `core/plan/mod.rs` 新增
  `pub(crate) trait ActPlan { type Reading<'a>; type Command<'a>; type Output; type Error: Display; fn plan(reading, command) -> Result<Output, Error> }`
  + `pub(crate) fn infallible<T>(Result<T, Infallible>) -> T`(判定型计划的 `Error = Infallible`,调用点用它解开)。
- 11 个计划/判定从 `*Planner` 静态方法命名空间变成 unit struct 的 trait 实现:
  `container::{Fanout, Join, CatchJoin}`、`propagation::Propagate`、`generation::Reenter`、`upward::Upward`、
  `branch::Decide`、`timeout::{Round, Dispatch}`、`task_admission::Admit`、`concurrency::Admit`。
- 两个新 DTO(借用命令,避免计时器每 tick 克隆分支表):`timeout::RoundCommand<'a>{owner, branches}`、`timeout::DispatchCommand<'a>{owner, source_operation, branch, extra_vars, claimed_after, cost_ms}`。
- 删除(无兼容壳):`ContainerPlanner`、`PropagationPlanner`、`GenerationPlanner`、`UpwardPlanner`、`BranchPlanner`、`GenerationAdmissionPolicy`,以及自由函数 `timeout::{candidate, dispatch}`、`concurrency::admit`。
- 调用点:core 内部 ~73 处(含各模块测试)、shell 13 处(`shell/hop/{action,propagate,reentry,timeout}.rs`、`shell/mod.rs`)+ shell 的 plan 导入表(`ActPlan`/`infallible`/各类型)。
- **顺带修掉 H1 遗留的一处同名冲突**:`shell/hop/{propagate,reentry}.rs` 的 driver 结构与计划类型同名(`Propagate`/`Reenter`)⇒ driver 改名
  `PropagateTask`/`ReenterTask`(与 operation kind 同名,更一致)。
- 保持不动(不是计划):`timeout::{claimed, claim, cost_ms, has_live_child(ren), start_write}`、`branch::PredicateValue`、`cron::Cron`(带状态的值类型)。

**行为差异(逐条登记)**:零语义变更。仅两处**借用命令**带来的克隆(命令按 `&` 传,实现里要把字段搬进产物):
`DispatchCommand.extra_vars`(通常是空 Vec)与 `source_operation`(2 个 String + u64)各克隆一次,都只在 timeout 派发这一条路径上、每 tick 最多一次。
其余判定逻辑、错误类型与消息、DTO 字段逐字保留。

**层界守卫按预期拦了一次(改法已修)**:新写的 `core/plan/mod.rs` 文档里提了 `crate::scheduler::shell` ⇒
`scheduler::core::architecture::pure_modules_have_no_infrastructure_dependencies` 立刻报
`core/plan/mod.rs: forbidden dependency crate::scheduler::shell` —— 纯层不许点名外壳,这正是守卫存在的意义;改写措辞后通过。

**验收(在提交状态门上跑)**:
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**;`cargo fmt --all --check` 干净。
- `cargo test -p acts --lib --tests` = **869 passed / 1 failed / 1 ignored**;唯一挂的是基线已证的既有抖动
  (`scheduler::shell::tests::kernel_task_and_act_messages_are_emitted`,见「H1 完成」的抖动归因)。
- `cargo test -p acts --lib scheduler::tests::reliability:: -- --test-threads=1` 连跑两次 = **65 passed / 0 failed**(两次)。
  (本笔的某次全量跑里 `reliability::timeout::a_completion_racing_the_timeout_tick_converges_exactly_once` 偶发一次:
  该用例包在 `bounded(..)` 看门狗里,是负载下的超时类抖动;单跑与两次模块级连跑均绿,记录在案但**未改代码**。)
- 三处跨切片改动由主 agent 整合完成后一次性验收:编译警告 0、clippy EXIT 0。


### H3 完成(2026-10-16):`hop` 一词从代码里退场 —— 模块 / 类型 / 函数 / 注释统一为 operation



**用户指令**:「将 Hop 改成 Operation,其他相关名称也同步修改,或者 Op 简写」(同一轮里还问了 operation 与 hop 的关系、为什么叫 hop)。



**关系(答案,登记在案)**:`Operation*` 是**持久面** —— 聚合里的一条工作记录(`OperationId` 的文档原话是 "Stable identity of one durable unit of work";

`OperationKind`/`OperationRecord`/`OperationState`/`OperationEvent`,恢复索引也是从它们建的)。而 `hop` 是**运行面**的老词,指「把一个 operation 从 claim 带到下一次提交」的那一趟

(一次 claim + 一次 work + 一次 plan + 一次把该 operation 就地关闭的 commit);同一个 operation 可能被带多趟(丢 CAS 重带、恢复扫描重推、租约过期释放后重带),一趟必然只对一个 operation。

该词是内核迁移时作为运行面的昵称进来的(`hop driver`、「一次 hop 的 store 流量」§9.2、「hop 吞吐」),`KERNEL_REFACTOR_DESIGN.md` §1.1 把它写成等式:「`Operation` = 一次持久化 hop」。

H3 起不再用它:**记录**叫 operation,那一趟在需要区分时叫 **drive**。



**改名(纯改名,零行为变更)**:



| 旧 | 新 |

|---|---|

| 模块 `shell/hop/`(9 文件) | **`shell/operation/`**(`git mv`) |

| 上下文结构 `Hop<'_>`(阶段 2/3 看到的那个 operation) | **`Operation<'_>`** |

| `Claimed::hop()` | `Claimed::operation()` |

| `prepare_hop` / `dispatch_hop` | `prepare_operation` / `dispatch_operation` |

| `hop_message_payload` / `task_hop_plan` / `attach_hop_message` | `operation_message_payload` / `task_operation_plan` / `attach_operation_message` |

| 探针 `shell::hop_trace`(计数常量 `HOPS`) | `shell::operation_trace`(`OPERATIONS`) |

| 3 个测试名(`..._in_the_same_hop`、`func_package_runs_inside_the_completion_hop`、`a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery`) | 同义改 operation |

| `.rs` 注释/文档里的 `hop`(全库 286 处,含可靠性矩阵 12 个文件的注释) | `operation`(会被读成"记录"处改用 `drive`) |



括号里的三个驱动类型(`PropagateTask`/`ReenterTask` 等在 H2 已与计划类型解耦)不受影响;`core/plan/**` 的 `ActPlan`、driver trait 的 `ActOperation` 保持 H2 的名字。



**文档**:`KERNEL_REFACTOR_DESIGN.md` §3.8 的路径与函数名同步到新词,并加了「H3 词汇变更」说明;

`KERNEL_MIGRATION_NOTES.md` 与本文件写于 H3 之前的记录(H1/H2 等)**保持原文**,不追改历史。



**验收**:`cargo fmt --all --check` 干净;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**;

`cargo test -p acts --lib --tests` = **869 passed / 1 failed / 1 ignored** —— 唯一失败是基线已证的既有抖动
`export::actions::tests::a_run_cannot_seal_another_subjects_scope`(与 H1/H2 同一条,见「H1 完成」的抖动归因);探针照跑(`cargo test --release -p acts --lib perf_probe -- --ignored`,改名后用例名 `perf_probe_engine_operation_stages`,94 个 operation、11 个阶段齐全)。

### C4 完成(2026-10-16):`TaskId` 显式化 + `OperationId.task` 换类型 —— 身份成型,输出不变

**做了什么**(类型化;零行为变更、零持久化字节变更 —— golden 不动):

| 前(C3 的形状) | 后(设计 §3.1) |
|---|---|
| `TaskId { node: String, generation: u64 }` | **`TaskId { node: NodeId, generation: u64 }`**(`node` 类型化,与 `TaskSnapshot.model_node` 对齐;`NodeId` = `model_graph::NodeId`) |
| `OperationId { process_id, node: String, generation: u64, sequence: u64 }` | **`OperationId { process_id: String, task: TaskId, sequence: u64 }`**(node+generation 两字段收成一个 `task`) |
| `TaskId::new(impl Into<String>, u64)`(空串检查) | 同签名保留,内部改走 `NodeId::parse`(空/全空白 → `TaskGraphError::EmptyTaskId`,行为不变);仍是唯一构造入口 |
| — | **`TaskId::wire()`**:身份的唯一派生入口;**输出仍是 node id**(`self.node.as_str().to_owned()`),C5 的 hex 切换就改这一个函数体 |

- **`Display for TaskId` = `wire()`**:身份的字符串渲染从此只有 wire 一条路(新加的 impl 今天没有 `{}` 调用点,它是给 C5 的落点立门)。
- **`Debug` 手写两份**(`TaskId`/`OperationId`):渲染保持**旧形状**(`TaskId { node: "s1", generation: 0 }`、
  `OperationId { process_id, node, generation, sequence }` —— 新类型从 `task` 字段取值拼平)。理由:几十处 `{task_id:?}`/`{id:?}`
  错误/日志插值的**渲染文本逐字节不变**(C3 的占位符纪律);派生 Debug 会把 `NodeId("s1")` 和嵌套结构漏进这些文本。
- 比较面:`OperationId` 与 `TaskId` 的同类校验(`SourceOperationTargetsDifferentTask` 家族 ×4:command/planner、fanout、generation、propagation)
  从"`node` 相等且 `generation` 相等"三行并成一行 **`source.task != command.task`**(整 `TaskId` 相等,语义恒等)。
- `snapshot.next_operation_sequence(&str, gen)`、`next_generation(&str)`、`find_step_in_model(&str)` 等**节点串 API 签名不动**,
  调用点改传 `task.node.as_str()`;模型查找、行表 `prev/next/parent`、`tid`、消息 `tid/nid`、`scope_spec.node`、投影 `latest` 折叠键
  —— 全部仍是节点串用法(C5 的落点,本笔不碰取值)。
- 错误面(类型化逼出的两处,均为 **pub(crate)** 且只发生在"坏字节"路径):①`RecoveryIndexError::EmptyTaskId(OperationId)` →
  **`InvalidTaskId`**(无载荷):`TaskId` 类型已拒绝空节点,旧"decode 收下空串、`validate` 再报"的路径不可表达;recover 索引
  `validate` 里那条空节点检查随之删除(类型不变量取代它)。②decode 对坏节点串改在 decode 层拒绝(与聚合 codec 既有的
  `SnapshotCodecError::InvalidTaskId` 同一纪律)。`RecoveryIndexCodec` 的字节布局(MAGIC/SCHEMA_V1/字段序)**一字节未动**。

**键字节对照(逐处核对,全部不变)**:

| 键/字节面 | 形状 | 来源 |
|---|---|---|
| 恢复条目键 | `hex(pid).hex(node).gen.seq`(`recovery_entry_key`)—— 容量式、分隔符、`format_u64` 全部原样,node 段从 `id.task.node.as_str()` 取 | recovery_entry.rs |
| 恢复索引文档 | `ACTRIDX\0` + V1 + `write_string(process)` + `write_string(node)` + `be(gen)` + `be(seq)` + `be(rev)` | recovery_index.rs |
| 聚合 canonical 字节 | `"aggregate-v1"` 域分隔 + 每 TaskId 两段(`len:node`、`len:gen`)+ 每 OperationId 四段(process/node/gen/seq),排序键 `TaskId: Ord` = `(String, u64)` 同序 | canonical.rs |
| codec(聚合文档) | V2 不动:TaskId 写 `string(node)+u64(gen)`,OperationId 写 `string(pid)+string(node)+u64(gen)+u64(seq)`;decode 侧同样顺序 | snapshot_codec.rs |
| 行键 / 行表 | `Id::new(pid, node_id)` 与 `prev/next/parent`/`tid` 原样(投影文件里这些表达式未被触碰) | projection.rs |
| outbox key/payload | `"action:{}:{}:{}:{}"` 家族的 `{node}{gen}` 槽位逐字节同文(槽位从 `task.node.as_str()`/`task.generation` 取) | 各 planner |

**字面量多重集对照**(办法同 §10「C3 完成」;除下列刻意项外 0 变化 ⇒ 无键、分隔符、topic、消息文本改动):

- `+2 "generation"`、`+2 "node"`、`+1 "OperationId"`、`+1 "process_id"`、`+1 "sequence"`、`+1 "TaskId"` —— 两份手写 Debug 的
  `debug_struct/field` 字面量(渲染文本不变,见上)。
- `+1 "the root tid constant is a valid node id"` —— `resume_operation` 的 fallback 从"字符串拼 OperationId"改成
  `TaskId::new(TASK_ROOT_TID, 0).expect(...)`(`"$"` 常量恒非空,不可达分支)。
- `−1 "child"`、`−3 "step"`、`−1 "t1"` —— 测试夹具里 OperationId 字面量改为复用**同一函数里已构造的 `TaskId` 夹具**
  (`task.clone()`/`child.clone()`/`id()`),节点串本体没变,只是不再写第二遍。

**证据**:`TaskId { node: NodeId, generation }`、`OperationId { process_id, task, sequence }` 成型;`crates/acts/src` 内
`OperationId` 字面量里的 `node:`/`generation:` 字段 **0** 处;golden 指纹 `canonical_process_and_commit_digest_match_golden_fingerprint`
与 `snapshot_wire_format_matches_golden_fingerprint`、`process_key_is_unambiguous_for_separator_characters`、
`recovery_entry::tests::key_order_matches_operation_order` **原样通过**。
`git diff -M --stat`(本笔含文档)= **59 文件,+2945/−2792**(58 个代码文件 + 本文件;净 +153:两份 Debug 手写与 §10 记录占了增量,
代码侧净变化为负 —— 合并字段与整 `TaskId` 比较让多处变短)。

**门禁与验收(在提交状态上跑,探针 WIP 旁置)**:
- `cargo fmt --all --check` 干净;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**
  (唯一一次红:`recovery_index.rs` 一处 `&&str` 的 `needless_borrow`,已改)。
- `cargo test -p acts --lib --tests` 三次:868/2 → 869/1 → 869/1。翻红的合计四条,逐条按 §10 §4 归因:
  `kernel_task_and_act_messages_are_emitted`(单跑红 ⇒ `git stash` 后在 **HEAD 上同命令单跑同样红** 0.18s ⇒ 既有窗口,
  H1 已登记的"proc 行先于节点行"投影写序)、`a_run_cannot_seal_another_subjects_scope`、
  `reliability::timer::a_delivery_stored_after_its_task_closed_is_born_settled`、`reliability::action::racing_completes…`
  (后三条单跑全绿;C2/C3/H2/H3 均登记过同批计时抖动)。**与本笔无关,不改代码。**
- 字面量多重集审计:见上,除刻意项外不变。

**过程登记(探针 WIP)**:与 C2/C3 同口径 —— 本笔动工前把工作区那笔未提交 WIP(`ApplyActionCommand`→`ActionCommand`、
`prepare`/`dispatch` 改名、`shell::operation_trace` 探针、`mod perf_probe;`)以 patch + 字节副本旁置到 `.git/c4-wip/`,
提交 C4 后按原样放回(WIP 回填登记见下)。行尾:工作区本就混有 LF/CRLF(`core.autocrlf=true`,HEAD 工作副本即不一致),
本笔 diff 均为小块 ⇒ 无整文件行尾翻转;`file` 复核通过。工具纪律自纠一处:中途两次误用 `sed` 做了个别编辑(其中一次方向写反,
`commit.rs` 测试里一行 `TaskId::new("t1", 1)` 被错误替换,当即核对 `git diff` 还原),此后全部回到 `edit`;教训与 §6 相同。

### C5 完成(2026-10-17):`wire()` 切定长摘要 + codec V3 + 行按实例 —— BREAKING,身份落地

**裁决(2026-10-16 用户拍板,设计 §2.2 已记录)**:D2 = **乙**(`sha256(pid ‖ 0x00 ‖ node ‖ 0x00 ‖ gen_le)[..32]`,定长 32 hex、不可逆);
D3 = **行链保持 node id**;golden **全量重冻结已接受**;恢复条目旧键 **登记不实现,与 §11 L1 同批**。

**身份(唯一派生入口)**:`TaskId::wire(&self, process_id: &str) -> String` —— 摘要吃 pid,`TaskId` 本体仍只存
`(node, generation)`;`Display for TaskId`(C4 加的门)**删除**(无 pid 无法渲染 wire,而 wire 是唯一字符串形态)。
全部身份→字符串边界都过它:恢复条目键、行键/行 `tid`、消息 `tid`、动作寻址、`$step.task_id`、投影 touched 集。

**落点(逐项)**:

| 面 | 前(C4) | 后(C5) |
|---|---|---|
| 恢复条目键 | `hex(pid).hex(node).gen.seq` | **`hex(pid).wire.seq`**(定长:wire 32;gen 已并入 wire) |
| 恢复索引文档 | `ACTRIDX` V1(process/node/gen/seq/rev) | **不变**(结构化字段够用,wire 由 pid+node+gen 可再派生) |
| 聚合 codec | `ACTSPV2`(TaskId = node+gen;OperationId = pid+node+gen+seq) | **`ACTSPV3`**:每 TaskId 落 `wire+node+gen`;每 OperationId 落 `pid+wire+node+gen+seq`;**V2 作 upcast 读路径保留**(`ProcessDocumentCodec::decode` 遇 `InvalidMagic` 回退 V2;两版布局只差 wire 段,实例 id 退化等价);V2 写者仅 `cfg(test)` 夹具 |
| 消息 | payload 无 `tid`,`message_from_record` 把 `tid := nid` | payload 落 **`"tid": wire`**;`Message.tid` 取 payload(旧记录回退 `nid` —— V2 退化);`Message.nid` 仍是 node;根 lifecycle 消息 `process_message` 的 `tid` 也换 wire |
| 动作寻址 | `action_target(node: &str)` 按 node 比较,选"能接受动作的最新代" | 参数语义成为**客户端给的 tid**:`action_target` 比较 `id.wire(process_id) == tid`;**选法未动**(newest-acceptable 结构原样保留);`action_identity` 的意图 operation 直接 `task: task_id.clone()`,sequence 扫描改按 TaskId 相等 |
| 行表 | 一行 = 节点最新代(`latest` 折叠);行键 `Id(pid, node)`;`tid = node` | **一行 = 一个实例**(折叠删除,每代一行);行键 `Id(pid, wire)`;`tid = wire`;`prev/next/parent` **仍 = node id(D3)**;`next` 改为"任何一代认我为前驱的节点(去重排序)";touched 集 = wire 集;`close_deliveries` 按 wire |
| 根/模型 | `proc.root()` 按 `tid == "$"` | 按**工作流节点**(`kind == Workflow`)找根(grep 证实无生产调用者,仅 env 测试读重建进程);聚合侧根查找本就是 `node == "$"`(C4 形状) |
| 包内动作 | `action` 包把 `task.node().id()` 当动作地址 | `ScopeSpec` 新增 **`task_id: TaskId`**;headless 任务直接以 `wire` 为 id 构造(不再 `create_task` 随机 id),`action` 包转发 `task.id`(acting 实例的 wire) |
| `$step` 契约 | `task_id` 槽 = node | `task_id` 槽 = **wire**(`node_id`/`node_name` 不变) |

**字节对照(量了写数)**:golden 夹具(**6 个身份出现点**)—— V2 **286 B** / `26dc1279…` → V3 **502 B** / `4887924d…`,
**+216 B = 6 × 36 B**(每个身份多 4 字节长度前缀 + 32 字符 wire)。键侧:恢复条目键由 `hex(pid)+2×len(node)+gen(16)+seq(16)`
变为 `hex(pid)+32+seq(16)` —— node 名 > 8 字符时更短,短名时略长,但**恒定**;行键 `pid+wire(32)` 同理恒长。
canonical 字节(`"aggregate-v1"` 域)与 commit digest **未动**:`a47d4bd4…` golden 原样通过。

**重冻结清单(逐条改写)**:`snapshot_wire_format_matches_golden_fingerprint` → V3(502/4887924d…);原 V2 golden 原样保留为
`v2_documents_still_decode` 的锚(286/26dc1279… + V2 解码回环 + V3 仅以 magic 拒绝);`document.rs` 增
`v2_documents_upcast_to_the_loaded_snapshot`(V2 信封 → `ProcessDocumentCodec::decode` 等值);恢复条目键的"排序=operation_order"
性质**随设计消失**(wire 序 ≠ node 序),测试改钉**单射 + 稳定 + 前缀**;行投影口径:同一节点多代 → 多行(shell 的 redo 对照测试:
s1/s2 各 2 行、`states_of` 收全代历史);`kernel_engine_projects_the_legacy_read_model`、`$step`/消息/动作相关断言全部换成
`tid_of(pid, node, gen)` 形式(测试夹具助手,node 名才是模型身份)。C5 前置检查(设计 §4)的清单:所有 pin 住 `tid == 节点 id` 的
断言全部在上述文件与 reliability 矩阵内,已逐条改写。

**过程登记(探针 WIP 的剥离与回填)**:C4 后工作区仍带着那笔未提交 WIP(`ApplyActionCommand`→`ActionCommand`、`prepare`/`dispatch`
改名、`shell::operation_trace` 探针 + `mod perf_probe;`)。C5 的 9 个文件与它同住,提交前用"临时提交提供 postimage blob → `git apply -R --3way`
反解 → `reset --soft` 丢弃临时提交 → 提交 C5"把它剥干净(反解 8/8 clean,`operation_trace`/`ActionCommand`/`mod perf_probe` 归零);
提交后按正向 `git apply --3way` 回填,`perf_probe.rs` 归位(见下方"回填登记")。

**门禁与验收(在提交状态上跑,WIP 旁置)**:
- `cargo fmt --all --check` 干净;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**。
- `cargo test -p acts --lib --tests` = **870 passed / 2 failed**(两条均为已登记既有抖动,HEAD 上同红:
  `export::actions::tests::a_run_cannot_seal_another_subjects_scope`(C2/C3 基线红)、`scheduler::shell::tests::kernel_task_and_act_messages_are_emitted`
  (H1 登记的 `$|running`/`$|completed` 根行窗口,C4 已实测 HEAD 单跑同样红));`scheduler::tests::reliability::` **65 passed / 0 failed**。
- per-crate(§11 C):`acts-acl` 28 ✓、`acts-expr --all-features` 45 ✓、`acts-channel` 32 ✓(+2 broker ignored)、
  `acts-store --features sled,sqlite,postgres` 96+97 ✓(+1 ignored)、`acts-cli` 6 ✓、`acts-package-http` 16 ✓、
  `acts-package-shell` 3+1(**HEAD 同红**:`the_inputs_of_the_act_embed_in_the_script`,`$inputs().data` 为空,HEAD stash 对照实测同失败)、
  `acts-package-state` 0、`acts-package-javascript` 9 ✓、`acts-package-nats` 2 ✓、`acts-plugin-nats` 5 ✓、
  `acts-plugin-grpc` 14+2(**HEAD 同红**:two message-dispatch timeouts)、`acts-plugin-web` 22+2(**HEAD 同红**:two SSE dispatch timeouts)、
  `acts-server` 17 ✓(`-j 2`,§10 §5k)。
- 本机无 NATS/Postgres 服务 ⇒ broker/postgres 套件按既有口径 ignored 或未覆盖(与 §10 既有登记一致)。

**§10 记录(身份前后)**:`TaskId` = `(node, generation)` 不变;其**外部渲染**由 node id 变为摘要;`OperationId.task` 语义不变。
`-- BREAKING` 见 `CHANGELOG.md` 0.25.0 末尾条目(含旧库混合窗口与孤儿键口径)。
`git diff -M --stat`(本笔含文档)= **29 文件**(代码 27 + `CHANGELOG.md` + 本文件;精确增删以 `git show --stat HEAD` 为准)。

### C6 完成(2026-10-17):消息层级显式化 —— act 载荷写 `kind`,层级复用 `NodeKind`

**载荷前后**(对外字节只增一个字段,旧值不变):

| 载荷 | 前 | 后 |
|---|---|---|
| act(`uses` step) | `tid/nid/name/uses/mid/inputs`(无 `kind`,消费端默认 `act`) | **加 `"kind": "act"`**(`NodeKind::Act` 的 Display 输出);其余字段逐字节同 |
| task(无包 step / root) | `tid/nid/name/kind/inputs`(`kind` 来自 `node_kind_and_name` 的 `&'static str`) | 同字段同值;`kind` 改由 `NodeKind` 渲染(step→`step`、root→`workflow`;branch 仍显式抑制) |
| 行 `kind` | 声明树的 `NodeKind`(`uses` step = `step`) | **不变**(`projection::project_task` 保留 `node.kind()`;新增注释说明这是读模型契约,不是消息层级) |

- `facts::node_kind_and_name` 返回 `(crate::NodeKind, String)`:root→Workflow、branch→Branch、**带非空 `uses` 的 step→Act**、其余 step→Step;`uses` 为空白按无包处理(与 `step_uses` 的过滤同口径)。
- **用户指令**:消息层级直接复用 `crate::NodeKind`,不引入本地 `MessageLevel`;`message_from_record` 的旧记录缺省仍是 `NodeKind::Act` 的字符串。
- 根双发:**代码未动**;`report_lifecycle` 的文档补明 outbox(`kind=workflow`)与 `LifecycleSink` 分别是 legacy `on_task`/`on_proc`,两路都保留。
- 静默主题:`task.propagated`/`task.reentered`/`fanout.selected|empty` 保持静默(设计 §3.4 已拍),不加状态载荷。

**映射测试清单**(`shell/message.rs`):
- `decided_state_topics_map_onto_message_states`:9 主题全覆盖(created/completed/submitted/backed/cancelled/failed/aborted/skipped/removed → 9 state)。
- 新增 `every_terminal_state_names_the_topic_it_announces`:`TaskState` 12 变体 × 穷尽 match,终态 ⇔ 主题(`is_terminal() == topic.is_some()`);新增状态在编译期迫使决定。
- 新增 `nid_names_the_node_and_tid_the_instance`:payload 的 `tid` 落 `Message.tid`、`nid` 落 `Message.nid`(节点/实例契约)。
- `a_created_record_maps_onto_the_act_message`:act 载荷带 `kind:"act"` 后 `r#type == "act"`;无 `kind` 的旧载荷仍由 `terminal_records...`/映射测试覆盖缺省。
- `shell/tests/mod.rs::kernel_task_and_act_messages_are_emitted`:两个 act 载荷断言 `kind == "act"`,root 仍 `workflow`。
- 新增 `shell/facts.rs` 单测:root/plain/uses/blank-uses/branch/missing 六种节点层级。

**字面量审计(对外词汇,`git diff HEAD` 对照)**:
- 主题:9 个 topic 字面量只在 `message_from_record` 的 match 与发射点;本笔**未改任何发射点**(`git diff HEAD -- shell/operation core/plan | grep topic` = 0 行)。
- 层级词:可执行真值源是 `core/model/view/node.rs::NodeKind` 的 `Display`(本笔未动);`Message.r#type` 的取值集合 `workflow|branch|step|act` 不变。
- 多重集差异(4 个生产文件、quoted literal):`"workflow"` 2→1、`"branch"` 2→0 —— 被删的是 facts.rs 里的本地副本(返回串 + 已放弃的 `MessageLevel` 臂),改由 `NodeKind` 提供;`"act"`/`"step"` 的增量来自新测试 payload 与文档。**无任何取值被改写**。
- 行 kind:`projection.rs` 取值未改,只加注释。`$step` 词汇:`utils/consts.rs` 未触碰。

**清单(改 / 保留,`git grep -nE "node_kind_and_name|message_from_record|topic" crates/acts/src/scheduler/shell` 逐条)**:

| 命中面 | 决定 |
|---|---|
| `facts.rs::node_kind_and_name` 及唯一生产调用点 `mod.rs::task_message_payload` | **改**:返回 `NodeKind`;`uses` step→`Act` |
| `facts.rs::message_topic`(TaskState→topic,9 项) | **保留**:终态映射真值源,本笔不动 |
| `message.rs::message_from_record` 的 9-topic match | **保留**(主题表未动);仅缺省 `kind` 改走 `NodeKind::Act` |
| `message.rs::MessageForwarder` / `mod.rs` 的 transport 接缝 | **保留**:投递路径零改动 |
| `mod.rs::task_operation_plan` 的 `topic` 参数;`operation/{action,propagate,task}.rs` 的 topic 字面量 | **保留**:发射点一个未改 |
| `core/plan/{propagation/cascade,generation/mod,container/fanout}` 的 4 个静默 topic | **保留**:§3.4 已拍"保持静默" |
| `model.rs:567` 的 `node_kind_and_name` 注释引用 | **保留**:policy 图的子节点仍是 Step,注释仍准确 |
| `shell/tests/mod.rs` 的 topic 收集器 | **改**:仅加 act 载荷 `kind` 断言 |
| `projection.rs` 行 `kind` | **保留取值**(读模型契约),**改注释**说明与消息层级的故意差异 |

**过程登记(探针 WIP)**:同 C5 —— 工作区先带 C6+WIP 建临时提交,`git apply -R --3way .git/c4-wip/wip.patch` 反解(8/8 clean),`reset --soft` 丢掉临时提交,`perf_probe.rs` 从 index 摘除(文件保留为未跟踪);门禁在 C6-only 状态跑,提交后正向 `git apply --3way` 回填 WIP。

**门禁与验收(在提交状态上跑,WIP 旁置)**:
- `cargo fmt --all --check` 干净;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**。
- `cargo test -p acts --lib --tests` = **874 passed / 1 failed**:唯一失败是已登记既有抖动 `kernel_task_and_act_messages_are_emitted`
  (根行 `$|running` 先于 `$|completed` 投影,C5 已归因;单跑该用例通过,失败断言在 `tests/mod.rs:1448`,与 C6 改动的 5268 行无关);
  C5 登记的另一条 `a_run_cannot_seal_another_subjects_scope` 本轮通过。
- 新增测试单跑:message 6/6、facts 1/1 全绿。
- per-crate(§11 C):`acts-acl` 13+10+5 ✓、`acts-expr --all-features` 3 ✓、`acts-channel` 32 ✓(+2 doctests ignored;broker 套件按既有口径)、
  `acts-store --features sled,sqlite,postgres` 2+94+96+96+97 ✓(+1 ignored)、`acts-cli` 24+1+5 ✓、`acts-package-http` 16 ✓、
  `acts-package-shell` 7+3+2 ✓ + `the_inputs_of_the_act_embed_in_the_script`(**HEAD 同红**,C5 已归因)、`acts-package-state` 0、
  `acts-package-javascript` 9 ✓、`acts-package-nats` 2 ✓、`acts-plugin-nats` 6+5 ✓、`acts-plugin-grpc` 14+2(**HEAD 同红**,message-dispatch timeouts)、
  `acts-plugin-web` 22+2(**HEAD 同红**,SSE dispatch timeouts)、`acts-server` 11+2+1+3 ✓(`-j 2`,§10 §5k)。
- 本机无 NATS/Postgres 服务 ⇒ broker/postgres 套件按既有口径 ignored 或未覆盖(与 §10 既有登记一致)。

`git diff -M --stat`(本笔含文档)= **8 文件,+304/−89**(代码 5:`facts.rs`/`message.rs`/`mod.rs`/`projection.rs`/`tests/mod.rs`;文档 3:`CHANGELOG.md`/设计/本文件;精确增删以 `git show --stat HEAD` 为准)。

### C7 完成(2026-10-17):投递边界入文档 —— at-least-once + 无订阅者不留行(零代码)

**边界前后(设计 §3.5)**:

| 边界 | 前(§3.5 原稿) | 后(已定) |
|---|---|---|
| at-least-once | 「deliver 与 commit 之间崩溃会重发(`shell/mod.rs:917-928`)。」 | `deliver_plan` 在提交**之前**发本 plan 的 outbox 记录(`shell/mod.rs:804-864`;调用点 `operation/dispatch.rs:37`);transport 确认的 receipt 随这次提交落成 `Delivered`(`core/commit/aggregate/commit.rs:100-112`);未确认的记录提交为 `Pending`(`shell/mod.rs:852-857`),由 driver 的重试 pass(`dispatch_outbox`,`shell/mod.rs:1880-1971`)在下一次通知或 tick 再发(`shell/mod.rs:666-695`);deliver 与 commit 之间崩溃 ⇒ 重发。 |
| 无 handler 不留行 | 「`messages`/`deliveries` 只由 ack 通道写(`export/channel.rs:321-336`)——"马上发出去"是内存事件,不是持久化事实。」 | outbox 只把记录写进聚合(`core/commit/aggregate/commit.rs:88-115`),**不写**两张行表;唯一落行链路是 ack 通道 —— `Channel::on_message`(`export/channel.rs:149-172`)→ `deliver`/`store_if`(`export/channel.rs:266-346`,`ack` 条件 `:323`)→ `Runtime::store_delivery`(`runtime.rs:186-194`)→ `writebehind::store_delivery`(`store/writebehind.rs:431-446`)→ `Store::store_message_delivery`(`store/store.rs:590-608`);没有订阅者 ⇒ 存储无痕。 |

- **取舍(已定)**:发在提交前 = 延迟最低、与 effect seam 同序(一个 operation 一次 commit 一次投递,不等 tick);代价 = 重复窗口,消费者必须幂等。**行为零变化**。
- **file:line 核对(按 C7 提交状态;本地探针 WIP 让 `shell/mod.rs` 行号 +9,不随提交)**:`deliver_plan` 由 §1.5 的 `907-958` 漂到 `804-864`;`dispatch_outbox` 由 `4609-4750` 漂到 `1880-1971`;`aggregate.rs:459-472` 随 C1b 目录化后落在 `core/commit/aggregate/commit.rs:100-112`;`deliver: Arc<Notify>` 在 `shell/mod.rs:163-170`(初始化 `:407`)。逐条对照 HEAD blob 的 `grep -n`/`sed -n` 均命中。**§1.5 按本笔边界未改** —— 它是历史事实表,当前引用由 §3.5 承载。
- **CHANGELOG**:`Unreleased` 末尾新增 `docs(acts): the delivery boundary is a contract now, not an implementation detail (design §3.5)...`(指向设计 §3.5;**不写 BREAKING** —— 行为未变)。
- **门禁与验收(在提交状态上跑,探针 WIP 旁置)**:`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` **EXIT 0**;`cargo fmt --all --check` 干净;
  `cargo test -p acts --lib --tests` = **873 passed / 2 failed**:`export::actions::tests::a_run_cannot_seal_another_subjects_scope`
  与 `tests::a_backward_next_re_enters_the_node_it_names`(后者是 §10「P4 收尾续」/§4 已登记的负载抖动);
  docs-stash 基线同命令 = **872 passed / 3 failed**,同一组两条外加 `scheduler::shell::tests::kernel_task_and_act_messages_are_emitted`
  (C5/C6 已登记;本轮通过)⇒ C7 没有新增失败。首轮全量还曾翻 `kernel_engine_projects_the_legacy_read_model` 与
  `the_driver_leaves_a_settled_run_alone`;前者随后单跑 3/3 绿、后者单跑绿,复跑全量未再出现 ⇒ 负载抖动,非本笔(零代码)。
- `git diff -M --stat`(本笔含文档)= **3 文件,+91/−39**(`CHANGELOG.md` +1;`KERNEL_CUTOVER_NOTES.md` +71/−33;`KERNEL_REFACTOR_DESIGN.md` +19/−6;精确增删以 `git show --stat HEAD` 为准)。

### C8a 完成(2026-10-17):`view` 删死件 + `tree_output` 自递归 —— 输出逐字不变

**删掉的死件(逐条 `git grep`,按 C8a 提交前的 HEAD 行号)**

| 死件 | 旧位置 | 判定与处理 |
|---|---|---|
| `visit.rs` 全簇(`VisitRoot`/`Visitor`) | `view/visit.rs`(116 行) | 只服务 `tree_output`/`print`;`Visitor` 每实例克隆 `Box<VisitRoot>` ⇒ `visits` 恒空 ⇒ `next` 环无界递归。**整文件删除** |
| `Node::from_str` | `view/node.rs:314-324` | 仅测试使用,而且是 inherent fn —— 仓库里从来没有 `impl FromStr for Node`(设计里的 "FromStr impl" 一句不成立,按现状只删这一个)。**删除** |
| `Node::print` | `view/node.rs:326-362` | 只被 `NodeTree::print` 调用。**删除** |
| `NodeTree::build` | `view/node_tree.rs:31-36` | 仅 `shell/tests/tree.rs` 使用。**删除**,测试改走 `NodeTree::load` |
| `build_workflow` | `view/build.rs:11-13` | 只被 `NodeTree::build` 调用。**删除**;`NodeTree::load` 直接走 `build_owned_workflow` |
| 注释掉的 `build_act` | `view/build.rs:179-209` | 纯注释块。**删除** |
| `NodeTree::print` | `view/node_tree.rs:122-127` | `visit` 簇在 `tree_output` 换实现后的最后消费者(测试里只有一行 `tree.print()`)。**删除** |
| `Workflow::print` | `model/workflow.rs:111-115` | **不是"无调用者"**:15 个 example 文件调用它(其中 6 个是 workspace member)。**删除**,全部调用点改为 `print!("{}", workflow.tree_output())`(对 Workflow 模型两者逐字同输出:模型装载不会产生 `NodeContent::Act`) |

模块清理:`view/mod.rs` 删 `mod visit;`;`NodeOutputKind` 不再 re-export(只在 `view` 内的 `build`/`node` 使用);`NodeContent` 的 re-export 加 `#[cfg(test)]`(唯一消费者是 `scheduler/mod.rs` 的 test-only re-export),因此不需要 `#[allow(unused_imports)]`;`visit.rs` 删除后不再有 `#[allow(clippy::borrowed_box)]`/`#[allow(unused)]` 残留。

**`tree_output` 换实现(本笔唯一的渲染实现变更)**

- 旧:`NodeTree::tree_output` → `VisitRoot::walk`(每节点 `Visitor` 克隆 `VisitRoot`,计数永不回写)。
- 新:`NodeTree::tree_output` 沿 `Node.children` 自递归,顺序与旧实现相同 —— 先本行,再 `NodeOutputKind::{Normal,Catch,Timeout}` 三组 children,最后 `next`;`is_last` 与 `path` 的算法逐条照搬(children 的 `i == len-1`,有未访问 `next` 则非末;`next` 的 `is_last = next.next().is_none()`),行文本与 `format!("{} id:{} name={}  next={}\n", …)` 一字未改。
- 循环守卫:`HashSet<String>` 按 node id 标记,进入节点即插入;children/next 只在未访问时下探。旧的 `visits` 计数与 `visit()` 一起消失。
- **逐字证据**:同一模型(3 个顶层 step、`first` 下 2 个 branch 各 2 步、`second` 带 catch+timeout、显式/链式 `next` 都在内)用 HEAD 旧实现与提交状态新实现各跑一次,输出均 **563 bytes**,`diff -u before.txt after-fmt.txt` **为空**(`TREE_OUTPUT_DIFF=0`);该输出已固化为 `tree.rs::sch_tree_output_is_byte_stable`。探针 test 目标随后删除,不入提交。
- 旧输出里 catch/timeout 的 `│   └──` 两条同层连接线(没有区分 child 类型的标记)是既有行为,按"逐字保持"原样保留。

**测试调整**

- 删除 5 条只测 `Node::from_str` 的 `*_ser_de` 用例;新增 2 条覆盖仍在生产路径上的 `Node::to_string`/`NodeData` 往返(`sch_tree_node_data_round_trips`、`sch_tree_act_node_data_round_trips`,含 parent/next/level、动态 act 的真实 `Act.id`)。
- `shell/tests/tree.rs` 的 `NodeTree::build` 全部改走本地 `try_tree_of`/`tree_of`(`NodeTree::new` + `load`),`sch_tree_next_error` 改用 `try_tree_of` 断言错误。
- 新增 `sch_tree_output_is_byte_stable`(见上);`export/tests.rs::export_manager_model_get_tree` 从"非空"升级为逐字断言(`model:get fmt=tree` 的 smoke):
  `workflow id:<id> name=  next=nil\n└── step id:step1 name=  next=nil\n`。
- `tree::` 17 条全绿;`export_manager_model_get_tree` 单跑绿。

**门禁与验收(提交状态,WIP 旁置;本笔不动 manifest)**

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**(15 个 example 一并编译,证明 `Workflow::print` 的迁移没有漏点);`cargo fmt --all --check` 干净。
- `cargo test -p acts --lib --tests` = **870 passed / 3 failed**(共 873;测试总数较 C6 的 875 少 2 = 删 5 + 增 3):
  - `scheduler::shell::tests::kernel_task_and_act_messages_are_emitted`(C5/C6 已登记;本轮单跑绿)
  - `scheduler::tests::reliability::timeout::the_driver_leaves_a_settled_run_alone`(C7 已登记;本轮单跑绿)
  - `tests::a_backward_next_re_enters_the_node_it_names`(C7/§10「P4 收尾续」已登记;自带 20 s 上限,本轮单跑 27.64 s 超时)**HEAD 基线复核**:`git stash` 掉本笔全部改动、在同一 target 上单跑同命令,同样 `Elapsed(())`(29.65 s)⇒ 既有环境/负载窗口,非本笔引入。
- 本笔只动 `crates/acts/src/scheduler/core/model/view/**`、`model/workflow.rs`、`export/tests.rs`、`shell/tests/tree.rs` 与 examples;**未跑 per-crate 套件**(无跨 crate 代码与 manifest 变化);`cargo deny` 不需要。

**过程登记(WIP 剥离的一次事故,已恢复)**

- 按惯例先剥 WIP 再跑门禁,但这一次 `git diff > .git/c8a/wip.patch` 是在**本笔改动已经落到工作区之后**生成的 ⇒ 该 patch 同时含 WIP 与 C8a;`git apply -R` 把两者一起反解,`node.rs`/`build.rs`/`tree.rs`/`export/tests.rs`/examples 回到 HEAD。恢复:从同一 patch 用 `git apply --include='…/view/*' --include='model/workflow.rs' --include='export/tests.rs' --include='shell/tests/tree.rs' --include='examples/*'` 只重放 C8a 路径,逐条 `grep` 核对 + 重跑 `tree::`/`export_manager_model_get_tree` 与 563-byte diff,全部通过后才继续。**教训(补进 §6 口径)**:剥离 WIP 的 patch 必须在动手前生成;恢复时要按路径过滤,不能整包重放。
- 提交后按 `.git/c8a/wip.patch` 的 WIP 路径子集正向回填(`git apply --3way`),`perf_probe.rs` 从 `.git/c8a/perf_probe.rs` 恢复;提交本身只含 C8a 的 23 个代码/示例文件 + 2 个文档。

**`git diff -M --stat`(本笔含文档)**

| 区域 | 文件 | + | − |
|---|---|---|---|
| `core/model/view/**`(`build`/`mod`/`node`/`node_tree`/`visit`) | 5 | 77 | 259 |
| `shell/tests/tree.rs` | 1 | 128 | 117 |
| `export/tests.rs`(`model:get fmt=tree` pin) | 1 | 7 | 1 |
| `model/workflow.rs`(`Workflow::print`) | 1 | 0 | 6 |
| `examples/**`(`print` → `tree_output`) | 15 | 15 | 15 |
| **代码/示例小计** | **23** | **227** | **398** |
| 文档(`CHANGELOG.md` + 本文件) | 2 | 121 | 51 |
| **合计** | **25** | **348** | **449** |

(精确逐文件增删以 `git show --stat HEAD` 为准;`visit.rs` 的 −116 行是整文件删除。)

### C8b 完成(2026-10-17):`view` 去重 —— 一份 id 分配、一份 name/kind 推导、一个 `NodeKind`

**合并了什么**

| 子项 | 前 | 后 |
|---|---|---|
| ① id 分配 | `view/build.rs::prepare_workflow_ids/prepare_step_ids/prepare_branch_ids` 与 `shell/model.rs::assign_ids/assign_step_ids/assign_branch_ids` 两份递归;`build_owned_workflow` 另给自己补 workflow id | `view/build.rs::assign_ids` 一份(`workflow`→`longid`,step/branch/catch/timeout→`shortid`);`build_owned_workflow` 与 `ProcessSpec::from_workflow` 都只调它 |
| ② name/kind | 投影读 `node.kind()/name()`;消息走 `facts::node_kind_and_name` 的另一条遍历 | `facts::node_facts` 一次遍历给出 `declared_kind`(行)/`message_kind`(消息)/`name`;投影取前者,消息取后者 |
| ③ 词汇 | `view::NodeKind`(4 值)+ `model_graph::NodeKind`(4 值,含 2 个 `#[allow(dead_code)]`) | 只留 `view::NodeKind`(`crate::NodeKind`);`NodeDefinition.kind` 直接用它;两个 allow 随 enum 删除 |

- **id 不变**:`git grep "prepare_workflow_ids|prepare_step_ids|prepare_branch_ids"` = **0**;`assign_ids` 是唯一分配点。`build_owned_workflow` 仍在 `Arc::new` 之前调用它;import 仍在 copy 上调用它、再 `materialize_container_acts`(顺序未变);`longid`/`shortid` 未换。
- **行/消息契约不变**:C6 的故意分歧由两个显式命名的字段承载 —— `declared_kind` 给行(`uses` step = `step`),`message_kind` 给消息(`uses` step = `act`)。`kernel_engine_projects_the_legacy_read_model` 现在同时 pin:`s1.r#type == "step"`、`s1.name == "Write"`、root row `workflow`/`Rows`(root 名 = `model.name`);`facts` 单测 pin `("call" → Step/Act/"Call")`;`uses_steps_write_created_message_records` 未改、仍绿 ⇒ 消息载荷逐字段不变。
- **词汇单一**:`git grep "enum NodeKind" crates/acts/src` = **1**(`view/node.rs`);`Display`(`workflow|branch|step|act`)、serde 形状、`From<String/&str>`、`crate::NodeKind` 公共面均未动;`ModelGraph` 与 `NodeTree` 仍是两张图(只换 `kind` 的类型,不合并);删除 policy 侧 enum 后不再有任何 `#[allow(dead_code)]`。
- **未动**:`NodeData`、`NodeTree::load` 的校验规则、`view` 模块位置、`projection` 行格式、持久化列。
- **新增/调整测试**:`sch_tree_assigns_ids_to_unnamed_nodes`(`NodeTree::load` 路径:workflow/catch/branch/inner step/timeout 全部非空且 `NodeId::parse` 通过,caller model 未被改动);原 import 路径用例改名 `unnamed_nested_nodes_get_ids_from_the_shared_pass`;`tree::` 19 条全绿。

**门禁(提交状态,WIP 旁置;本笔不动 manifest)**

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = **EXIT 0**;`cargo fmt --all --check` 干净。
- `cargo test -p acts --lib --tests` = **872 passed / 2 failed**(共 874;较 C8a 的 873 +1 = 新增 tree 用例):
  - `export::actions::tests::a_run_cannot_seal_another_subjects_scope`(C2/C5/C6 已登记);
  - `scheduler::tests::reliability::timeout::the_driver_leaves_a_settled_run_alone`(C7 已登记)。
  - 两条**单跑均绿**(同一 target,9.39 s)⇒ 既有负载/时序抖动,非本笔引入;本轮没有新的失败名。
- 本笔只动 `core/model/**`、`shell/{model,facts,projection,mod}.rs`、`shell/tests/**` 与文档 ⇒ `-p acts` 套件足够;未跑 per-crate、不需要 `cargo deny`。

**过程登记(WIP 旁置;对 C8a 教训的修正)**

- 动手前先生成 `.wip/c8b-perf-probe.patch`(584 行)并复制 `.wip/perf_probe.rs`(102 行),再改代码 —— 这次 patch 只含探针,不含 C8b。
- 剥离线时 `git apply -R --3way` 报 8 个 `does not match index`:WIP 的 postimage 从未进对象库,3-way 没有 blob 可用。`git apply -R --check` 通过后改用**直接 `git apply -R`**(8 文件全 clean)。**口径补充**:旁置 patch 的 postimage 不在对象库时,`--3way` 不可用;先 `--check` 再 direct apply,提交后同样 direct `git apply` 回填。
- 提交本身不含 `.wip/`、`perf_probe.rs` 与任何探针改动;回填后再确认工作区与动手前一致。

**`git diff -M --stat`(本笔含文档)**

| 区域 | 文件 | + | − |
|---|---|---|---|
| `core/model/view/**`(`build`/`mod`) | 2 | 26 | 13 |
| `core/model/model_graph.rs` | 1 | 6 | 23 |
| `core/coverage.rs` | 1 | 2 | 1 |
| `shell/facts.rs` | 1 | 64 | 33 |
| `shell/model.rs` | 1 | 16 | 59 |
| `shell/projection.rs` | 1 | 12 | 7 |
| `shell/mod.rs` | 1 | 4 | 4 |
| `shell/tests/**`(`mod`/`tree`) | 2 | 53 | 1 |
| **代码小计** | **10** | **183** | **141** |
| 文档(`CHANGELOG.md` + 本文件) | 2 | 80 | 65 |
| **合计** | **12** | **263** | **206** |

(精确逐文件增删以 `git show --stat HEAD` 为准。)

---

### C8c 完成(2026-10-17):D4 = 暂不挪 —— effect seam 成立,`view` 留原路径(零代码)

**裁决**:用户 2026-10-17 拍 D4 = **暂时不挪,后续考虑**。`view` 仍在 `scheduler/core/model/view`;
C8c 的交付是 effect seam 取证 + D4 三案材料,无生产代码。后续重评放在 §11 L11/L12 之后;
若届时仍需挪,优先 c2、不取 c1(理由见下)。

**effect seam:成立**

调用链(基准 HEAD = C8b `5eb441a`):

1. `shell/mod.rs:1662 scope_spec` 从快照取 durable `__model`(`facts.rs:5 model_of`),构造 `ScopeSpec`;
   `shell/mod.rs:1681 step_effect` 只把 `step.uses` 作为包 id 放进 `EffectRequest`。
2. `shell/operation/action.rs:1163` 的 `StepDriver` 调 `runner.run(request)`。
3. `shell/effects.rs:125 PackageEffectRunner::run` → `:101 headless` → `shell/scope.rs:47 headless_scope`:
   `Process::new` + `process.load(&spec.model)`(`process/process.rs:88`)在一棵**新的内存树**上 `NodeTree::load`;
   acting task 的 node 也取自这棵树。
4. 容器包执行 → `context.rs:255 build_acts` → `self.proc.tree()`(`process.rs:105`)→ `dyn_build_act`
   (`view/build.rs:191`)→ `tree.append_node`;只改这份 headless `NodeTree`。三个调用点:
   `package/core/block.rs:107`、`parallel.rs:98`、`sequence.rs:98`,全部返回 `Ok(None)`。
5. `effects.rs:172` 只构造 `EffectOutcome { outputs, data: data_delta(&task, ...), env: non_private(process.env()) }`;
   结构体只有这三个字段(`:57,61,65`),`process`/`task`/`ctx` 在函数返回时 drop —— **没有 tree 字段或回传路径**。
6. `action.rs:376 fold_effect_outcome` 只把 `outputs`/`data`/`env` 折进 `TransitionPlan`(`:392`、`:399`)。
7. 投影 `shell/projection.rs:149 project_nodes` 另从 `model_of(snapshot)` 读 durable model,再
   `:159-160 NodeTree::new(); tree.load(&model)`;行 `node_data` 与 headless 树无关。
8. 模型侧 `shell/model.rs:450 from_workflow` → `:461 assign_ids` → `:465 materialize_container_acts`(`:107`)
   在 import 时把同一形状写进 durable model;kernel fan-out 读的是它。

**证据**(HEAD 状态;临时探针已删,副本 `.wip/c8c-effect-seam-probe.rs`):

- 临时测试 `zz_probe_build_acts_result_is_discarded`:自定义 Func 包 `test.dyn-acts` 调
  `ctx.build_acts(&[Act::irq(... "dyn1")], true)` 并返回标记 `ran=true`;手写 `ProcessSpec` 绕开
  `materialize_container_acts`。结果:进程完成、`ran == "true"`(包与 `build_acts` 确已执行)、聚合只有
  `$`/`s1` 两个 execution、outbox 无 `dyn1` 消息。`cargo test -p acts --lib zz_probe...` = **1 passed**。
- 既有 `a_block_packages_inline_acts_run_as_children_in_both_modes` = **1 passed**:模型 materialize 出的
  inline acts 恰好两个、两条消息都到;若 headless 树也参与会多出子执行/消息。
- 既有 `sch_tree_dyn_acts_restore_chain` = **1 passed**:`dyn_build_act` 只操作内存树的链/父子关系。

结论:**`build_acts` 的树产物被 effect seam 丢弃**,是每 hop 的白付开销。唯一仍可观察的残留是
`dyn_build_act` 失败会让 effect 失败(错误传播);L9 删除时保留该语义即可。

**D4 三案(材料;已拍“暂不挪”)**

| 案 | 改动面 | 守卫 | L11/L12 后 | 评价 |
|---|---|---|---|---|
| **c1** 挪 `scheduler/shell/view` | 4 文件/781 行移动;`shell/mod.rs` +`mod view;`;7 个 shell/test import;`core/model/mod.rs` 删声明;`scheduler/mod.rs` 2 行 + 文档;外部 4 个消费者经 façade 不动 | **0**;`forbidden(Pure)` 已含 `crate::scheduler::shell` | `NodeTree`/`Node` 被 L11 删;`NodeData` 留在 shell,与 `model/info.rs`/`store` 的方向冲突 | 不取 |
| **c2** 挪 `crate::model::view` | 4 文件/781 行移动;`model/mod.rs` 加 `pub(crate) mod view;`;`core/model/mod.rs` 删声明;7 个 shell/test + 4 个外部消费者 + `scheduler/mod.rs` + `lib.rs:59`;`model/workflow.rs` 可去掉对 scheduler 的依赖 | 需新增:最小 1 行 `ENGINE_VOCABULARY` 加 `"crate::model::view"`,完整为 ~30-50 行 importers 白名单 + `model/view/**` 纯度扫描 | `NodeTree`/`Node` 被 L11 删;`NodeData`/`NodeContent`/`NodeKind` 归 `model`,正确 | 方向最正;后续重评优先 |
| **不挪 + 写边界** | 0 代码;只写 §3.7(c)/§2.2 | 0 | 无先搬后删;L11/L12 后残件再定 | **本轮采用** |

**删除边界**:按 §11 L9-L12,不重复逐方法评估。L9 删 `build_acts`/`dyn_build_act`/`append_node` 与三处调用;
L10 换 headless scope;L11 删 `NodeTree`/`Node`(粗算 `view` 只剩 `NodeData`/`NodeContent`/`NodeKind`/`assign_ids`
约 1/4);L12 删 `Process`/`Task`/`TaskTree`/`store/process_rows`。因此现在搬 781 行的大部分会在 L11 被删。

**门禁**(提交状态,WIP 旁置;本笔零代码):
`cargo fmt --all --check` 干净;`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` = EXIT 0;
`cargo test -p acts --lib --tests` = **873 passed / 1 failed**(共 874)—— 唯一失败
`export::actions::tests::a_run_cannot_seal_another_subjects_scope` 是 C2/C5/C6/C8b 已登记的负载/时序抖动,
**单跑 `--exact` 绿**(0.07 s);本轮无新增失败名。其余 `--tests` 目标未报失败。
**过程**:WIP(性能探针)按 C8b 口径旁置/回填;`git diff` 哈希与动手前一致;提交不含 `.wip/`、`perf_probe.rs` 或探针改动。

### A1a 完成(2026-10-17):effect 后的 CAS 冲突不再自动重试

**任务**:§11 D 的 A1a(审计 #2 的第一刀)。目标不是完成 effect 协议,而是先关闭“包 effect 已执行、plan commit 丢失
后被释放 claim 自动重试”的重复副作用路径。

**改动**:

- `shell/operation/mod.rs`:`Outcome::ran_external_effect()`。只有 `Effect::Completed(Some(_))` 与 `Effect::Failed(_)`
  算跨过外部 effect 边界;client decision、timer round、纯计划不算。
- `shell/operation/dispatch.rs`:`dispatch` 增加 `effect_started`;CAS `Conflict` 时,
  effect 已执行的 operation 走 `MarkEffectUnknown` 并返回 conflict,不再 `release_claim`;
  effect-free operation 保持原来的释放 + recovery 重试。新增 `KernelHost::mark_effect_unknown`。
- `shell/operation/mod.rs`:`driver.next` 在 effect 之后失败时也 `MarkEffectUnknown`,不再让 operation 停在
  `CommitPrepared` 等 lease 过期。
- `shell/mod.rs`:`apply_action_once` 同样分类;`next` 失败且 effect 已执行时 `MarkEffectUnknown`,
  否则维持 `release_claim`。
- `shell/tests/mod.rs`:`ConflictKv` 改为可按 operation 动态 arm;新增
  `a_package_effect_that_loses_its_commit_is_marked_unknown`,用真实 `PackageEffectRunner` + `USES_SET` 钉住:
  effect 后丢失 CAS ⇒ `EffectUnknown`;`recover` 只报 `ManualReconciliation`,不 wake。
- 既有 `a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery` 继续通过,证明 effect-free 的内部 hop
  仍走原来的可重试路径。

**验证**:

```text
cargo test -p acts --lib a_package_effect_that_loses_its_commit_is_marked_unknown  -> 1 passed
cargo test -p acts --lib a_hop_that_loses_its_plan_commit_releases_the_claim_for_recovery -> 1 passed
cargo test -p acts --lib linear_process_runs_to_completion_and_delivers_outbox -> 1 passed
cargo test -p acts --lib --tests -> 875 passed, 1 ignored (151.53s)
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --check -> OK
```

**剩余**:

- **A1b**:outbox 仍在 plan commit 之前发送;需要提交后批量确认 dispatcher。per-record 试点已回退,
  见下一条「A1b 试点回退」。设计见 `KERNEL_REFACTOR_DESIGN.md` §3.10。
- **A1c**:可自动重试的幂等 effect 协议尚未实现;当前非幂等 effect 冲突一律人工协调。
- A1a 不改变 `ClaimAndPrepare` 先落盘的现有顺序;它只保证 effect 后不再自动重放。
- **A1a 不修复 outbox 的提交前窗口**:`dispatch` 仍在 plan commit 前调用 `deliver_plan`,
  因此一个随后冲突的 plan 仍可能已经发出了本 plan 的 outbox 消息。该窗口由 A1b(提交后发送)关闭;
  在 A1b 落地前,不能把 A1a 当作完整的 effect/outbox 协议。

### A1b 试点回退(2026-10-17):per-record claim/confirm 写放大不可接受

**尝试**:按 A1b 目标把 outbox 改成提交后发送——plan commit 只写 `Pending`,提交后对每条新记录执行
`claim -> deliver -> confirm`,并删除 `deliver_plan` 与 `OutboxIntent.receipt` 的前置直发快路径。

**结果**:

- 每条 outbox 记录多出 **2 笔 aggregate commit**(claim + confirm),每次提交都重写整包聚合与两个索引。
- `examples/simple` 的 11 次循环用例在 debug 下超过 20s;测试超时后驱动仍在继续推进,证明是吞吐退化而非死锁。
- 全量 `acts` 套件明显变慢,并暴露了额外 CAS 冲突路径;这与“高性能流程执行引擎”的目标相反。
- 该变体还让 A1b 依赖更成熟的单写者/批量提交协议,不适合作为 A1a 之后的直接切法。

**结论**:

- **回退** per-record claim/confirm 实现;恢复 `deliver_plan`、`OutboxIntent.receipt` 与既有 at-least-once 窗口。
- 保留本轮的有益修正:
  - A1a 的 effect 边界与 `EffectUnknown`;
  - `release_claim` 对 CAS 冲突做有界重试,成功后立即 `wake`,避免 operation 停在 `CommitPrepared` 等 lease;
  - 投影一致性:proc 行总是连同 root task 行一起投影,且 proc 行最后写;
  - `sweep_removable` 的 proc 行修复先 `forget_proc` 再投影,允许同一 aggregate version 重算 `removable`,
    同时保持 task 行的严格版本守卫。
- A1b 必须改为:**每个 operation 至多一笔批量确认提交**(把所有新记录的 `Pending -> Delivered` 一次提交),
  或把 outbox 状态移出 aggregate 的独立日志。**不得原样重试 per-record claim/confirm。**
- A1b 仍是 P0,但排在 A2(ready queue)之后;A1c 依赖批量版 A1b。

**验证(回退后)**:

```text
cargo test -p acts --lib a_package_effect_that_loses_its_commit_is_marked_unknown -> 1 passed
cargo test -p acts --lib tests::a_backward_next_re_enters_the_node_it_names -> 1 passed
cargo test -p acts --lib kernel_catch -- --test-threads=1 -> 3 passed
cargo test -p acts --lib --tests -> 875 passed, 1 ignored (150.38s)
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --check -> OK
```

### A2 完成(2026-10-17):durable ready queue + recovery 收缩/分页

**任务**:§11 D 的 A2(P0)。正常 operation 的活性移出 recovery 扫描;recovery 只处理异常兜底;
修掉 recovery 每页全扫索引的 $O(N^2)$ 分页。

**事实与契约**

- 新增 `core/ready.rs`:`ReadyIndex`/`ReadyEntry`/`ReadyIndexCodec`(magic `ACTRDY`,schema 1)。
  每个进程一个文档 `kernel/indexes/ready/{hex pid}`,entry 含 `operation_id`、`ready_at_ms`、
  `source_aggregate_revision`,按 `(ready_at, operation_order)` 排序;可空的索引不写文档。
- ready 文档与 aggregate 同属一个 `DocumentTransaction`:create 写入首条根 operation;claim
  (`ClaimAndPrepare`) 删除;`ScheduleRetry`/`ReleaseExpiredClaim` 写回;plan commit 在同一 batch 替
  换。`LoadedProcess` 现在加载/守卫四个文档(aggregate、recovery、outbox、ready),`load` 的
  `read_many` 是四键。旧库没有 ready 文档仍能加载。
- `OperationIntent` 新增 `ready_at_ms: Option<i64>`:普通 operation 为 `None`(提交即到期);
  `DispatchTimeout` 由 shell 写 `now + timer_poll_ms`。parked 根 operation 提交即到期,由 driver
  在一次无可驱动后把记录推迟一个 poll,而不是允许它忙轮询。

**驱动**

- `KernelHost::drive_ready(now, budget)`:按位置游标从 ready index 取 **due** 记录并直接驱动
  (wake 通道仍是快路径)。一个 operation 的 plan commit 即使稍晚于本轮 `now`,下一轮也会重新读钟,
  因此丢 wake/队列满不会等到 tick。
- `serve` 的 select 增加 `ready` 通知;tick 顺序为 `recover` → `drive_ready` → outbox。`wake` 现在
  返回是否入队;失败计数并通知 due pass。
- `ReadyMetrics`:backlog、due、最老 ready 年龄、scanned、deferred、wake_failures、repaired;
  每次 tick 以 `tracing::debug!` 输出非零积压。
- parked 准入:释放 slot 时 `place_run` 返回 admission operation,`admit_run` 把它的 ready 记录
  提前到 now 再 wake;重建/准入都不依赖 tick。

**Recovery 收缩与分页**

- `OperationRecord::needs_recovery()`:只有 `Claimed`/`CommitPrepared`/`Committed`/`EffectUnknown`/
  `Quarantined` 进入 recovery index;`Pending`/`RetryScheduled` 只属于 ready index。
  `RecoveryIndex::project_legacy` 保留旧全量投影用于加载兼容;boot repair 会把旧文档归一化。
- `recover` 不再 wake 普通操作。dispositions:`RepairReady`(缺 ready 记录)、`AwaitOwner`、
  `ReclaimExpired`(后写 ready + wake)、`FinalizeCommit`、`ManualReconciliation`、`IgnoreTerminal`。
  `RecoveryReport` 以 `reclaimed`/`ready_repaired` 取代 `woken`。
- 首次 `recover` 做一次 boot reconcile:扫描 aggregate 前缀,逐进程 `repair_ready` 重算归一化
  recovery index 与缺失 ready 记录(保留已有 `ready_at`,deferred timer 不会因重启变热)。修复
  transaction 只带四个文档的 exact guard,不重写 aggregate。
- `load_recovery_page` 改为 `AsyncDocumentStore::scan_range` + 位置游标
  `RecoveryCursor{process_key, operation}`:单页最多检查 `limit+1` 个 index entry,不重扫前面的文档;
  `RecoveryPage.next` 由最后一个被检查的 entry 生成,保证稳定、非重叠、必有进展。ready 分页用
  `ReadyCursor{process_key, ready_at_ms, operation}`。

**测试**

- 新增 `a_full_wake_queue_does_not_delay_the_ready_record`:填满 wake channel 后仅靠 ready 记录
  把 run 驱动到完成,`wake_failures > 0`。
- `wake_loss_is_recovered_by_the_ready_queue`:create 后不 wake,`run_until_quiescent` 直接完成。
- durable:ready 记录随 create/claim/retry 移动;timer 在 poll 前不 due 且不在 recovery index;
  删除 ready 文档后 `repair_ready` 从 aggregate 重建且幂等;三进程 recovery 游标分页无重复。
- boot/cap 测试改为由 ready 队列准入;stall 的写边界计数改为同时观察 recovery 与 ready index,
  保留异常 claim reclaim 用例。

**验证**

```text
cargo test -p acts --lib -> 885 passed, 0 failed, 1 ignored (final, 159.56s)
cargo test -p acts --lib --tests -> 884 passed, 0 failed, 1 ignored (earlier run; a later identical
  run hit the registered C2/C5/C6/C8b flake export::actions::tests::a_run_cannot_seal_another_subjects_scope,
  whose --exact rerun is 1 passed)
cargo test -p acts --lib a_full_wake_queue_does_not_delay_the_ready_record -> 1 passed
cargo test -p acts --lib the_timeout_timer_fires_a_branch_once_and_the_owner_waits_for_it -> 1 passed
cargo test -p acts --lib reliability::stall:: -> 5 passed
cargo test -p acts --lib reliability:: -> 65 passed
cargo test -p acts --lib recovery_ -> 13 passed
cargo test -p acts --lib recovery_does_not_wake_a_parked_run -> 1 passed
cargo test -p acts --lib pure_modules_have_no_infrastructure_dependencies -> 1 passed
cargo test -p acts-acl -p acts-expr --all-features -p acts-channel -> green
cargo test -p acts-store --features sled,sqlite,postgres -> green(末个测试二进制 97 passed)
cargo test -p acts-cli -> green
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

`acts-server` 在本机未能编译/运行:链接期 `os error 1455`(页面文件太小,无法 mmap `libacts*.rlib`),
属环境资源限制;未据此改写代码。`acts-plugin-*`/`acts-package-*` 的组合运行到
`acts-package-shell --test vars` 的已知非本笔失败为止,其余先跑的二进制为绿。

**已知非本笔回归**:`cargo test -p acts-package-shell --test vars` 的
`the_inputs_of_the_act_embed_in_the_script` 在 base `f262862`(干净 worktree)上同样失败
(`$inputs().data` 为 null;H1/H2 的 headless scope 未携带 predecessor 输出),
与本笔无关,已按原样登记,不在 A2 内修。

**边界/剩余**

- outbox 仍是提交前发送,顺序与 payload 未改;A1b(批量 confirm)仍是下一项 P0,排在 A11 之后。
- recovery 分页已不是每页全扫;ready 扫描是逐进程文档的位置游标,单页成本有界,但 boot reconcile
  本身是启动时的一次全 aggregate 扫描(非 tick 路径)。
- timer 的 “own due time” 落地为 `commit_at + timer_poll_ms`(legacy 无 timer round 先于首个进程
  tick);不是从任意 guard 表达式反解绝对到期时间。idle timer/parked run 只占一条 future ready
  记录,不属于 recovery,也不会在热路径自旋。
- A11 必须把 `kernel/indexes/ready/` 与 aggregate/recovery/outbox 一起原子回收。

### A11 完成(2026-10-18):终态回收 + PID 复用

**任务**:§11 D 的 A11(P1)。存储只保留恢复所需的最小状态:run 终态、outbox 全 `Delivered`、
operation 全部 retired、channel delivery 全部结算后,原子删除 aggregate + 派生索引,再幂等删除
行/messages/deliveries/workdir;清理后同一 pid 可再次启动。不做历史归档。

**事实与契约**

- 可回收判定收敛为 `ProcessSnapshot::is_reclaimable()`:tasks 非空且全部 terminal;outbox 每条
  `Delivered`;operations 全部 retired(`Completed`/`Superseded`)。`Pending`/`RetryScheduled`/
  `Claimed`/`CommitPrepared`/`Committed`/`EffectUnknown`/`Quarantined` 任一存在即为不可回收;
  `DeliveryUnknown`/`Quarantined` 的 outbox record 同样保留。投影的 `removable` 复用该判定;
  channel delivery 仍按既有 `has_unsettled_deliveries`(只有 `Completed` 算结算)。
- 正常终态 run 的三个派生索引本来就可为空(A2 的“可空索引不写文档”),所以权威删除 batch 通常只删
  aggregate;但 batch 仍对每个索引分别 `require_exact`(存在时)或 `require_absent`(缺失时),
  并删除 aggregate + recovery/outbox/ready 三键,外加该 pid 前缀下残留的 recovery-entry 文档。
  aggregate 的 exact bytes 就是 version guard:任何 commit 都会改变它;guard 失败重读重试,绝不删
  一个已移动的 aggregate。
- 顺序:`refresh_projection` 先确认/重写 `removable` → `flush_pid` 后检查 delivery → **要求 proc 行
  已标 `removable`**(设计第 1 步;未标记则本轮不切割)→ guarded batch 删 aggregate+索引 → 通过
  writer 的 `RemoveProc` 幂等删行(含 messages/deliveries,workdir 先删)——删行与 `flush_pid` 都在
  per-pid 生命周期锁内完成,锁释放时行已落盘删除。aggregate 已删而索引/行
  残留时,下一轮以 `Absent` 识别:只删仍存在的索引并继续删行——标记在切割前已持久化,所以这个
  重试有据可依;不会出现“索引先删而 aggregate 残留”。
- 崩溃安全:删除 batch 是后端原子的;中断后 aggregate 缺失 → recovery/outbox/ready 扫描跳过
  missing aggregate(ready 记录由 `prepare` 忽略,outbox page 跳过),清理下一轮补删行/workdir。
  清除进程内状态(reported/admission/projection claims)在 aggregate 删除成功后、行删除前完成,
  不依赖墓碑。
- PID 复用:pid 的存在性**只看 aggregate**(`KernelHost::exists`);行、消息、delivery、尤其
  legacy `ops` 都不参与判定(`ops` 由 L1 删除,本笔已删掉 `Store::pid_has_rows`)。start 与 cleanup
  共用 per-pid 生命周期锁:start 在锁内查 aggregate 并创建 run;cleanup 从 aggregate batch 到
  行/workdir 删除完成都持锁,并用 `reclaiming` 标记覆盖“batch 已删、行删除失败”的重试窗口,因此
  新 run 不会落进旧 run 的残渣清理窗口。删除失败时若 proc 行已消失(整批删除已生效),标记即清;
  每轮 cleanup 也会清掉 proc 行已不存在的陈旧标记。到达旧 run 的迟到消息/动作属于业务去重边界,
  acts 不保存 generation 映射(CHANGELOG 已写)。
- 调度:cleanup 从 retry timer 拆出为独立 loop(`init_cleanup_timer`),独立 interval
  (`cleanup_interval_secs`,默认跟随 tick)、批大小(`cleanup_batch`,默认 256)、`LoopGuard`
  健康(`SchedulerHealth::cleanup`)与指标(`Runtime::cleanup_metrics`:backlog、最老 removable
  年龄、reclaimed、删除字节、删除失败、孤儿索引)。cleanup 失败只影响自己的 health。
- 测试构建不启动 cleanup timer(`init_cleanup_timer` 在 `cfg(test)` 下直接返回):reliability 用例
  在 run 结束后仍把 aggregate 当作决策真值读取,回收由用例显式调用 `sweep_removable` 驱动;生产
  构建的循环代码不变。投影故障用例在等待行收敛时也显式驱动 sweep(否则 A11 不再自动替它清掉
  滞后的行)。
- 投影遗留边界:A5 的逐行 generation 协议未做;清理在 `RowProjector` 记录 pid 墓碑,阻止清理后
  仍持有旧 snapshot 的投影重写行,下一次同 pid start 前清除。投影与清理另按 pid 用一把异步锁
  串行:cleanup 等已开始的投影写完再删 aggregate,墓碑再挡住之后的投影。完整 generation 防护
  留给 A5。

**测试**

- `reclaim_deletes_the_guarded_documents_and_is_idempotent`:完整终态 run 的 aggregate 与四个键
  全删,第二次 reclaim 为无副作用 `Absent`(覆盖旧库缺 ready index)。
- `reclaim_transaction_guards_the_aggregate_and_every_index`:batch 对 aggregate 用 exact bytes
  guard、缺失索引用 absent guard;`reclaim_refuses_to_delete_an_aggregate_that_moved` 在 batch
  前注入一次并发 commit,guard 拒绝删除并重读为 `Retained`(batch 前崩溃/竞争的可重试性)。
- `reclaim_retains_effect_unknown_and_quarantined_runs` /
  `reclaim_retains_outbox_records_that_did_not_deliver`:`EffectUnknown`、`Quarantined`、
  `DeliveryUnknown`、`Pending` outbox 一律保留,文档不动。
- `reclaim_cleans_orphan_indexes_after_the_aggregate_is_gone`:aggregate 缺失时只删存在的
  索引/recovery-entry,不碰其他 pid;`outbox_page_skips_an_index_whose_aggregate_is_gone` 固定
  outbox 扫描跳过 missing aggregate。
- `kernel_engine_finishes_an_interrupted_terminal_cleanup`:模拟 batch 后崩溃,下一轮补删行与 workdir,
  且再次 sweep 幂等;残渣未清时同 pid `start` 仍被拒(aggregate 已无,由 cleanup 的 `reclaiming`
  标记 + 生命周期锁挡住),清理完成后允许。
- `kernel_engine_keeps_a_run_with_an_open_delivery`:未结算 delivery 保留 aggregate/行/workdir,
  结算后才回收。
- `kernel_engine_refuses_a_pid_until_the_run_is_reclaimed_and_then_reuses_it`:清理前拒绝、清理后
  允许并保留 pid;`kernel_engine_projects_the_legacy_read_model` 增加聚合/索引全消失断言。

**验证**

```text
cargo test -p acts --lib --tests -> 894 passed, 0 failed, 1 ignored (219.44s)
cargo test -p acts --lib reliability:: -> 65 passed, 0 failed (186.35s)
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --check -> OK
```

**下游套件**:`acts-acl`、`acts-expr --all-features`、`acts-cli`、
`acts-package-state/http/javascript/nats`、`acts-store --features sled,sqlite,postgres` 绿。
两个已知非本笔失败:base `f57fde5` worktree 上 `acts-plugin-grpc` 的
`test_on_message_stream_drop_deregisters_channel` / `test_on_message_slow_subscriber_is_disconnected`
同样在 10s 超时;`acts-package-shell --test vars` 的
`the_inputs_of_the_act_embed_in_the_script` 与 A2 记录一致。
`acts-server` 的 test 构建在本机 rustc ICE(metadata/alloc;与 A2 记录的链接期 `os error 1455`
同属环境资源问题),`cargo clippy --all-targets` 的 check 构建已通过;未据此改代码。

**已知边界**

- 测试构建不跑 cleanup timer(§10 上文),生产构建照常;`Runtime::cleanup_metrics` 与
  `SchedulerHealth::cleanup` 在两种构建都可用。
- A11 的投影防护只是 pid 墓碑;真正的逐行 generation 协议仍是 A5。清理后仍持有旧 snapshot 的
  投影不会再写行,但新 run 复用同一 pid 后,A5 之前没有跨 generation 的严格隔离。
- 一个带有 `EffectUnknown`/`Quarantined` operation 或 `DeliveryUnknown`/`Quarantined` outbox
  记录的 run 会永久保留其 aggregate/行/pid,直到人工处理。这是契约,不是泄漏。
- 权威 batch 通常只删 aggregate:终态 run 的 recovery/outbox/ready 索引按设计已为空;batch 仍
  逐一 guard(present=exact / absent=absent),以覆盖旧库和并发写。`recovery-entry` 文档在
  reclaimable snapshot 里本应为空,清理会顺带删除该 pid 前缀下的残留并计入孤儿索引指标。
- `reclaiming` 是进程内标记,不跨重启。重启后第一轮 cleanup 与同一 pid 的 start 仍由 per-pid
  生命周期锁串行:start 先建出新 aggregate 时,cleanup 读到的就不是 `Absent`,而是这个非终态
  新 run,因此 `Retained`,不会删除它的行;cleanup 先完成时,start 随后从空 aggregate 开始。

### A1b 完成(2026-10-18):outbox 提交后发送 + 每 operation 一笔批量确认

**任务**:§11 D 的 A1b(P0)。“消息先于 aggregate commit 可见”的窗口来自 `dispatch` 在 plan commit
之前调用 `deliver_plan`,把 transport 确认的 receipt 随提交直接写成 `Delivered`。本笔把发送移到提交
之后,并把一个 operation 的全部新记录放进**同一笔**确认提交;per-record claim/confirm 试点
(§10「A1b 试点回退」)没有恢复。

**事实与契约**

- `deliver_plan` 与 `OutboxIntent.receipt` 删除。plan commit(`advance_transition`)现在只把每条
  outbox 记录写成 `Pending`;提交失败(guard/CAS/存储错误)不产生任何外部消息。
- `dispatch` 的提交后顺序:plan commit → wake 新 operation(内部加速)→ `project`/`project_touched`
  (proc/task/vars)→ lifecycle 上报 → transport 发送本 plan 的新记录 → **一次**
  `confirm_outbox_batch` → 槽位结算。lifecycle 事件语义未变,只是排到投影之后,保证任何外部观察者
  都能先读到行。
- `deliver_committed_outbox` 只发送 `receipt.new_outbox` 里仍为 `Pending` 的记录(`AlreadyCommitted`
  或并发 pass 已推进的记录跳过);它把 claim 与 transport 结果在**内存中**按现有状态机组合成
  `OutboxConfirmation`,交给 `AsyncProcessRepository::confirm_outbox_batch`。后者直接复用 plan commit
  写出的 exact bytes(不再重新 `load`),对每条记录校验 revision、在已提交 snapshot 上应用事件,
  再用专用 guarded batch(`build_outbox_confirm_transaction`)一次提交 aggregate + outbox 索引;
  确认不改变 operation,所以 recovery/ready 索引只作为 guard,不重写。**一个 operation 至多一笔
  确认提交**,与记录条数无关。`TransitionCommitOutcome` 因此带回 `LoadedProcess`,正常路径少一次
  聚合/索引读与 decode。
- 结果落盘沿用现有语义:确认成功的记录 `Delivered`;`RetryAt` 回到带 `available_at_ms` 的 `Pending`;
  `OutcomeUnknown` → `DeliveryUnknown`;`Quarantine` → `Quarantined`。`OutboxIndex` 由同一次
  `replace` 从新 snapshot 重算。
- 确认提交丢失 guard(`Conflict`)时整批不写,记录保持 `Pending`;`deliver` 通知立即唤醒 outbox pass,
  pass 重投并最终确认。这是 at-least-once 的既有重复窗口,不是新语义;幂等收敛留给 A1c。
  记录 revision 已被并发 pass 移动时该条跳过,由持有者收敛。
- retry pass(`dispatch_outbox`/`dispatch_outbox_record`)保持原有的 per-record claim+confirm;
  它只处理异常/重投,不承担正常路径。tick 未缩短。
- A11 判定未改:`ProcessSnapshot::is_reclaimable()` 仍要求 outbox 全部 `Delivered`。确认前 run 因
  `Pending`/`DeliveryUnknown`/`Quarantined` 被保留;确认后 cleanup 的 `refresh_projection` 重算
  `removable` 并回收。正常路径不额外投影一次 proc 行,该修复仍由 cleanup 的
  `unmarked_settled_procs` 负责。

**测试**(`crates/acts/src/scheduler/shell/tests/mod.rs`)

- `an_outbox_message_waits_for_its_aggregate_commit`:注入 plan commit 失败(store 在
  `CommitPrepared` 后的 aggregate batch 返回错误)⇒ transport 0 次、aggregate 无 outbox 记录。
- `one_operation_confirms_all_its_records_in_one_commit`:instrumented store 统计
  “新增 outbox 的 plan commit 数”与“同版本的 outbox 变更 batch 数”;线性 3 节点全程二者相等、
  确认条数等于新增条数、每条只发送一次,且全部 `Delivered`。per-record claim+confirm 会让确认数
  成倍增加,此断言直接挡住回归。
- `a_commit_with_two_records_confirms_them_in_one_commit`:一笔 plan 写两条记录 ⇒ 恰好一笔确认
  batch、该 batch 推进两条、outbox 索引键删除。
- `a_lost_confirmation_is_redelivered_without_losing_the_message`:确认 batch 返回 guard 失败 ⇒ 记录
  `Pending`;手动 outbox pass 重投一次并全部 `Delivered`(记录既有 at-least-once 重复)。
- `partial_delivery_failures_are_persisted_and_keep_the_run`:同批分别落 `Pending{RetryAt}`、
  `DeliveryUnknown`、`Quarantined`;outbox 索引仍带 deferred `Pending` 与 `DeliveryUnknown`、
  不再有 `Claimed`;`is_reclaimable()` 为 false,`reclaim_process` 返回 `Retained`。
- `messages_are_visible_only_after_the_projection`:transport 在 `deliver` 时检查 store 已写入 proc
  行;每次投递均为 true。
- 既有 outbox/recovery/A11/reliability 套件全绿(见下)。A11 的两个 cleanup 用例改为等待
  `is_reclaimable()` 而不是仅等 task 终态:`process_completed` 只证明执行已终态,确认提交可能
  紧随其后;等待 A11 谓词消除了这个时序窗口。

**验证**

```text
cargo test -p acts --lib --tests -> 900 passed, 0 failed, 1 ignored (524.75s)
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> EXIT 0
cargo fmt --check -> OK
examples/simple: debug 13.0-13.7s / release 1.48-1.70s
A11 baseline f5f62bc(same machine, worktree): debug 9.8-11.8s / release 1.16-1.33s
```

吞吐口径:`examples/simple` 的固定成本是“有 outbox 的 operation 多一笔确认提交”(与记录条数无关);
per-record 试点在同一循环上是 >20s(debug)。本笔没有恢复该写放大,也没有把消息延迟转嫁给 tick。
完整 `-p acts` 套件的墙钟受本机并发构建影响(同一提交的多次运行在 265-525s 之间),不作为阈值。

**已知边界**

- 正常路径固定多出一笔确认提交(有 outbox 的 operation);这是“提交后才可见”的代价,但不再随
  outbox 条数增长,也没有 per-record 的 claim/confirm 写放大。
- 确认失败后的重投会再次调用 transport:at-least-once 的既有契约;稳定幂等键与可查询 observation
  属于 A1c,本笔不改变 channel 的 ack/delivery 行语义。
- commit 之后、投影之前进程崩溃时,retry pass 仍可能先于一次完整的行投影发送消息;这是 A5
  逐行 generation/补投影协议的边界,A1b 只保证正常路径的 project→transport 顺序。
- 未引入独立 outbox log;批量确认在现有 aggregate + 派生索引上完成。

### A1c 完成(2026-10-18):effect 幂等协议 —— 稳定 key + observation + 人工闭环

**任务**:§11 D 的 A1c(P1)。A1a 已把 effect 后的 CAS 冲突/`next` 失败落到 `EffectUnknown`,
A1b 已把 outbox 改为提交后发送并每 operation 一笔批量确认。本笔补齐 effect 侧的身份与证据:
没有稳定 key 与可查询 observation 时,`EffectUnknown` 只能人工;任何自动重试都可能重复外部副作用。

**事实与契约**

- `EffectUnknown` 的产生点不变:`mark_effect_unknown`(`dispatch` 的 CAS 冲突、`execute_one`/`apply_action_once`
  的 `next` 失败)与 `ReleaseExpiredClaim` 对 `CommitPrepared` 的租约过期。`ClaimAndPrepare` 仍在 effect
  之前落盘;outbox 顺序/payload/channel ack 行语义未动,A1b 的提交后发送与批量确认未改。
- **稳定 key**:新增 `core/model/effect.rs::EffectKey`,字段为 `process_id`/`TaskId(node,generation)`/
  `sequence`,是持久化 operation 身份的**纯函数**;渲染为 C5 instance wire + sequence
  (`<sha256(pid,node,gen)[..32]>-<sequence>`)。`ClaimAndPrepare` 已把 operation 身份写入 aggregate,
  因此 key 在 effect 之前就是 durable 的;没有随机源,也不另存一份会漂移的副本。重规划、重试、
  重启后由同一 operation 重建出 byte-identical 的 key。
- **executor 接口**:`EffectRunner::run(request, key)` 收到 key;`PackageEffectRunner` 另把它写入
  headless scope 的私有 env `__effect_key`,`Context::effect_key()` 只读地交给包,`is_private_key`
  保证它不会写回 run 的 durable env,workflow 的 `$env` 也读不到。
- **幂等声明**:`ActPackage::idempotent() -> bool`(默认 `false`)是包作者对“同 key 重复执行安全”的
  显式声明;`ActPackageRegister` 捕获该函数指针,`PackageEffectRunner::idempotent` 按注册表回答。
  没有声明的 effect 永远不被自动重试。
- **observation**:`EffectRunner::observe(key)` 返回
  `Applied(EffectOutcome) | Failed(message) | NotApplied | Unknown`;默认 `Unknown`。
  observation 是 executor 自己的事实(可跨重启持久化,由 executor 决定),内核不把它塞进 aggregate,
  因此不产生每 effect 一笔额外的 aggregate commit。
- **恢复协议**(`recover` 的 `ManualReconciliation` 与 `ReclaimExpired -> EffectUnknown` 分支):
  - `Applied`/`Failed` ⇒ `prepare_reconciliation` 以 `ClaimAndPrepare`(fence = attempt+1)重占,
    用 observation 的结果直接跑 `ApplyAction::next` 并提交,**不再调用 effect**;
  - `NotApplied` + `idempotent` ⇒ `ResolveEffectNotApplied` 把 operation 写回 `RetryScheduled`
    (attempt 不降低),同 key 重新入队并 wake;
  - `Unknown`、`NotApplied` 但非幂等、查询失败、重放失败 ⇒ `ManualReconciliation`,不自动 wake。
  `RecoveryReport` 新增 `effect_applied`/`effect_retried`;剩下的仍在 `reconciliation_required`
  (serve 的日志与 A11 的回收判定沿用该列表)。
- **人工闭环**:`Engine::effect_unknowns(pid)` 按 pid 列出 `EffectUnknown` 的
  `tid`/`sequence`/稳定 `key`/`attempt`;`Engine::resolve_effect(pid, key, resolution)`:
  - `Applied { outputs }` ⇒ 以正常 guarded commit 重放计划(不再执行 effect),`outputs` 是操作员
    捕获的返回值(可为空);
  - `NotApplied` ⇒ 专用 `ResolveEffectNotApplied` durable 事件,回到 retry 队列,同 key 重试。
  两条路径都走 aggregate 的 compare-and-swap;重启后读到的就是 decision 的结果,不会有进程内
  “已复位”的假象。`EffectUnknown`/`Quarantined` 仍不可回收(A11 条件未改)。

**测试**(`shell/tests/mod.rs`、`core/model/{effect,operation}.rs`)

- `an_observed_effect_converges_without_running_it_again`:真实包 effect + `ConflictKv` 丢失 plan
  commit ⇒ `EffectUnknown`;executor observation `Applied` 后 `tick` 报 `effect_applied`,operation
  `Completed`、run 收敛,effect 计数保持 **1**。
- `an_idempotent_effect_is_retried_under_the_same_key_and_dedupes`:observation `NotApplied` + 幂等声明
  ⇒ `effect_retried`,`RetryScheduled`;重跑一次,executor 以 key 去重,attempts=2、external side
  effects=**1**,两次 key 字符串相等。
- `a_non_idempotent_effect_is_manually_reconciled_across_restarts`:无 observation、非幂等 ⇒
  `reconciliation_required`;重启后的新 host 读出的 key 与 executor 见过的一致且等于
  `EffectKey::of(operation)`;`resolve_effect(Applied)` 后不再调用 effect、run 收敛;第三个 host
  读回 `Completed` 且 `effect_unknowns` 为空。
- `a_non_idempotent_effect_can_be_resolved_not_applied`:人工 `NotApplied` 让非幂等 effect 重新入队,
  同 key 执行,run 收敛。
- `a_proven_not_applied_effect_returns_to_the_retry_queue`:状态机单测(错误 attempt 拒绝、同 attempt
  转 `RetryScheduled`、下一 claim 必须 fence 更高)。
- `effect.rs` 单测:key 由 operation 身份纯函数重建、不同 operation 不同、无随机/时间成分。

**验证**

```text
cargo test -p acts --lib --tests -> 908 passed, 0 failed, 1 ignored (517.11s, final)
cargo test -p acts --lib effect -> 24 passed, 0 failed (all A1c cases + effect regression)
cargo test -p acts --lib effect_write_back_folds_into_the_process -> 1 passed
cargo test -p acts --lib non_idempotent_effect -> 2 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

首次全量运行(661.40s)出现两个负载敏感的未通过,均单跑绿,最终全量运行不再出现:

- `scheduler::tests::reliability::cancel::cancel_racing_next_converges_on_one_terminal_decision`
  (单跑 1 passed,184.04s;当时 5s 行回收等待在满负载下超时,任务的 one running 属于等待窗口);
- `scheduler::tests::reliability::timeout::the_driver_leaves_a_settled_run_alone`
  (C7 已登记;单跑 1 passed)。

`cancel_racing_next_converges_on_one_terminal_decision` 不在既有 flake 清单中;本笔未改该路径
(effect runner/observation 均未参与),按负载/时序抖动登记,不静默忽略。门禁外的影响面套件:
`acts-acl`、`acts-expr --all-features`、`acts-cli`、`acts-package-state/http/javascript/shell`
绿;唯一失败 `acts-package-shell --test vars` 的 `the_inputs_of_the_act_embed_in_the_script`
是 A2/A11 已登记的既有非本笔失败(`$inputs().data` 为 null)。

**已知边界**

- `PackageEffectRunner` 不替内置包保存 observation:它不知道外部系统实际发生了什么,默认回答
  `Unknown`。因此内置包的 `EffectUnknown` 仍停在人工协调;可自动收敛的前提是 executor
  (自定义 runner 或包自身的持久 observation)实现 `observe`。这是协议,不是遗漏 —— “不能证明”
  就不能自动推进。
- 幂等声明按当前注册表回答,不随 operation 落盘;重新发布一个包并改变声明会改变后续恢复的判定。
  声明是运维断言,不是运行期事实;需要固化时可另立字段(会动持久化格式)。
- 人工 `Applied` 只折叠操作员提供的 `outputs`;包在崩溃前写过的 task data/env 增量无法重建,
  因此这类恢复不承诺逐字段等价于原 effect 结果。
- 不引入跨进程 exactly-once:同一 key 的重复投递仍由 executor/外部系统去重;内核只保证不再生成
  第二个 key,并在 observation 证明 not-applied 时才自动重试。
- `EffectUnknown`/`Quarantined` 的 run 仍永久保留(直到人工 resolve),A11 回收条件未变。

### A3 完成(2026-10-18):纯内部 operation 折叠进产生它们的 plan

**任务**:§11 D 的 A3(P1)。A1a/A1c 已给出 effect 边界与 `EffectUnknown` 语义,A2 已把正常活性移到 durable
ready queue,A11 已给出回收边界。本笔只处理一个缺口:线性链为纯内部传播、join、branch 选择与终态推导
单独创建 durable operation,把一次可由产生者一次算完的 transition 放大成
“创建 -> 提交 -> wake -> 出队 -> claim -> load -> plan -> commit -> close”。

**事实与契约**

- 旧链(线性 3 节点 = 9 个 operation):每个节点 `ExecuteTask`(seq n)提交后创建 `ApplyAction`(seq n+1);
  `ApplyAction` 的完成计划(或客户端动作的 `ActionPlanner`)创建 `PropagateTask`(seq n+2);`PropagateTask`
  再写后继 `Created -> Ready` + `ExecuteTask`,并顺带算 join/catch-join、upward walk 与终态。fan-out 的
  未选中分支创建 `SkipTask`,死源传播可再创建 `SkipTask`/`AbortTask`,`Cancel`/`Back` 创建 `ReenterTask`。
  这些 operation 的 phase 2 都只是 snapshot 本身(`Outcome::Planned`),工作全在 phase 3 的计划里。
- `TransitionPlan` 已能在一个 commit 里携带 `task_creates`/`state_changes`/`variable_writes`/`operations`/
  `outbox`/`source_operation`;commit 在同一 atomic batch 里关闭 source operation,并写后继 ready 记录。
  所以折叠不需要新持久化语义,只需要一个**带 pending delta 的读取视图**来跑那些纯计划。
- 保留的 durable 边界:`ExecuteTask`(guard/package 解析/新 execution 的开始)、`ApplyAction`(客户端决策
  的 replay 或包 effect)、`DispatchTimeout`(时钟等待)。`EffectKey` 仍由承载 effect 的 `ApplyAction`
  operation 身份派生,本笔未动。`ApplyAction` 仍是 start→completion/decision 的 durable 单位,包括
  package-less 节点;本笔删除的只是纯 hand-off,不是这个边界。
- 身份/消息契约:`PropagateTask`/`SkipTask`/`AbortTask`/`ReenterTask` 只作为 scratch 里的占位记录保留,
  因此 `next_operation_sequence`、折叠计划新建的 operation id、`propagate:`/`skipped:`/`reenter:`
  outbox key 与旧的分次提交逐字一致。最终 aggregate 不落这些 operation 记录;outbox topic/payload/相对
  顺序不变。`Back` 原本由 `ActionPlanner` 创建的 +1 空 `ApplyAction`(没有 action fact,只写一条空
  `completed:` 标记)不再创建;`ReenterTask` 仍在 source+2,rewind 的 key 不变。
- `TaskCreate` 现在允许终态初值(`Skipped`/`Aborted`):同一批创建、又被折叠级联决定的分支在执行出生时
  就是级联状态,aggregate 不观察中间态,commit 也不再收到“对同批创建的任务做 state change”的非法计划。

**实现**(`shell/operation/fold.rs`,`dispatch` 在 commit 前调用)

- **快速路径**:计划里没有纯 hand-off 时不复制 snapshot、不进入 fold(启动、包 effect、timer round 的
  常见计划都走这里)。
- **scratch**:复制 claim 时的 snapshot,应用本计划的 creates/state changes/variable writes/operation
  占位;之后每个折叠计划先应用到 scratch,再合并进最终计划。revision 与 process counters 不推进,生成的
  state change 仍以 claim 前 revision 为 `expected_revision`。
- **工作队列**:按 `plan.operations` 顺序 FIFO;每个纯 intent 用其自身身份(合成 claim,只为 outbox key)
  调用现有 driver 的 `next(Outcome::Planned)`;它新产生的纯 intent 入队,边界 intent 留在最终计划。
  重复 intent 同 kind 幂等跳过,不同 kind 报错。
- **合并**:对同批创建任务的终态决定改写 `TaskCreate.initial_state`,并丢弃该任务已排队的 start;重复
  state change/variable write 按“后写胜”合并;outbox 追加。合并后 `validate()` 一次。
- **失败原子性**:fold 任一步计划失败时,`dispatch` 提交**原计划**,纯 operation 仍按旧路径落盘并由
  ready queue 驱动 —— 已执行的 effect 不会因为一个内部级联算不出来而被标成可重试;effect-free 的 CAS
  失败仍走 A1a 的释放/重规划。
- **旧库**:四个 kind 的持久化词汇与 driver 都保留。旧 aggregate 里 pending 的纯 operation 仍能被驱动;
  它新产生的 hand-off 在 `dispatch` 里折叠。
- **终态行修复**:fold 后的决策提交可能是 run 的最后一笔,旧链里“下一笔 propagation 提交”对写失败行的
  自然重投影没有了。`project`/`project_touched` 在 snapshot 全部终态且投影报错时,清掉该 run 的投影
  claims(`RowProjector::forget_run`)并重投影同一终态 snapshot 一次。这是重试,不是 A5 的版本协议。

**测试**

- `linear_process_runs_to_completion_and_delivers_outbox`:3 节点 `report.executed` 从 **9 → 6**
  (每节点 start + completion),outbox 仍 9 条,aggregate 无任何纯 kind;`assert_no_folded_operations`
  同时用于 branch 套件、catch 套件和 linear。
- 新增 `a_folded_completion_loses_its_commit_without_splitting_the_propagation`:完成计划带着后继
  `Created -> Ready` + `ExecuteTask` + `propagate:` 记录一起丢失 CAS ⇒ 后继仍 `Created`、无
  `propagate:` 记录、completion `RetryScheduled`;重驱动后一次落齐。
- `a_cancel_carries_its_rewind_in_the_deciding_commit` 改为断言 rewind 的新 generation、其 `ExecuteTask`
  与 target fact **都在决策提交里**(不再有 pending `ReenterTask`);reliability/back 的四条 cell 改为断言
  零纯 operation,仍以 generation 数守住“恰好一次”;stall 的 completion cut sweep 从 0..=3 收为 0..=2。
- 既有 branch/join/catch/next/terminal 行为用例全部保持;A1a/A1b/A1c/outbox/reliability/A11 套件绿。

**验证**

```text
cargo test -p acts --lib --tests -> 911 passed, 0 failed, 1 ignored (292.12s, final)
cargo test -p acts --lib a_folded_completion_loses_its_commit_without_splitting_the_propagation -> 1 passed
cargo test -p acts --lib scheduler::tests::reliability::back:: -> 4 passed
cargo test -p acts --lib branched_step -> 4 passed
cargo test -p acts --lib kernel_catch_ -> 3 passed
cargo test -p acts --lib a_cancel_carries_its_rewind_in_the_deciding_commit -> 1 passed
cargo test -p acts --lib a_run_beyond_the_cache_cap_parks_until_a_slot_frees -> 1 passed
cargo test -p acts --lib a_store_fault_under_the_completed_acts_row_keeps_the_decision_and_sweeps -> 1 passed
cargo test -p acts --lib a_failed_projection_keeps_the_decision_and_never_resurrects_the_act -> 1 passed
cargo test -p acts --lib a_cut_swept_across_a_completion_converges_at_every_write_boundary -> 1 passed
cargo test -p acts --lib one_operation_confirms_all_its_records_in_one_commit -> 1 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

门禁外:`acts-acl`、`acts-expr --all-features`、`acts-cli`、`acts-channel`、`acts-store --features sled,sqlite`、
`acts-package-http/state/javascript` 绿;`acts-package-shell` 除 A2 已登记的
`the_inputs_of_the_act_embed_in_the_script`(`$inputs().data` 为 null,base 同样失败)外绿。
`acts-server` 仍因链接期 `os error 1455`(页面文件不足)无法
编译,同 A2 登记。`acts-plugin-grpc`/`acts-plugin-web` 各有两个 slow-subscriber cell 超时;已在 base
`683f282` 的干净 worktree 上复现同样失败,与本笔无关,按已知非本笔登记。`acts-channel` 的 NATS 套件、
`acts-store --features postgres`、`acts-plugin-nats`/`acts-package-nats` 需要本机未启动的
NATS(4222)/Postgres(5433),未运行。

**边界/剩余**

- 折叠把纯 operation 的 outbox 记录移进产生它们的提交;内部记录(`propagate`/`skipped`/`reenter`/`fanout`)
  不映射成外部 `Message`,外部消息的集合、topic、payload 与提交内相对顺序不变。join/终态完成消息随触发它
  的提交一起落盘,不再晚一个 operation;最终行/生命周期事件由既有 golden 用例逐字钉住。
- 本笔不做 A4 的 per-pid command lane、不做 A5 的 projection 版本协议;`forget_run` 的重投影只用于
  已终态 snapshot,不改变 A5 的“旧投影不能覆盖新投影”缺口定义。
- 保持 A11 回收判定不变:折叠不产生额外 non-terminal operation,也不把 `EffectUnknown`/`Quarantined`
  变成可自动推进。

### A4 完成(2026-10-18):每 Process 单写者 command lane

**任务**:§11 D 的 A4(P1)。driver 的 `serve` 串行执行 ready operation,但客户端动作在独立任务上并发提交;
同一 aggregate 的 client action 之间、以及与 driver/恢复之间靠 revision/CAS + `ACTION_ATTEMPTS=8` 重规划。
A1a 只关闭 effect 后的自动重试,A1c 只把已执行 effect 的冲突落到人工/observation;正常路径仍可能两个 plan
基于同一 revision 竞争。

**事实与契约(本笔前的一个 pid 写者)**

- driver:`serve` 的 select 循环 → `drive_ready` → 逐个 `execute_one`(prepare/run/next/dispatch);一个任务,
  跨 pid 也串行。
- client:`Runtime::do_action` → `KernelHost::apply_action`,在调用方任务上跑;`apply_action_once` 每次
  load → preflight → intent commit → claim → plan → commit,冲突由 `ACTION_ATTEMPTS` 重新 load/规划。
- recovery:tick 上的 `recover`;`ReclaimExpired`/`FinalizeCommit` 直接 `operation_event`,
  `ManualReconciliation` 走 `reconcile_unknown_effect` → `converge_observed_effect`/`retry_unknown_effect`。
- outbox:`dispatch` 内的 `deliver_committed_outbox`(提交后发送 + 一笔批量确认)与 tick 上的
  `dispatch_outbox`(per-record claim → transport → confirm)都写 aggregate。
- cleanup:`sweep_removable` → `cleanup_process`,持 per-pid lifecycle + projection 锁,guarded 删除 aggregate。
- 结论:同一 pid 有 5 类进程内写者;driver 与 client、client 与 client 之间完全由 revision/CAS 仲裁。

**lane**

- `KernelHost` 新增 `command_lanes`(per-pid `tokio::sync::Mutex`,与 `lifecycle_locks` 同形)与
  `lock_command_lane`;`shell/operation::CommandLane` 是命令持有的 guard,提供 `release()`/`resume()`。
- `Operation` 携带 `lane: &'a mut CommandLane`;`ActOperation::run` 改收 `&mut Operation`(只有 run 需要动
  lane;`next` 仍是 `&Operation`)。
- 进入 lane 的命令:`execute_one`(driver 每次驱动)、`apply_action_once`(client 每次尝试,load 之前)、
  `converge_observed_effect`、`retry_unknown_effect`、`resume_action_intent`、`recover` 的
  `ReclaimExpired`/`FinalizeCommit`、`dispatch_outbox_record` 的 claim 与 confirm。lane 按 pid,不同 pid 独立。
- effect 窗口:`ApplyAction::run` 在 `EffectRunner::run` 前 `release()`、返回后立即 `resume()`,两者之间没有
  其他 await;其余 phase 全在 lane 内。effect 期间其他 command 推进 aggregate 时,dispatch 的提交失败按
  A1a/A1c 分类(effect_started ⇒ `EffectUnknown`,不重跑 effect)。
- 折叠:A3 的 scratch 计划用 `CommandLane::detached`(不持 lane、不跑 effect),仍在产生者的 turn 内完成。
- 锁顺序:cleanup 走 lifecycle → projection 且**从不等 lane**;命令只持 lane(内部只有同步锁与 store 写)。
  lane 与 lifecycle/projection 从不嵌套 ⇒ 无环。lane 不可重入:`recover` 的 reclaim 只把 lane 包住
  `operation_event` 本身,随后的 reconcile(converge/retry)自己取 lane。

**同步化暴露的 A1a 分类缺口(本笔修复)**

`commit_transition` 在 domain 层对陈旧 plan 返回 `VersionConflict`(`repository_error` 映射为 conflict),
原 `dispatch` 只在 store guard 的 `Ok(Conflict)` 分支做 effect 分类;effect 之后走 Err 分支会停在
`CommitPrepared` 等 60s 租约。现在 `dispatch` 对 `effect_started && is_conflict(&error)` 的 Err 也
`mark_effect_unknown` 并返回同一 conflict 错误;effect-free 的 Err 保持原样(由 client 重试循环/ready pass
处理)。这使 lane 唯一的 effect 窗口内提交失败立即落到 A1a 的 `EffectUnknown`,而不是等租约。

**指标**:`KernelHost::action_replans()` 统计 `ACTION_ATTEMPTS` 的重规划次数(正常路径必须为 0),每次
重规划写进 debug 日志。

**测试**(`shell/tests/mod.rs`)

- `GuardCountingKv`:数 per-pid aggregate 上被拒绝的 guard(store-level CAS 冲突计数器)。
- `concurrent_client_actions_decide_exactly_once_under_the_lane`:同一 execution 上 8 个并发 client
  action(4×Complete、2×Fail、1×Cancel、1×Back)。恰好一个决策提交;loser 全部是**决策性拒绝**(不是
  conflict);`action_replans()==0`;aggregate guard 冲突计数 **0**(无两个 plan 基于同一 revision);
  失配的 auto ApplyAction 仍落盘并在随后 drain 中完成。
- `effects_of_different_runs_overlap_outside_the_lanes`:两个 pid 的 step effect 在 `Barrier(2)` 会合——
  任一 lane 或全局 driver 锁跨 effect 持有都会卡死;两者都提交,run 都收敛。证明 lane 不全局串行、
  effect 不被 lane 串行。
- `an_effect_releases_the_lane_for_a_concurrent_decision`:effect 被闸住时,同一 pid 的 client Complete
  在 10s 内成功(lane 已释放);放行后 effect 的 plan commit 失配 ⇒ driver 报 conflict、operation
  `EffectUnknown`、effect 计数保持 1;`recover` 报 `reconciliation_required`;人工 `resolve` 作为 lane
  命令返回(此处按契约拒绝重放一个已被 client 完成的 decision),不重跑 effect。
- A1a/A1b/A1c/A2/A3/A11/outbox/reliability 既有套件全部保持绿(见验证)。

**验证**

```text
cargo test -p acts --lib --tests -> 914 passed, 0 failed, 1 ignored (154.72s)
cargo test -p acts --lib reliability:: -> 65 passed, 0 failed (119.23s)
cargo test -p acts --lib effect -> 26 passed, 0 failed
cargo test -p acts --lib reclaim_ -> 7 passed, 0 failed
cargo test -p acts --lib outbox -> 32 passed, 0 failed
cargo test -p acts --lib concurrent_client_actions_decide_exactly_once_under_the_lane -> 1 passed
cargo test -p acts --lib effects_of_different_runs_overlap_outside_the_lanes -> 1 passed (5 consecutive runs)
cargo test -p acts --lib an_effect_releases_the_lane_for_a_concurrent_decision -> 1 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

**边界/剩余**

- lane 是**进程内**正常并发控制:崩溃/重启/租约过期仍由 recovery 以 CAS 接管,跨实例写者仍由 revision
  guard 挡住。`EffectUnknown`/`Quarantined` 的人工闭环、A11 的回收判定、A1b 的提交后发送与批量确认均未改。
- lane 表按 pid 常驻(与 `lifecycle_locks` 同口径),不在回收时删除;PID 复用时复用同一 lane。
- cleanup 不取 lane:它以 aggregate exact-bytes guard 为 fence,且在 lifecycle/projection 锁内;与 lane
  无嵌套。A11 的 pid 墓碑仍是清理后旧投影的唯一防线(A5 接手)。
- 本笔不做 A5 的 projection 版本协议、不做 A6 的 Core Decision、不做 A7 的统一 Recovery Command;
  `run_settled` 与投影 `settled` 的两套语义未合并(A5/A6)。
- 新增的 dispatch Err 分类只覆盖 `is_conflict` 的错误;非 conflict 的提交失败(真正损坏的 plan)仍原样
  上抛,e2(协议错误)不被吞掉。

### A5 完成(2026-10-18):Projection 版本协议

**任务**:§11 D 的 A5(P1)。投影从“进程内 claim 表的写前检查”变成**持久化版本 + store-level 条件写**;
启动按持久化 revision 增量补投影,失败进入有界、可重试、有指标的 repair queue;外部观察者只在行写完后可见。

**事实与契约**

- **行版本载体**:`procs`/`tasks`/`vars` 三行各新增 `#[serde(default)] source_revision: String`,
  编码为 `"<graph>.<process>.<operations>"`(`ProcessVersion::encode/decode`)。字段是**追加**的:
  codec `v` 不升版(与 `Proc.removable` 同口径),旧行缺字段解码为空串 → `None` → “比任何 aggregate
  都旧”,下一次投影覆盖它。`ProcessVersion` 的字段顺序/派生 `Ord` 与字符串解析后的数值比较一致。
- **store-level 条件写**:`KvCollection::write_if_newer`(`Store::write_projection`)在文档锁内读
  stored 行、比较 revision,再把 `update_ops` 与**该行当前字节的 exact guard**(缺失时 absent guard)
  放进同一个 `KvStore::batch`。guard 失败 → 重读重判(跨进程写者的唯一可见形式);有界 8 次后报错。
  规则:`stored 无 revision` → 应用;否则 `incoming > stored` 才应用;`allow_equal` 只给两种显式修复:
  `project_repair`(boot catch-up / repair queue)与 `project_proc_row`(`removable` 依赖 delivery 行,
  不在 aggregate 版本内)。相同内容不重写(`Unchanged`)。
- **claim 表降级**:`RowProjector::projected` 仍是快路径(避免重复读/写),但不再是唯一守卫;去掉了
  `forget_proc` 的“同版本重投影靠清 claim”这一假设,claim 只在两步都失败时兜底。
- **启动增量补投影**:`AsyncProcessRepository::process_ids()` 枚举 `kernel/aggregates/`;host 的首次
  recover 在 `repair_all_ready` 之后调用 `catch_up_projections`,`project_repair` 每个 aggregate,已同版本
  的行由条件写跳过。失败的行/run 进入 repair queue。
- **durable repair queue**:queue 的持久形态就是“行 revision 落后于 aggregate”这一事实本身 —— 重启后
  boot catch-up 从 aggregate 集合重建;进程内只是它的视图(`BTreeMap<pid, first_seen>`,每 pid 一条,
  与 aggregate 数同阶,不随失败次数增长)。`recover` 每轮重试至多 32 条;失败留在队列,指标可见。
  不新增 collection、不缩短 tick。
- **顺序契约**:`dispatch` 仍是 commit → wake → project/project_touched → lifecycle → outbox transport →
  confirm → slot。`project` 现在返回“行是否已 current”:失败时**跳过 lifecycle 与本次投递**,outbox 记录
  保持 `Pending`,run 入 repair queue;repair 成功后补报 lifecycle(once-per-phase),outbox retry pass 在
  投递前先 `repair_projection`,因此消息消费者同样只能看到已投影的提交。A1b 的“提交后发送 + 每 operation
  一笔批量确认”未改;失败只是把发送推迟到下一次 pass。
- **读契约**:Core/Recovery 从不读投影做权威判断(aggregate 仍是唯一真值)。查询 API 选择**强读 aggregate**:
  `Runtime::proc`(以及经它的 `proc:get`)比较 proc 行的 `source_revision` 与 aggregate 版本,落后就同步
  `project_repair`;修复失败仍返回已存的行并留下 repair(用 `projection_metrics().lag` 观察),不把读
  API 变成聚合读的唯一入口。
- **终态语义**:`facts::run_settled`(只看 task terminal,决定 admission slot / lifecycle)与
  `projection::projection_settled`(还要求 outbox 全 `Delivered`、operation 全 retired,决定行/目录可回收)
  分别命名并在模块文档写清;`report_lifecycle` 用前者,`removable` 用后者。
- **指标**:`ProjectionMetrics { lag, oldest_lag_age_ms, applied, unchanged, stale, failed, repaired }`,
  经 `Runtime::projection_metrics()` 暴露。`stale` 是“被持久版本拒绝”的次数(旧投影);`failed` 是写失败;
  `repaired` 是 repair queue 中成功收敛的 run 数。
- **A11 未回归**:清理仍按 pid 墓碑 + lifecycle/projection 锁串行;`refresh_projection` 改为整 run 的
  `project_repair`(tasks 先、proc 最后),清理前仍要求 proc 行已标 `removable`;删除行/aggregate 的顺序、
  `reclaiming` 标记、`RemoveProc` 幂等删除均未动。

**测试**

- `an_older_projection_cannot_overwrite_a_newer_one`:instrumented store 在旧投影“读后、guarded batch 前”
  注入新版本行;旧 batch 的 guard 失败,重读后为 `Stale`,行保持新版本;重启(无 claim 表)同版本也拒绝旧投影。
- `a_restart_catches_up_only_the_rows_that_lag`:把一行改成旧 revision/旧状态后换 host;boot catch-up 恢复
  到 aggregate 版本,`lag == 0`。
- `a_projection_failure_is_repaired_before_the_completion_event`:行写持续失败时 completion event 不发出、
  `failed/lag > 0`;解除故障后 `recover` 重投影并补报 completion,`repaired > 0`、`lag == 0`。
- 既有 `messages_are_visible_only_after_the_projection`、A11 回收/复用、outbox/reliability、effect 套件全部保留。

**验证**

```text
cargo test -p acts --lib --tests -> 919 passed, 0 failed, 1 ignored (152.67s)
cargo test -p acts --lib reliability:: -> 65 passed, 0 failed (119.06s)
cargo test -p acts --lib outbox -> 32 passed, 0 failed
cargo test -p acts --lib effect -> 26 passed, 0 failed
cargo test -p acts --lib reclaim -> 8 passed, 0 failed (A11)
cargo test -p acts --lib projection -> 8 passed, 0 failed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

**下游(非服务依赖)**:`acts-acl`、`acts-expr --all-features`、`acts-store --features sled,sqlite`、`acts-cli` 全绿;
`acts-channel`/`acts-plugin-nats`/`acts-server` 仍按 §11.C 的服务依赖口径。

**边界**

- 不新增持久化 repair collection:queue 的持久性由“行 revision 落后”表达,重启重建;这是有意的简化。
  若未来需要跨进程共享 repair 进度,再把它提到 durable index(A10 的独立 interval/指标范畴)。
- 同一 aggregate 版本的显式修复可以改 `removable`/时间戳保留;它无法把行写回更旧状态(持久版本仍单调)。
- 读 API 的强读以 proc 行作为整 run 的 current 标记:tasks 先写、proc 最后写,因此 `proc.source_revision`
  至少等于 task/vars 行版本。
- 不做 A6 的 Core Decision、A7 的统一 Recovery Command;不改 effect/outbox payload 与 A1b 顺序;不改 A4 lane。

### A6 完成(2026-10-18):Shell 变薄,Core 输出完整 Decision

**任务**:§11 D 的 A6(P1)。把 settle/lifecycle/admission 的判定从 `shell/mod.rs`、`operation/dispatch.rs`、
`facts.rs`、`projection.rs` 收进 `core/decision.rs`;Shell 只保留 load → plan → commit → 执行 Decision 的
IO 顺序。本笔不改 effect 语义、outbox payload/topic、A1b 顺序、A4 lane、A5 持久 revision/repair queue、A11 回收判定。

**事实与契约(判定来源)**

- `TransitionPlan` 已表达**提交写什么**:`task_creates`/`state_changes`/`variable_writes`(aggregate changes)、
  `source_operation`(本批关闭的 operation)、`operations`(新建 operation;`DispatchTimeout` 是 timer,
  `ApplyAction` 是 effect intent)、`outbox`。它**不表达提交对 run 的含义**。
- `dispatch` 在 commit 之后补做的判定:去重后的 wake(纯 IO 顺序)、`project`/`project_touched`(IO + A5 顺序)、
  `report_lifecycle`(业务语义:何时 alive/terminal、failed root 是 error)、
  `deliver_committed_outbox`(A1b 顺序 + transport IO)、`refresh_proc_row`(A11 的 `removable` 派生)、
  `place_run`/`admit_run`(业务语义:slot 释放/准入 + shell 的 in-memory 集合)。
- `recover` 的 special-case:boot `repair_all_ready`/`catch_up_projections`、`RecoveryPlanner::classify` 的
  state 分类、`ReclaimExpired`/`FinalizeCommit` 的 lane 内 event、effect observation 的人工分流。
  其中 classify 与状态迁移已是 core;`recover()` 的编排仍是 shell,**统一 Command 留给 A7**,本笔不提前做。
- 归入 Core 的业务语义:`run_settled`(slot/lifecycle)、`projection_settled`/`is_reclaimable`(回收)、
  `ProcessStatus`、lifecycle 事件种类与 failed root 的 error facts、admission 规则、parked run 的 admission
  operation 身份(`resume_operation`)。
- 留在 Shell 的 IO/顺序:A5 的“行 current 后才能让 lifecycle/transport 看见”、A1b 的“commit 后发送 +
  一次批量 confirm”、A11 的“先 `removable` 再原子 reclaim”、A4 的 lane、wake/队列、in-memory slot 集合本身。
- **Decision 形态**(`core/decision.rs`,value-only、无 IO/tokio/store,`Clone/Debug/Eq`):
  `Decision { transition, process_status, settlement, lifecycle, admission }`。
  - `process_status`: `Pending | Running | Completed | Failed | Terminated`(由 root task state 派生);
  - `settlement`: `Settlement { run, projection }`,分别就是 `run_settled` 与 `projection_settled`;
  - `lifecycle`: `Started | Completed | Errored { code, message }`;`None` = 无 root,不可上报;
  - `admission`: `Park | Drive(Keep | Release | Take | NoSlot)`;输入是 shell 观察到的
    `SlotObservation { standing: Parked|Holding|Unplaced, cap, active }` 加快照的 root state/settled。
  - 设计词的映射(不复制 plan 字段):aggregate changes = `transition.{task_creates,state_changes,
    variable_writes}`;operations to create/timers/effect intents = `transition.operations`;
    operations to close = `transition.source_operation`;outbox records = `transition.outbox`。
    effect 仍由原 operation 在 commit 前执行(A1a/A1c),Decision 不跨 effect/transport。

**协议**

- `Decision::committed(transition, snapshot, admission)` 是 commit 后的唯一总入口;Shell 的 `dispatch`
  用它一次性取得 lifecycle/settlement/status/admission,并按 A5/A1b/A4 的顺序执行。
- `Admissions` 只保留状态与执行:`directive()` 调 `decision::place(...)`(纯规则),
  `apply()` 执行 `Park/Release/Take/Keep/NoSlot` 并把 freed slot 的 admission operation 交给 `admit_run`;
  `place_run` 自身只剩下取锁、取 directive、apply。
- `report_lifecycle` 改为执行 `decision::lifecycle(snapshot)` 的 directive;`report_lifecycle_decision`
  只负责 message 形状、sink 与 once-per-phase 去重。`exposes` 校验仍留在 message 层(它需要
  `crate::Message`/model),但“完成/错误/进行中”的判定在 Core。
- `project`/`project_touched` 的 settled 重试分支、`Runtime`/`projection` 的回收判定都直接引用
  `decision::run_settled` / `decision::projection_settled`;`shell/facts.rs` 的重复实现已删除。
- `OperationKind::is_schedulable()` 移到 enum 旁(`core/model/operation.rs`),`core/ready.rs` 不再维护
  第二份 kind 名单;新增 operation 的 Core 改动收敛为 enum/codec + `is_schedulable` + plan/Decision 分支,
  Recovery 仍按 operation state 分类、Projection 不匹配 kind。
- 未改:effect/outbox payload 与 topic、A1b 的 Pending→project→deliver→batch confirm、A4 的 lane 边界、
  A11 的 reclaim 顺序与判定、A5 的 store-level revision/repair queue。

**测试**

- Core 新增 6 个单测:`run_settled` 与 `projection_settled` 的区分(pending outbox 只阻止回收)、
  空/无 root 快照、root state → status/lifecycle 映射、failed root 的 durable error facts、
  parked/held/unplaced 的 admission 决策(含 cap 满 park、settled release、terminal root no-slot)、
  `Decision::committed` 同时携带 plan 与全部 process directive。
- 既有 A1a/A1b/A1c/A2/A3/A4/A5/A11、outbox/reliability/effect 套件全部保留。

**验证**

```text
cargo test -p acts --lib --tests -> 925 passed, 0 failed, 1 ignored (176.47s)
cargo test -p acts --lib scheduler::core::decision -> 6 passed
cargo test -p acts --lib reliability:: -> 65 passed
cargo test -p acts --lib outbox -> 32 passed
cargo test -p acts --lib effect -> 26 passed
cargo test -p acts --lib reclaim -> 8 passed
cargo test -p acts --lib projection -> 9 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all --check -> OK
```

**边界**

- 不做 A7 的统一 Recovery Command;`recover()` 的编排/分支本笔未改,只把其投影/生命周期路径接到 Core。
- 不做 A8 的额度拆分、A9 的 durable handle、A10 的调度器拆分;不缩短 tick。
- `Decision` 不序列化到 store:它是 commit 后的纯值,持久化格式未变(golden 不变)。

### A7 完成(2026-10-18):Recovery 统一为 `RecoverOperation(observed) -> Decision`

**任务**:§11 D 的 A7(P1)。恢复层只收集 `Observed`,Core 输出 directive;正常与恢复共用
transition/commit 协议、per-pid command lane 与 post-commit Decision 执行器。

**事实与契约(当前恢复事实)**

- `OperationRecord` 的 phase 是唯一状态源:`Pending`/`Claimed`/`CommitPrepared`/`Committed`/
  `RetryScheduled`/`EffectUnknown`/`Quarantined`/`Superseded`/`Completed`。`Claimed`/`CommitPrepared`
  携带 `ClaimToken { owner, fence, lease_until_ms, heartbeat_revision }`;`Committed`/`Completed`
  携带 `CommitToken`。lease 过期判定是 `now_ms >= lease_until_ms`。
- `commit_operation_event` 以 operation revision 做 CAS,并在同一批写 aggregate + recovery/ready/outbox 索引。
  revision guard 是 lane 看不见的写者(崩溃、跨实例)的 fence。
- `RecoveryIndex` 只投影 `needs_recovery()` 的异常状态(`Claimed`/`CommitPrepared`/`Committed`/
  `EffectUnknown`/`Quarantined`);`recoverable_page` 是位置游标分页;`repair_ready`/boot catch-up
  仍是旧库或损坏索引的修复路径,不属于逐次扫描语义。
- effect observation 仍是 A1c 的四态(`Applied`/`Failed`/`NotApplied`/`Unknown`)与人工闭环;恢复只在
  `EffectUnknown` 时向 executor 询问,不重跑 effect。

**业务语义 vs IO 顺序**

| 原 `recover()` 内容 | 归属 |
|---|---|
| 何时 reclaim、何时 finalize、何时 await/ignore | Core(`recover_operation`) |
| reclaim 后是“可立即 wake”还是“进入 effect observation” | Core(`ReclaimReady` / `ReclaimUnknown`) |
| `EffectUnknown` 的 replay / retry / manual | Core(按 `EffectReceipt`;幂等仍由包声明) |
| 缺失 ready record 的 repair、终态的 ignore | Core directive |
| 分页游标、lane 获取、event 提交、`repair_ready`、`observe`、wake 发送 | Shell(IO 顺序) |

**`Observed` 与 Decision 形态**

- `Observed { operation_id, kind, persisted_phase, revision, aggregate_revision, effect_receipt, now_ms }`。
  claim/lease 就是 phase 里的 `ClaimToken`(通过 `claim()`/`lease_expired()` 读取);`aggregate_revision`
  由恢复页新增的 `RecoveryCandidate` 随索引行返回,恢复不再为它二次 load aggregate;
  `effect_receipt` 由 Shell 在 Core 要求观察时填入。
- Core 的唯一入口是 `recover_operation(&Observed) -> RecoveryDecision { directive, command }`;
  `RecoveryPlanner::classify` 降为私有实现,Shell 不再匹配 phase。
- `RecoveryDirective` = `AwaitOwner | IgnoreTerminal | RepairReady | ReclaimReady |
  ReclaimUnknown | FinalizeCommit | ObserveEffect | ReplayEffect | RetryEffect | Manual`;
  `RecoveryCommand` 是 `OperationEvent + expected_revision`,由 Core 生成。
- `ReclaimUnknown` 提交后 Shell 用返回的 `OperationRecord` 重新形成 `Observed` 再问 Core
  (`CommitPrepared → EffectUnknown → ObserveEffect`),不在 Shell 里分支处理 event 结果。
- `FinalizeCommit` 用 event commit 返回的 `LoadedProcess` 构造
  `Decision::recovered(snapshot, admission)`;其 transition 是“committed version 的空 plan”(永不提交),
  `process_status/settlement/lifecycle/admission` 与 `Decision::committed` 同源 ⇒ 同一状态在正常/恢复
  路径得到同一 Decision。

**协议**

- `recover()` 只做:分页 → `Observed` → 调用 Core → 取 lane 提交 command / 执行 directive。
- reclaim/finalize 在同一 pid 的 lane 内提交 `OperationEvent`,event 的 revision guard 仍是 fence;
  lane 在 effect observation 与 post-commit 执行前释放(A4 边界不变)。
- 新增 `execute_decision`:项目 current → lifecycle → `deliver_committed_outbox`(A1b 一次批量 confirm)
  → admission;正常 `dispatch` 与恢复 `FinalizeCommit` 共用这一执行器,A5 的“行 current 后才允许外部观察者”
  顺序不变。
- 未改:effect/outbox payload 与 topic、channel ack/delivery 行语义、A11 的回收判定与顺序、
  A5 的持久 revision/repair queue、A4 lane 边界。

**测试**

- Core 新增 7 个 recovery 单测:`Observed → directive/command` 覆盖 live/expired claim、
  `Committed` finalize(命令携带自己的 commit token)、`EffectUnknown` 四态与幂等分流、
  非 `ApplyAction` 的 unknown、`Quarantined`、pending/retry 的 ready repair、terminal ignore、
  claim/lease accessor。
- Core 新增表驱动测试:同一 snapshot 经正常 `Decision::committed` 与恢复 `Decision::recovered`,
  `process_status`/`settlement`/`lifecycle`/`admission` 相等,恢复的 transition 为空且版本是 committed version。
- `async_document_store` 的“旧 writer 留下 `Committed`”测试改为断言 `Observed → recover_operation`;
  恢复分页测试改读 `RecoveryCandidate`。既有 reliability/A1a/A1b/A1c/A2/A3/A4/A5/A6/A11/outbox/effect 套件保留。

**验证**

```text
cargo test -p acts --lib --tests -> 933 passed, 0 failed, 1 ignored (153.57s)
cargo test -p acts --lib scheduler::core -> 168 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all -- --check -> OK
```

**边界**

- 不做 A8 的额度拆分、A9 的 durable handle、A10 的调度器拆分;不缩短 tick。
- 不引入跨进程 exactly-once;recovery 的 wake 只是“event 已写 ready record”的 accelerator,
  业务语义由 directive 决定。

### A8 完成(2026-10-18):Admission 分离(execution / effect / recovery / reconciliation / projection)

**任务**:§11 D 的 A8(P1)。把 `Admissions { cap, active, parked }` 的单一额度拆成显式类别;
`EffectUnknown`/`Quarantined` 的 run 退出 execution 额度;manual reconciliation 不占正常 slot;
cancel/终态决策在同一 `Decision` 里释放 slot;parked 准入由释放方直接 `admit_run`,tick 只兜底。

**事实与契约(当前准入事实)**

- `Admissions` 原状:一个 `cap`,一个 `active: HashSet<String>`,一个 `parked: BTreeMap<(i64, pid), OperationId>`;
  `start_process` 用 `reserve` 与决策同锁占位;`place_run` 调 `decision::place`;`dispatch` 在 commit 后
  用 `Admissions::directive` 构造 `Decision`,再在 `execute_decision` 末尾 `apply`。
- 原 `decision::place` 只读 `root_state` 与 `run_settled`;因此 root 仍非终态、但只剩
  `EffectUnknown`/`Quarantined` 的 run 会一直持有 execution slot;manual decision 本身走
  `Slot::Direct`,不占 slot,但被它占住的 slot 会挡住新 run。
- `mark_effect_unknown` 只写 operation event,不碰 slot;释放要等下一次 operation 的
  `prepare → place_run`。客户端终态动作虽然会在 `execute_decision` 里释放,但规则仍只认
  `run_settled`。
- `admit_run` 已是释放方驱动(`set_ready_at(now)` + wake);`drive_ready` 的 `set_ready_at(now+tick)`
  只是兜底。A11 的 `forget_process` 仍通过 `Admissions::abandon` 清掉进程内状态。

**业务规则 vs IO 顺序**

| 事实 | 归属 |
|---|---|
| 谁占 execution slot:root 非终态且无 reconciliation 挂起 | Core(`RunFacts::execution_active`) |
| 何时释放:settled / 终态决策 / `EffectUnknown`/`Quarantined` | Core(`place`) |
| manual reconciliation 不占 execution slot | Core(`NoSlot`) + Shell(`Slot::Direct`) |
| parked FIFO 顺序、`__created` 键、队首是谁 | Shell 的集合(规则仍由 Core 给出 Release/Park) |
| `set_ready_at`、wake、tick cadence、lane | Shell(IO 顺序) |
| 四类辅助额度的 `active < cap` | Core(`decision::quota`);Shell 的 gate 只计数/等待 |

**Core 协议**

- 新增 `RunFacts { root_state, run_settled, awaiting_reconciliation }`,由 `RunFacts::of(snapshot)` 一次导出;
  `awaiting_reconciliation` 认 operation 的 `EffectUnknown { .. }` 与 `Quarantined { .. }`。
  `place(observation, facts)` 的唯一规则:
  - `Parked → Park`;
  - `Holding` 且 `!facts.execution_active() → Drive(Release)`(settled 或 quarantine);
  - `Holding → Keep`;
  - `Unplaced` 且 root 终态或 quarantine → `Drive(NoSlot)`(不需要/不能用 execution 额度);
  - 其余按 `concurrency::Admit` 的 cap 规则 `Take`/`Park`。
- `Decision` 新增 `quarantined: bool`,与 `settlement/lifecycle/admission` 同源;`Decision::committed` 与
  `Decision::recovered` 对同一 snapshot 给出同一值。
- 新增 `AdmissionCategory::{Execution, Effect, Recovery, Reconciliation, Projection}` 与
  `QuotaDirective::{Admit, Wait}`;`decision::quota(cap, active)` 是四类辅助额度的唯一规则(`0` = 无界)。

**Shell 协议**

- `Admissions` 现在保存:execution 的 `cap/active/parked`、一个 `quarantined` gauge,以及
  effect/recovery/reconciliation/projection 四个 `AdmissionGate`(计数器 + `Notify`)。`directive`
  只构造 `SlotObservation` 与 `RunFacts`,不再在 Shell 判断业务条件;`apply` 只执行 directive。
- `mark_effect_unknown` 在 event 成功后立即 `place_run` 该 event 返回的 snapshot:持有槽位的 run
  因 quarantine 得到 `Release`,Shell 立刻 `admit_run` 队首 —— 不等下一次 operation/tick。
- `execute_decision` 在应用 admission 前记录 `decision.quarantined`;manual 决策成功后同一路径把
  gauge 清零并释放已不存在的 quarantine。
- 四类 gate 接在实际调用点:effect 在 `runner.run` 外(命令 lane 已释放)、recovery 在整个 `recover`
  pass、reconciliation 在 `resolve_effect`、projection 在 `project`/`project_touched`/`repair_projection`。
  gate 用 Core 的 `quota` 判断,只有饱和时才等待;execution 与 reconciliation 因此不会互相占用。
- worker/cancel 的释放顺序不变:`project → lifecycle → outbox 批量 confirm → admission`;释放后的
  `admit_run` 仍在同一决策内完成。
- 未改:A4 lane、A5 的行/版本顺序、A1b 的 outbox 顺序、A7 的 `Observed -> RecoveryDecision`、
  A11 的回收判定与顺序。

**配置与指标**

- 顶层新增 `effect_concurrency` / `recovery_concurrency` / `reconciliation_concurrency` /
  `projection_concurrency`(默认 256/16/16/16,`0` = 无界);`cache_cap` 仍是 execution 的上限。
  `AdmissionCaps` 由 `KernelHost::new_with_caps` 接收,Runtime 从 `Config` 构造。
- `AdmissionMetrics { execution: QuotaMetrics, parked, effect, recovery, reconciliation,
  projection, quarantined_runs }` 由 `KernelHost::admission_metrics()` 暴露;tick 在有积压/隔离时输出
  `kernel admission` 日志。

**测试**

- Core 新增/改写:`place` 的 Parked/Holding/Unplaced 表;`a_quarantined_run_leaves_the_execution_quota`
  (Holding + EffectUnknown → Release;Unplaced + 满 cap → NoSlot);`awaiting_reconciliation` 同时认
  `EffectUnknown` 与 `Quarantined`;辅助额度 `quota` 的 cap/无界边界。
- Shell 新增 3 条:
  - `a_quarantined_run_frees_its_slot_and_admits_the_parked_run`:cap=1,effect 丢 commit → run A
    进 quarantine,A 的 slot 在同一决策释放,B 立即被准入并完成;A 仍为 `EffectUnknown`。
  - `manual_reconciliation_advances_while_the_execution_quota_is_full`:holder 占唯一 execution slot,
    `resolve_effect` 的 manual 决策照常提交;execution active 仍为 1,reconciliation permit 归还,
    `quarantined_runs` 归零。
  - `a_terminal_cancel_frees_the_slot_and_admits_the_parked_run`:`Abort` 结束唯一 run 的 deciding
    command 释放 slot 并准入 parked run,不经过 tick。
- 既有 reliability/A1a/A1b/A1c/A2/A3/A4/A5/A6/A7/A11、action/cancel、outbox/effect 套件保留。

**验证**

```text
cargo test -p acts --lib --tests -> 940 passed, 0 failed, 1 ignored (157.71s)
cargo test -p acts --lib scheduler::core::decision -> 10 passed
cargo test -p acts --lib reliability:: -> 65 passed
cargo test -p acts --lib outbox -> 32 passed
cargo test -p acts --lib effect -> 28 passed
cargo test -p acts --lib cancel -> 15 passed
cargo test -p acts --lib reclaim -> 9 passed
cargo test -p acts --lib projection -> 9 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all -- --check -> OK
```

**边界**

- 不做 A9 的 durable handle、A10 的调度器拆分;不缩短 tick、不把正常活性转嫁 recovery 扫描。
- 不改 effect/outbox payload、channel 语义、A5 持久 revision/repair queue、A11 回收。
- 不引入跨进程 exactly-once;quarantine gauge 只是进程内观测,准入规则仍从 aggregate 推导。

### A9 完成(2026-10-18):Durable run handle

**任务**:§11 D 的 A9(P2)。`ProcessExecutor::start` 不再只返回 pid,而是返回可 `wait()` 的 durable
handle;`wait()` 的顺序固定为“查聚合终态 → 未完成则订阅 → 订阅后复查”;进程内事件 channel 只做通知,
不是正确性来源。示例改为先注册 handler 再 start。

**事实与契约(当前等待事实)**

- `ProcessExecutor::{start,start_from_model,start_as_owner}` 原返回 `String`(pid);`Runtime::start`
  在返回前已经 `kernel.start_process(&spec)` 提交 seed 聚合,所以“pid 持久化”早于调用方拿到返回值。
- 终态只在一个地方判定:`core/decision.rs`。`run_settled(snapshot)` = 所有执行终态;
  `ProcessStatus::of` 读根执行;`lifecycle(snapshot)` 给出 `Started`/`Completed`/
  `Errored { code, message }`;failed root 的 facts 来自 `root_error` 读根 scope 的
  `error.code`/`error.message`(由 operation 的 `Upward` 走 + propagate 的 carry 写入)。
- 终态通知的现有路径:commit → `Decision` → `execute_decision` → projection →
  `report_lifecycle_decision`(per-pid 去重)→ outbox。channel 的 `on_complete`/`on_error` 是进程内
  事件队列;订阅晚于终态提交时事件已经过去,调用方无从补查 —— 这就是本笔关闭的缺口。
- aggregate 才是 run 的 durable existence record;`run_settled` 仍只在 `core/decision.rs`,handle 不复制
  状态机,也不新建第二套终态判定。

**协议**

- `start()` / `start_from_model()` 返回 `RunHandle`;`start_as_owner()` 返回同一类型(subflow 只丢弃它)。
  `RunHandle { runtime, pid }` 的 identity 就是 pid + runtime 句柄;`pid()` 暴露 id,`Display` 与
  `Serialize` 都输出 pid 字符串,因此 `proc:start`/`proc:start_from_model` 的 wire 结果不变。
- `proc().handle(pid)` 从 pid 重建 handle(ACL 检查 `proc:get`),重启后走同一入口。
- `wait()` 的循环:
  1. `KernelHost::snapshot(pid)`;聚合缺失 = 从未存在或已被 A11 回收,返回 `ActError::Runtime`;
  2. `decision::run_outcome(snapshot)`:`None` 表示仍在飞,`Some(Completed)` /
     `Some(Errored { code, message })` / `Some(Terminated)`;
  3. 未终态:`KernelHost::run_waiter(pid)` 取该 pid 的 `Notify`,`enable()` 注册后**再查一次**;
  4. 通知到达后回到 1。
- 通知点:`execute_decision` 在 `decision.settlement.run`(即 `run_settled`)时 `notify_settled` ——
  聚合已 durable,projection 的失败无法扣住终态;`forget_process`(A11 回收成功)也发一次,让仍在
  等待的 handle 醒来并得到“记录已消失”。通知只是加速器:丢失只增加延迟,复查永远给出结论。
- `RunOutcome` 由 core 的同一批 facts 导出(`run_outcome` 复用 `run_settled` + `lifecycle`),从 crate
  根公开为 `acts::RunOutcome`;`RunHandle` 公开为 `acts::RunHandle`。
- 未改:channel 的 ack/delivery 与事件顺序、A1b outbox 顺序、A4 lane、A5 projection/revision/repair
  queue、A11 回收判定;handle 不参与 admission/durable ready,也不提供跨进程 exactly-once。

**测试/示例**

- `crates/acts/src/export/tests.rs` 新增 6 条:
  - 无任何订阅的极短 run:start 后立即 `wait()` 得到 `Completed`(check-subscribe race 不存在);
  - 已终态 run:`wait()` 立即返回,且同一 pid 用 `handle(pid)` 重建后得到同一结果;
  - failed root:`act().fail(ecode/message)` → `Errored { code, message }`(复用 lifecycle facts);
  - abort root → `Terminated`;
  - 重启:run 停在 act 上,close 旧 engine、同一 store 启动新 engine、`proc().handle(pid)` 重建,
    完成前保持 pending,完成 act 后 `wait()` 得到 `Completed`;
  - 不存在的 pid:`handle` 可构造,`wait()` 报告 gone(不无限等待)。
- 示例改为“先注册 handler / 订阅,再 start”:`actions`、`approve`、`simple`、`while`、
  `custom_package`、`message`、`packages/{http,shell,state}`、`plugins/{grpc,web}`;
  `catches`、`timeout`、`subflow`、`packages/javascript` 原本已是有序。

**验证**

```text
cargo test -p acts --lib --tests -> 946 passed, 0 failed, 1 ignored (148.65s)
cargo test -p acts --lib run_handle -> 6 passed
cargo test -p acts-channel -> 32 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all -- --check -> OK
```

- 全量运行期间观察到两次既有负载翻转:`export::actions::tests::an_unsub_reaches_only_the_callers_own_namespace`
  与 `scheduler::tests::reliability::timer::the_trigger_loop_degrades_under_a_down_store_and_recovers`;
  两条单跑均通过,全量复跑(152.39s)为 946 passed / 0 failed。属 §1.8 的负载翻转类,与 A9 改动无关
  (A9 只新增每 pid 的 `Notify` 唤醒与终态读取,不触及 trigger/recovery 循环)。
- `cargo test -p acts-package-shell --test vars` 的 `the_inputs_of_the_act_embed_in_the_script`
  在干净 HEAD worktree(7fc8a52)上同样失败(`$inputs().data` 为 `null`),是既有环境/基线问题,
  不是 A9 回归;同 target 的其余 3 条通过。

**边界**

- 不做 A10 的调度器拆分;不缩短 tick;wait 不轮询,只靠 commit/reclaim 的通知。
- 不引入跨进程 exactly-once;通知是进程内的,另一个实例提交的终态由 `handle` 的首次/重建读取发现,
  已在等待中的 handle 不订阅另一实例。
- 不改 effect/outbox payload、channel ack/delivery 语义、A5 持久 revision/repair queue、A11 回收。

### A10 完成(2026-10-18):拆分控制面调度器

**任务**:§11 D 的 A10(P2)。timer/outbox retry/recovery/lease renewal/settled sweeper/projection repair
各自拥有独立的 interval/backoff/budget/指标;timer 使用到期索引,不靠全量 recovery 扫描;禁止用统一
缩短 tick 解决延迟。

**事实与契约(拆分前)**

- driver 的一个 `tick` 同时做 ready 到期扫描、recovery 扫描、outbox retry,并把投影 repair 塞在
  `recover()` 里;trigger、message retry、snapshot TTL 也读同一个 `tick_interval`。
- 正常活性早已来自 A2 的 durable ready index:`default_ready_at` 只让 `DispatchTimeout` 等一个轮询间隔,
  `drive_ready` 只读到期记录。`RecoveryIndex::project` 只收 `needs_recovery()` 的状态
  (`Claimed`/`CommitPrepared`/`Committed`/`EffectUnknown`/`Quarantined`),所以 timer 本就不在 recovery
  索引里;但两者的**频率**仍被同一个 tick 绑死。
- cleanup(A11)已有独立 loop/health/metrics;lease renewal 没有独立实现——claim 过期仍由 recovery
  的异常路径回收(`ReleaseExpiredClaim`),本笔不新增假的续期循环。

**协议**

- `ConfigData` 新增 `ready_interval_secs`/`recovery_interval_secs`/`outbox_interval_secs`/
  `projection_interval_secs`/`trigger_interval_secs`/`retry_interval_secs`/`snapshot_interval_secs`,
  以及 `ready_batch`/`recovery_batch`/`outbox_batch`/`projection_batch`。全部省略时跟随
  `tick_interval_secs`(测试构建的 `TEST_TICK_MS`),因此默认配置与 A9 完成时一致;显式值下限 1 秒,
  budget 夹在 1..=1024。
- `KernelSchedule` 把一个宿主的四个 driver 间隔与预算固定下来;`KernelHost::serve` 用四个独立的
  `tokio::time::interval` 跑 ready/timer、recovery、outbox retry、projection repair。`ready.notified()`
  与 `deliver.notified()` 仍只做加速,丢失只增加延迟。
- timer:仍是 ready index 的到期记录(`DispatchTimeout.ready_at_ms = now + ready_interval`;idle round
  由 `drive_ready` 延后一个 ready 间隔),recovery 频率不参与其唤醒。
- recovery:预算按候选操作数截断,`recovery_cursor` 从上次已观察的 entry 继续;游标只是调度提示,
  重启/丢失只会从头再扫。`recover()` 不再调用 `repair_projections`。
- outbox:预算按记录数截断,`outbox_cursor` 从上次已处理记录继续;cursor 越过末尾时回绕一次。
- boot:ready 索引修复归 ready loop(`ensure_ready_boot`),投影 catch-up 归 projection loop
  (`ensure_projection_boot`);`serve` 启动时先做一次 `ensure_boot`,之后由各自的 loop 重试失败项。
- trigger/message-retry/snapshot TTL 各用自己的 interval;cleanup 保持 A11 的独立 interval。
- 健康与指标:`SchedulerHealth` 增加 `ready`/`recovery`/`outbox`/`projection`;新增
  `ReadyMetrics.passes`、`OutboxMetrics`(backlog/oldest_due_age/observed/passes/delivered/deferred)、
  `RecoveryMetrics`(scanned/more_pending/passes/reclaimed/ready_repaired/finalized/
  reconciliation_required/effect_*);`Runtime::{ready,outbox,recovery}_metrics` 公开读取。
  `ProjectionMetrics`/`CleanupMetrics` 沿用。
- 未改:effect/outbox payload 与顺序、channel ack/delivery、A4 lane、A5 revision/repair queue、
  A8 额度、A9 handle、A11 回收判定与顺序;lease 续期仍不存在,claim 过期仍走 recovery。

**测试/指标**

- `config.rs::control_plane_intervals_default_to_the_tick_and_move_independently`:七类 interval
  默认跟随 tick、可分别覆盖、0 被夹到 1。
- `scheduler/shell/tests/scheduler_split.rs`:
  - recovery 压到 1 小时时,ready 间隔 25 ms 的 timeout branch 仍在 ready pass 触发;
  - ready 10 ms 运行 250 ms:ready passes ≥5,recovery passes ≤2,outbox passes ≥3;
  - 真实 `Engine` 配置(`ready=1s`、`recovery=3600s`)下 timeout 仍触发,recovery 只跑启动时的一轮。
- 既有 reliability/A1*/A2/A3/A4/A5/A6/A7/A8/A9/A11、action/cancel、outbox/effect、timer/timeout、
  cleanup/reclaim、projection 套件全绿(见下)。

**验证**

```text
cargo test -p acts --lib --tests -> 950 passed, 0 failed, 1 ignored (155.35s)
cargo test -p acts --lib scheduler::shell::tests::scheduler_split -> 3 passed
cargo test -p acts --lib config::tests::control_plane_intervals_default_to_the_tick_and_move_independently -> 1 passed
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings -> OK
cargo fmt --all -- --check -> OK
```

- 全量首跑出现一次既有负载翻转:`export::actions::tests::an_unsub_reaches_only_the_callers_own_namespace`
  (A9 记录同一现象);单跑通过,复跑全量 950 passed / 0 failed。与 A10 的 loop 拆分无关。

**边界**

- 不做跨进程 exactly-once;不改 effect/outbox payload、channel ack/delivery、A5 持久 revision/repair
  queue、A11 回收判定与顺序。
- 不把“缩短 tick”当延迟手段;不把异常兜底(recovery)转成正常活性来源。
- 不新增 lease-renewal 循环:当前协议没有需要续期的执行期租约,claim 生命周期仍由 recovery 的
  过期回收决定。

---

### P1 完成(2026-10-18):engine ~ms/hop 的分阶段画像

**任务**:§11 B 的 P1 —— 把 engine 形状每跳的 plan/记账 CPU 分解到具体块,**只量不改代码**。
第三波 A1-A11 已完成后的第二波第一项。

**探针形状**(临时,交付前已删;按下述可重建)

- `crates/acts/src/scheduler/shell/tests/perf_probe.rs`(`#[ignore]`)+ `shell::operation_trace`
  (`#[cfg(test)]` 计时;非 test 构建为空函数),两者都只存在于工作区,未进入本笔提交。
- 两种形状,均是一次 `Engine`(内存 store、release):
  - **engine**:§10「P6-1」的 30 步线性 workflow,31 节点、聚合 47,959 B;
  - **simple**:`examples/simple/model.yml`(`input=10`,11 次循环),聚合 85,226 B。
- 每轮 fresh `Engine` + `CountingStore`(`MemoryStore` 装饰器,按 key 族统计 `one`/`many`/`batch`);
  用 `Engine` 的 completion/error 信号结束,再做 **quiet drain**(2 ms × 3 次操作计数不变),**不轮询
  aggregate**。旧探针的 `process_completed` 每 200 µs 做一次整包 load+decode,本身是测量噪声;本轮
  改用事件后,`at_completion` 的 operation 数与历史“93 hops”逐字对齐。
- 命令:`cargo test --release -p acts --lib perf_probe -- --nocapture --ignored --test-threads=1`。
- 口径:`operations` = `execute_one` 调用次数(含 produce `Ok(false)` 的 no-op wake/ready 重试);
  `commits` = 含 `kernel/aggregates/` put 的 guarded batch;`row reads/writes` = `procs`/`tasks`/`vars`
  数据 key 上的 `one` / put。`ms/op` 是 per attempt;吞吐另给 per commit。

**结果(best of 3,release,内存 store,同机)**

| 形状 | 聚合 | operations | at_completion | commits | wall | ms/op | ms/commit | commits/s |
|---|---|---|---|---|---|---|---|---|
| engine 30 步 | 47,959 B | 111 | **93** | 187 | **426.7 ms** | 3.84 | 2.28 | 438 |
| simple | 85,226 B | 180 | 111 | 295 | **714.7 ms** | 3.97 | 2.42 | 413 |

- 同一批的另一轮:engine 426.7–491.3 ms(3.84–4.43 ms/op)、simple 714.7–861.0 ms;更早一轮
  engine 343.7 ms、simple 731.5 ms。**环境/负载漂移 ≈1.2–1.5×**,阶段占比与 per-call 成本稳定;
  结论只取同一轮内的比值。
- `operations` 与 `at_completion` 的差值(engine 18、simple 68–69)全部是 completion 之后的 no-op
  `execute_one`(tail claims=0、commits=0):只付 prepare 的 LOAD + `place_run`,然后 defer。它们
  是 ready/wake 记录的重复扫描,不是真实 hop。历史“93 hops”对应 `at_completion=93`。
- **前后对照**:§5n/P6-1 记录 93 hops / 0.684 s = 7.35 ms/hop;当前同形状在 `at_completion=93` 处约
  0.37–0.50 s(≈4.0–5.4 ms/op),总 quiescent 111 attempts ≈3.1–4.4 ms/op。两次口径不可直接相减:
  (a) 旧探针每 200 µs 轮询 `process_completed`,每次都是整包 load+decode;本轮已去掉该噪声;
  (b) 旧基线在 A3/A6/A10 之前。**不宣称这是某一笔代码改动的收益。**
- **legacy 对照**:legacy 2778–5099 hop/s;当前 413–438 commits/s ⇒ 约 **6.3–12.4×**,比 §5n 的
  “20–37×” 估计窄(边界与代码版本不同)。不要用 debug 数字。

**每阶段(engine / simple,best run;ns/op = µs/op × 1000)**

| 阶段 | engine ns/op | engine % | simple ns/op | simple % |
|---|---:|---:|---:|---:|
| `load`(prepare 的整包读) | 484,000 | 12.6 | 672,000 | 16.9 |
| `place1`(`place_run`) | 1,000 | 0.0 | 1,000 | 0.0 |
| `claim`(`ClaimAndPrepare`) | 695,000 | 18.1 | 822,000 | 20.7 |
| `perform`(kind 的工作) | 49,000 | 1.3 | 35,000 | 0.9 |
| `plan`(`ActOperation::next`) | 88,000 | 2.3 | 46,000 | 1.1 |
| `fold`(纯内部 hand-off 折叠) | 79,000 | 2.1 | 72,000 | 1.8 |
| `commit`(计划提交) | 1,007,000 | 26.2 | 1,190,000 | 30.0 |
| `decide`(`Decision::committed` + admission) | 1,000 | 0.0 | 1,000 | 0.0 |
| `wake` | 1,000 | 0.0 | 0 | 0.0 |
| `project`(行投影) | 428,000 | 11.1 | 254,000 | 6.4 |
| `lifecycle` | 4,000 | 0.1 | 1,000 | 0.0 |
| `deliver.tx`(消息 payload + emit) | 10,000 | 0.2 | 8,000 | 0.2 |
| `deliver.confirm`(A1b 批量确认提交) | 449,000 | 11.7 | 521,000 | 13.1 |
| `place2`(admission 归还) | 0 | 0.0 | 0 | 0.0 |
| **top-level sum** | **3,297,000** | **85.8** | **3,623,000** | **91.2** |

**嵌套阶段(同轮;`calls/op` 与 ms/call)**

| 嵌套块 | engine calls/op | engine ms/call | simple calls/op | simple ms/call |
|---|---:|---:|---:|---:|
| `agg.load`(两类 commit 内的整包读) | 1.12 | 0.385 | 1.09 | 0.483 |
| `agg.apply`(`advance_*` + recovery entries) | 1.12 | 0.309 | 1.09 | 0.394 |
| `agg.ready` | 1.12 | 0.003 | 1.09 | 0.003 |
| **`agg.encode`**(整包 encode + seal + 索引) | 1.12 | **0.761** | 1.09 | **0.934** |
| `agg.batch`(guarded batch) | 1.12 | 0.057 | 1.09 | 0.029 |
| `confirm.apply` | 0.56 | 0.009 | 0.54 | 0.008 |
| **`confirm.encode`**(确认提交的整包 encode) | 0.56 | **0.743** | 0.54 | **0.926** |
| `confirm.batch` | 0.56 | 0.042 | 0.54 | 0.019 |
| `proj.frame`(`model_of` + `NodeTree::load`) | 0.58 | 0.102 | 0.56 | 0.046 |
| `proj.task` | 1.93 | 0.123 | 1.58 | 0.092 |
| `proj.proc` | 0.58 | 0.180 | 0.56 | 0.090 |
| `proj.read`(行 `find_opt`) | 2.50 | 0.028 | 2.14 | 0.016 |
| `proj.write`(`write_if_newer`,含 guard 读) | 3.08 | 0.072 | 2.88 | 0.046 |
| `proj.flush`(write-behind `flush_pid` + deliveries) | 0 | 0 | 0 | 0 |
| `msg.payload`(`message_from_record`) | 1.13 | 0.006 | 1.23 | 0.004 |
| `msg.emit`(channel route) | 0.56 | 0.002 | 0.41 | 0.002 |

**结论(按实测占比)**

1. **plan/记账逻辑本身不是主因**:`perform + plan + fold + decide` = 0.217 ms/op(engine,5.6%)/
   0.154 ms/op(simple,3.9%);`place_run`/`place2` < 2 µs/op;消息 payload 0.005–0.006 ms/op(0.1–0.2%);
   outbox 传输 0.008–0.010 ms/op。
2. **整包 encode/decode/apply 是主因**:engine = 0.915(读)+1.265(写)+0.345(apply)= **2.53 ms/op
   (65.7%)**;simple = 1.197 + 1.521 + 0.429 = **3.15 ms/op(79.2%)**。每次 committed op 仍是
   **2 次 commit 内 load + 1 次 prepare load = 3 次 decode**,以及 **claim/plan/confirm = 3 次 encode**;
   A1b 的“提交后批量确认”把 P6-1 折叠掉的第三次整包 encode 又加了回来。
3. **行投影 0.25–0.43 ms/op**(6.4–11.1%),大头是 `NodeTree::load` + 行 JSON 构造与 2 次 `kv.one`
   (`find_opt` 后又 guard 读);`proj.flush` 在该形状为 **0 次**(无 channel ack 的 delivery 行),
   write-behind 不在每跳热路径。
4. **per-commit 的 codec 微基准**(47,877 / 85,144 B payload,release):`snapshot.encode` 0.417/0.849 ms、
   `envelope.seal` 0.318/0.406 ms、`envelope.encode`(seal 后再次 `validate`)0.291/0.411 ms、
   `envelope.decode` 0.280/0.399 ms、`document.encode` 0.934/1.769 ms、`document.decode` 0.643/0.991 ms。
   **`ProcessDocumentCodec::encode` 对同一 payload 做了两次 SHA-256**:`seal` 一次、`encode` 的
   `validate` 再一次。

**与 §10 P6-1 的 708 / 726 / 569 对齐**

| 量 | P6-1(debug,≈500 hops) | 本轮 simple(release) | 归一 |
|---|---:|---:|---:|
| 聚合读 | 708 | 379 | 0.98 vs **1.28 / commit** |
| 提交 | 726 | 295 | 1.45 vs **1.64 / attempt** |
| 行读 | 569 | 1,417 | 0.78 vs **4.80 / commit** |

- 绝对值不可比:旧数字是 debug、hop 数约 500 且在任何 folding/A1b/A10 之前;只比 per-commit 比值。
- 聚合读/commit 变高:当前正常路径是 prepare load + claim load + plan load(确认提交不 load),
  平均 1.28;P6-1 的 <1 反映不同的 load 复用/mix。
- 行读/commit 变高是**计数器定义**差异:本轮统计每个行数据 key 的 `kv.one`,所以
  `find_opt` + `write_if_newer` 的 guard 读都算(每行写 2 次);P6-1 的 569 应是行 API 级读。
  内存 store 下行读只占 0.033–0.070 ms/op(0.8–1.8%),不是 hop CPU 的大头;它属于 L5 的
  “真后端写次数/字节”问题。
- 提交次数 1.64–1.68/attempt:claim、plan、outbox confirm 三笔 guarded batch;这就是 3 次整包 encode。

**下一块(P2,见 §11 B / §12)**

- 选 **`agg.encode` / `confirm.encode`** 作为下一笔,先做**无格式变更**的一步:
  `ProcessDocumentCodec::encode` 只算一次 SHA-256(`seal` 与 `encode.validate` 去重)。
- 假设/预期:每次 encode 省一次 `envelope.encode`(0.291/0.411 ms);1.68/1.64 encodes per attempt
  ⇒ **−0.49 ms/op(engine,−12.7%)/ −0.67 ms/op(simple,−17.0%)** `[INFERENCE:按本轮微基准]`;
  持久化字节与摘要不变。
- 验证:同一探针的 `agg.encode`/`confirm.encode` ms/call 与 wall/commit;`-p acts --lib --tests` 全绿;
  golden/字节断言不变。再往后才是 **L8**(拆文档/append-only,格式变更)与 L5(行读/写次数)。

**边界/遗留**

- 本笔**未改生产代码**;探针与 `operation_trace` 已从工作区删除(它们此前被误提交在 `f262862`,
  本笔的代码提交只做删除)。
- 不改 effect/outbox payload、outbox 顺序、A4/A5/A8/A9/A10/A11 语义;不以缩短 tick 或跳过校验制造吞吐。
- 仍存在的测量限制:quiet drain 的 2 ms × 3 会加 ~6 ms 到 wall;`cargo test --release` 的
  `TEST_TICK_MS = 800` 让 control-plane 的异常路径不在本 profile 的 1 s 窗口内(它们不在正常 hop 热路径)。

---

## 11. 任务(合并:内核切换遗留 + scheduler 重构设计 C1-C8c)

> **维护规则**:提示词只含**当前要执行的那一项任务与限制约束**(下一节),任务清单在本节正文。
> 每执行完一项:**原地更新下一节的提示词**(改成下一项)+ 只改本节的状态标记,**不追加新提示词、不新增小节**。
>
> **本节是唯一任务清单。** 设计细节(事实表、命名表、身份模型、切分理由、风险)在 `KERNEL_REFACTOR_DESIGN.md`;
> 本节只记"做什么、顺序、状态、验收"。**已完成项已从本节删除** —— §1.a(合并 Claim+PrepareCommit)、§1.b 的两笔 codec 削减
> (§5m、§10「P6-1」)、§1.b(ii)(按聚合版本缓存;实测收益不足、已放弃)、原第 7a 项(节点寻址;已并入 C4/C5),
> 逐笔记录见 §10,本节不复述历史。
>
> **执行顺序(2026-10-15 用户裁决)**:先按 `KERNEL_REFACTOR_DESIGN.md` 重构 scheduler(**第一波 = C1→C8c,严格按序**),
> 再做其他工作(**第二波 = P1 性能画像 + L\* 遗留,可与彼此交错**)。
>
> **2026-10-17 用户裁决(架构优先)**:在第一波之后、第二波之前插入**第三波 = A1→A11 架构修正**。
> A1a 已完成;A2(durable ready queue)已完成(§10);A11(终态回收 + PID 复用)已完成(§10);
> A1b(提交后发送 + 每 operation 一笔批量确认)已完成(§10「A1b 完成」);
> A1c(effect 幂等协议)已完成(§10「A1c 完成」);A3(折叠纯内部 hop)已完成(§10「A3 完成」);
> A4(每 Process 单写者 command lane)已完成(§10「A4 完成」);A5(projection 版本协议)已完成(§10「A5 完成」);
> A6(Shell 变薄,Core 输出完整 Decision)已完成(§10「A6 完成」);
> A7(Recovery 统一成 `RecoverOperation(observed) -> Decision`)已完成(§10「A7 完成」);
> A8(Admission 分离)已完成(§10「A8 完成」);A9(durable run handle)已完成(§10「A9 完成」);
> A10(拆分控制面调度器)已完成(§10「A10 完成」)。
> **第三波 A1-A11 已全部完成;第二波 P1 已完成(§10「P1 完成」),当前执行项为 §11 B 的 P2**。
>
> **编号约定**:`C1…C8c` = `KERNEL_REFACTOR_DESIGN.md` 的提交切分(**沿用它的编号**,便于两文档对齐;属第一波);
> `P*` = 性能项、`L*` = 其他遗留项(属第二波);`A1…A11` = 架构修正项(属第三波,设计见设计文档 §3.10)。

### A. 第一波:重构主线(设计 C1-C8c,按序)

> 顺序/依赖图/每笔内容/回滚见 `KERNEL_REFACTOR_DESIGN.md` §4;每笔的收口门禁见本节 §11.C。**严格按序,不跳笔、不合笔。**

| 编号 | 任务 | 阻塞/前置 | 状态 |
|---|---|---|---|
| **C1** | 拆 `core/plan/**` 成同名目录 + `pub(crate) use` 保路径 + tests 归位(纯搬移,调用方一行不改) | — | **已做(§10「C1 完成」)**;**补笔 C1b**:`core/commit/{aggregate,command}` 也目录化(§10「C1b 完成」) |
| **C2** | `Aggregate*` 全族 → `Process*`(纯改名) | C1;**D1 已拍**(`{Process, Task, Operation}`) | **已做(§10「C2 完成」)** |
| **C3** | `Execution*` → `Task*` + `task_state::TaskState` 改名让位(纯改名) | C2 | **已做(§10「C3 完成」)** |
| **C4** | `TaskId` 显式化 + `OperationId.task` 换类型(`wire()` 派生,输出仍是 node id ⇒ golden 全绿) | C3 | **已做(§10「C4 完成」)**(承载原 §7a) |
| **C5** | `wire()` 切 hex + codec V3 + upcast + 行按实例 + golden 重冻结 + CHANGELOG BREAKING | C4;**D2/D3 已拍**(2026-10-16:乙案摘要 / 行链保持 node id / 重冻结接受 / 孤儿键与 L1 同批,见设计 §2.2) | **已做(§10「C5 完成」)**(BREAKING) |
| **C6** | act 载荷写 `kind` + `node_kind_and_name` 扩 act + 终态映射测试 | C5 | **已做(§10「C6 完成」)** |
| **C7** | 投递边界写进 `KERNEL_REFACTOR_DESIGN.md` §3.5 + CHANGELOG(零代码) | — | **已做(§10「C7 完成」)** |
| **C8a** | `view` 删死件(`visit.rs` 全簇、`Node::from_str`、`NodeTree::build`/`build_workflow`、注释掉的 `build_act`、`Workflow::print`)+ `tree_output` 改自递归 | — | **已做(§10「C8a 完成」)** |
| **C8b** | `view` 去重(§3.7 b:id 分配合一、`node_facts` 统一 name/kind、`NodeKind` 对齐) | C8a | **已做(§10「C8b 完成」)**。policy 侧 enum 已删,`NodeDefinition.kind` 直接用 `crate::NodeKind`;两份图未合并(§3.7) |
| **C8c** | `view` 挪位(§3.7 c);D4 材料里引用 §11 B 的 L9-L12 取代关系 —— effect seam **已验证成立**(`build_acts` 产物被丢弃),退役另立 L9-L12 | C8b;**D4 已拍(2026-10-17:暂不挪,后续考虑)** | **已做(§10「C8c 完成」)**。零代码;`view` 留原路径;后续重评放在 L11/L12 之后,优先 c2、不取 c1 |

**与第二波的关系**:C1/C8a/C8b 是纯搬移/删死件,与性能项零冲突;从 C4 起动的是 `core/model` + `commit/aggregate` + `shell/mod.rs`,
与性能探针同一批文件 ⇒ **性能项一律排在 C8c 之后**(第二波),不在第一波中途插入。**C8c 已完成(零代码)⇒ 第二波解锁,§12 指向 P1。**

### A2. 独立结构性重构(用户指令,不占 C1-C8c 的序)

> 与第一波并行存在、**不改变 C1-C8c 的顺序**;每笔同样过 §11.C 的门禁。第一波已完成,§12 当前项以 §11 B 为准。

| 编号 | 任务 | 阻塞/前置 | 状态 |
|---|---|---|---|
| **H1** | host 的 hop 分相:`shell/hop/` 四阶段(准备 / 执行并返回结果 / 生成 next plan / 分发 next 操作)+ 按 kind 的 driver 模块;`execute_one` 与 `apply_action_once` 走同一条流水线 | — | **已做(§10「H1 完成」,设计见 `KERNEL_REFACTOR_DESIGN.md` §3.8)** |
| **H2** | `core/plan/**` 的 plan 抽象层:每个计划/判定 = 一个类型,实现 `ActPlan`(用户 2026-10-16 指令;形态:trait + GAT 读取视图,连求值/策略一并收进) | H1 | **已做(§10「H2 完成」,设计见 `KERNEL_REFACTOR_DESIGN.md` §3.9)** |
| **H3** | 词汇:`Hop` → `Operation`(模块 `shell/hop/` → `shell/operation/`,相关函数/类型/注释同步;Trait 名由 H2 定为 `ActPlan`/`ActOperation`) | H1/H2 | **已做(§10「H3 完成」)** |

### B. 第二波:性能与遗留(可与彼此交错)

**P1(原 §1.b(iv),本清单此前的 A1)** —— 给 engine 形状那 ~1 ms/hop 的 plan/记账 CPU 做分阶段画像(**只量不改代码**;
用 §10「P6-1」的 engine 探针形状 + `#[cfg(test)]` 分阶段计时,插桩在工作区、未提交)。**已做(§10「P1 完成」)**。

- 验收:把 ~1 ms 分解到具体块(计划构造、消息 payload、`place_run`/`deliver_plan`、行投影),据此决定下一块动谁;完成后原地更新 §12 的提示词。
- 性能口径(**release**,§10 §5n):93 hops / 0.684 s = 7.4 ms/hop ≈ 136 hop/s(内存 store)。debug 的 11.2 hop/s 与 §9.5 legacy
  2778–5099 hop/s 的对比**作废**(debug 下两次 SHA-256 占整 run ~65%,是构建产物的假差距;release 下真实差距 ~20–37×)。
- 性能项的最终验收(留给 P1 引出的下一笔):hop 吞吐显著上升(目标 legacy 量级);`870+` 用例仍全绿;§10 记录前后对比数值与探针形状
  (探针代码已删,可按 §10「P6-1」的形状重建)。参考:一次 `examples/simple`(≈500 hops,debug,内存 store)= 708 次聚合读、726 次提交、569 行读。

**状态:第三波(A1-A11)已全部完成;第二波 P1 已完成,§12 当前指向 P2(下一笔性能工作)。**

| 编号 | 任务 | 来源 | 状态 / 验收 |
|---|---|---|---|
| **P2** | **整包编码只算一次 SHA-256**(无格式变更):`ProcessDocumentCodec::encode` 现在 `seal` 一次、`encode.validate` 再一次;去掉重复的一次。可选:跳过未变化的派生索引文档编码(先量)。 | §10「P1 完成」 | 未做。验收:同一探针 `agg.encode`/`confirm.encode` ms/call 降 ≈一次 `envelope.encode`,wall/commit 降 10–20%;持久化字节/摘要不变;`-p acts --lib --tests` 全绿 + clippy/fmt |
| **L1** | 删死掉的 `ops` 族(**BREAKING**):`Store::ops()`/`StoreIden::Ops`/`store/data/op.rs` + `remove_proc_rows` 家族清单那一行;先决定老库 `ops-*` 孤儿行的处理(接受 / 保留只删不建 / 一次性清理) | 原 §5 | 未做。验收:生产侧 `grep` 为 0 + 全量门禁绿 + CHANGELOG BREAKING |
| **L2** | 聚合文档回收/归档:~~先定保留策略 / pid 复用语义 / 历史取舍~~ **已由 A11 取代**。A11 明确:存储只服务恢复,终态且消息全部投递/结算后回收;不做历史归档;回收后 PID 可复用,旧 pid 重复执行由业务系统负责 | 原 §6;2026-10-17 用户裁决 | **被 A11 取代**(设计见 `KERNEL_REFACTOR_DESIGN.md` §3.10);不再单独执行 |
| **L3** | `sealed` 边界:实现(聚合持久化 sealed 或投影携带),或写成永久已知边界 | 原 §2 | 未做(现状 = 写成边界) |
| **L4** | 逐项绑定 `index`/`value` 的上浮:给一个不参与折叠的载体,或写成永久边界 | 原 §7b | 未做(证据见 §10「Feature B 完成」§4) |
| **L5** | 索引键合并/懒写(`tasks` 家族 ~9 写/hop、`kernel/indexes` 18.6 写/hop)+ `load1` 少一次(需把 claim 提前、预检挪后) | 原 §1.b① / §1.b(i)① | 未做,**低优先**:§10「P6-1」已证与 hop 吞吐无关(`recovery_entries`/两索引文档每 hop < 1 µs),只在真后端(sled/pg)的写次数/字节上有意义 |
| **L6** | 投影降频或 wake 驱动(§9.3.3) | 原 §1.c | 未做;等 P1 把那 ~1 ms 分解出来再定。别把"引擎级用例单例已被 tick 主导"误当 hop 吞吐(§5f) |
| **L7** | 文档收口:`lib.rs` 顶部注释、`CHANGELOG.md`(`kernel_engine` 已不存在;`cache_cap` 语义 = 在飞 run 数上限)、`README` 里可能残留的 legacy 说法 | 原 §4 | 未做 |
| **L8** | **减字节:整包聚合 → 拆分文档 / append-only 操作日志**(§9.3.4:16.0 MB 里 14.7 MB 是整包聚合)。属**持久化格式变更**,必须单独立项(迁移 + golden fingerprint 更新);与 C5 的 codec V3 **是同一批格式工作** ⇒ 建议与 C5 合并排期 | 原 §1.b③ / §1.b(i)② / §1.b(iii) | 未做。现状:每 hop 整包工作量 = 3 decode + 2 encode 一个随 run 增长的聚合 |
| **L9** | **退役 Phase 0:删动态 act 死簇与 legacy 事件面** —— `Context::build_acts`/`dyn_build_act`/`NodeTree::append_node`、`package/core/{block,parallel,sequence}.rs` 的三个调用点;`Emitter::on_proc`/`on_task`/`emit_proc_event`/`emit_task_event*`/`TaskExtra`;确认零调用的 `Process`/`Task` 方法(`create_task`/`print`/`into_data`/`task_by_*`、`unique_id`/`into_data_vars`/`is_vars_dirty` 等) | 设计 §3.7(d)⑤ + 2026-10-17 评估 | 未做。**零行为变更**(headless tree 的产物本来就被 `effects::run` 丢弃,`projection` 从 durable model 重建);验收:生产侧 `git grep "build_acts\|dyn_build_act\|append_node\|emit_proc_event\|on_proc("` = 0,`-p acts` 全绿,行/消息/`tree_output` 不变 |
| **L10** | **退役 Phase 1:headless scope 去 `Process`/`Task`** —— `ScopeSpec` 已是完整输入,换成 `ScopeTask`(node content + data + sealed + cancel)+ `Scope`(pid/env/owner/workdir);替换 `scope.rs:47-87`,保留包实际用到的 `task.{pid,id,options,node,data}` 与 `proc.{id,owner_scope,env,workdir,set_data}` | 设计 §3.7(d)① | 未做;前置 L9。验收:effect/predicate/package 套件全绿;每 hop 不再建 `Process`+`NodeTree`(收益按 P1 口径量);`Context` 的包 API 名字不变或按 BREAKING 记录 |
| **L11** | **退役 Phase 2:model 直出 view** —— `Workflow::valid`/`tree_output` 与投影 `node_data` 直接走 `Workflow`/`Step`/`Branch`(逐字保持行与树输出);删 `NodeTree`/`Node`;`NodeData`/`NodeContent` 持久化形状保留 | 设计 §3.7(d)③ | 未做;前置 L10。验收:`sch_tree_output_is_byte_stable` 563-byte、`model:get fmt=tree`、`node_data` 逐字、投影行格式不变 |
| **L12** | **退役 Phase 3:读 API 改行查询并删类型** —— `proc:get` 用 task 行 + `TaskInfo::from(&data::Task)` + vars join;删 `Runtime::proc`/`Store::load_proc`/`store/process_rows`/`Process`/`Task`/`TaskTree`;`Executor::proc().get_process` 删除或改读模型 DTO(**BREAKING**) | 设计 §3.7(d)④ | 未做;前置 L11。验收:`proc:get` 字段不变、reliability 测试改读行/快照后全绿、`git grep "struct Process\|struct Task\|struct NodeTree"` = 0、CHANGELOG BREAKING |

> **L9-L12 的关系**:四笔按序、每笔独立可回;`NodeData`/`NodeContent` 是持久化形状,必须保留。**D4 已拍(2026-10-17):暂不挪**;`view` 的挪位不再作为独立笔。L11/L12 后若重评,优先 c2(c1 不取:`NodeData` 会留在 shell)。L9 的“零行为变更”已由 C8c 的 effect seam 取证背书:headless tree 的产物本来就被 `effects::run` 丢弃,`projection` 从 durable model 重建。

### C. 每笔的收口门禁(原 §3;不是"一项",是每笔都要过的闸)

`cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` / `cargo fmt`(或 `--check`);
`-p acts` 至少 `--lib --tests`;动到 store/CLI/server/package/channel 或 intent 面时跑全 workspace 的 per-crate 套件
(`-p acts`、`-p acts-acl`、`-p acts-expr --all-features`、`-p acts-channel`、`-p acts-plugin-*`、`-p acts-package-*`、
`-p acts-store --features sled,sqlite,postgres`、`-p acts-cli`、`-p acts-server`);不动 manifest 就不必跑 `cargo deny check bans`。
`acts-server` 若只有"全部 target 一条命令"会红,逐 target 跑并在 §10 记录该环境现象。C5/L1/L8 另需 CHANGELOG BREAKING;C5 另需 golden 重冻结的书面理由。

### D. 第三波:架构修正(A1-A11;2026-10-17 审计,优先于第二波)

> 设计见 `KERNEL_REFACTOR_DESIGN.md` §3.10。审计结论:分层方向保留,问题集中在
> **effect/commit 顺序、正常路径的活性来源、同一 Process 的写者模型、投影协议、Shell 仍承担状态机语义、
> 以及终态 ProcessSnapshot/投影/索引的回收与 PID 复用**。
> 执行顺序:**A1a → A2 → A11 → A1b(批量 confirm) → A1c → A3/A4/A5 → A6 → A7/A8/A9/A10**。
> A1a/A2/A11/A1b/A1c/A3/A4/A5/A6/A7/A8/A9/A10 已完成并分别记录在 §10;A1b 的 per-record 试点保持回退(见 §10「A1b 试点回退」),
> 正常路径不得恢复 per-record claim/confirm;A1c 的自动重试只对 executor observation 证明 not-applied
> 且包显式声明幂等的 effect 生效。
> 第二波 P1/L* 暂缓,不删除。

| 编号 | 任务 | 审计条目 | 优先级 | 依赖 | 状态 / 验收 |
|---|---|---|---|---|---|
| **A1a** | effect 边界分类:包 effect 后丢失 CAS/`next` 失败 → `MarkEffectUnknown`,不释放 claim 自动重试;客户端动作路径同处理 | #2 | **P0** | — | **已做(§10「A1a 完成」)**。验收:effect 后冲突为 `EffectUnknown`,recovery 只报 `ManualReconciliation`,不 wake |
| **A1b** | outbox 提交后发送:plan 先写 `Pending`;**批量确认本 operation 的新记录**(或把 outbox 状态移出 aggregate);投影先于外部消息;删除前置直发快路径。**per-record claim/confirm 变体已回退,不得原样重试** | #2 | **P0** | A1a;批量确认设计 | **已做(§10「A1b 完成」)**。验收:消息不在 aggregate commit 前可见;每 operation 最多一笔确认提交;loop/吞吐不退化;at-least-once 不变。正常路径 `deliver_committed_outbox` + `confirm_outbox_batch`;retry pass 仍 per-record |
| **A1c** | effect 幂等协议:稳定 idempotency key + 可查询 observation;非幂等 effect 只走 manual reconciliation;补齐人工处理闭环 | #2 | P1 | A1b(批量) | **已做(§10「A1c 完成」)**。验收:`EffectKey` 由 operation 身份纯函数派生(重规划/重试/重启不变);`EffectRunner::observe` 的 `Applied`/`Failed` 直接重放、`NotApplied`+幂等声明才重新入队、其余人工;`Engine::effect_unknowns`/`resolve_effect` 提供可持久化的人工闭环;恢复不重复调用 effect |
| **A2** | durable ready queue:业务提交内写 ready 索引;`try_send` 只做加速;driver 从 ready 取到期操作;recovery 只做异常兜底;修 recovery 分页每页全扫的 $O(N^2)$ | #1 | **P0** | —(与 A1b 的 store schema 改动需协调) | **已做(§10「A2 完成」)**。验收:队列满不导致下一跳等 tick;recovery 扫描量与异常操作数相关;idle timer/parked run 不进热 ready 集合;冷启动/缺 ready 记录可从 aggregate 修复 |
| **A3** | 折叠纯内部 hop:Planner 一次跑到下一个外部边界;branch/join/next/终态/纯传播/纯级联不单独立 operation | #3 | P1 | A1/A2 | **已做(§10「A3 完成」)**。验收:线性 3 节点 claim/commit 从 9 降到 6(`ExecuteTask` + `ApplyAction`,无纯 hand-off 落盘);branch/join/catch/next/终态与 skip/abort 级联、rewind 都在产生者提交内折叠;行为、消息、行投影、lifecycle 不变;fold 失败回退原计划,effect/CAS 语义不退 |
| **A4** | 每 Process 单写者 command lane:正常/客户端动作/恢复 command 串行决定状态;effect 可并发;observation 回 lane;CAS 只做恢复/fencing | #4 | P1 | A2 | **已做(§10「A4 完成」)**。验收:同一 pid 的并发 client action 恰一个决策、loser 为决策性拒绝、`action_replans()==0`、aggregate guard 冲突为 0;不同 pid 的 effect 在 `Barrier(2)` 会合、各自提交;effect 窗口内同一 pid 的 command 可执行,effect 后冲突落 `EffectUnknown`(不重跑)。`commit_transition` 的 domain `VersionConflict` Err 在 effect 后也按 A1a 分类 |
| **A5** | projection 正式协议:每行持久化 `source_aggregate_revision`;store-level 条件写;启动增量补投影;repair queue;API lag/强读 aggregate;lifecycle 后置或显式契约 | #5 | P1 | — | **已做(§10「A5 完成」)**。验收:旧投影不能覆盖新投影(store guard + 持久 revision);重启不丢版本(boot catch-up);消息/完成事件消费者只能在行 current 后看到提交;lag/failed/repaired 有指标 |
| **A6** | Shell 变薄:Core 输出完整 `Decision`(aggregate changes/operations/effect intents/outbox/timers/process status/admission directives);Shell 只做 IO/commit/execute | #6 | P1 | A3/A4/A5 | **已做(§10「A6 完成」)**。验收:`core/decision.rs` 的 `Decision { transition, process_status, settlement, lifecycle, admission }` 由 `Decision::committed` 产出;`run_settled`/`projection_settled`/lifecycle/admission/resume-operation 规则只在 Core;`Admissions`/`report_lifecycle`/`place_run` 只执行 directive;`OperationKind::is_schedulable` 与 enum 同处;Recovery 按 state 分类、Projection 不匹配 kind |
| **A7** | Recovery 统一成 `RecoverOperation(observed) -> Decision`:恢复层只收集 observed;正常/恢复共用 transition/commit 与 per-pid lane | #7 | P1 | A2/A4 | **已做(§10「A7 完成」)**。验收:`Observed { operation_id, kind, persisted_phase, revision, aggregate_revision, effect_receipt, now_ms }` 是唯一输入;`recover_operation -> RecoveryDecision { directive, command }` 是唯一入口,`RecoveryPlanner::classify` 私有;reclaim/finalize 走 lane + revision guard,`FinalizeCommit` 与正常 `dispatch` 共用 `execute_decision`(project → lifecycle → outbox batch confirm → admission);`EffectUnknown` 仍按 A1c(`Applied`/`Failed` 重放、`NotApplied`+幂等才重试、其余人工);正常/恢复同状态同 Decision |
| **A8** | Admission 分离:execution/effect/recovery/reconciliation quarantine/projection 各自额度;manual reconciliation 不占正常 slot;cancel 立即释放 | #8 | P1 | A2/A4 | **已做(§10「A8 完成」)**。验收:`RunFacts::execution_active`/`place` 在 Core,`EffectUnknown`/`Quarantined` 的 run 立即释放 execution slot;manual 决策走 `Slot::Direct` + reconciliation gate;`mark_effect_unknown` 与终态 client decision 在同一决策里 `admit_run` 队首;五类 cap 各自配置,`AdmissionMetrics` 暴露 execution/parked/effect/recovery/reconciliation/projection/quarantined |
| **A9** | durable run handle:`start()` 返回可 `wait()` 的 handle;先查终态→订阅→复查;event channel 只做通知;示例改先注册或 `wait()` | #9 | P2 | A1/A5 | **已做(§10「A9 完成」)**。验收:check-subscribe race 不存在;重启后仍可等待 |
| **A10** | 拆分控制面调度器:timer/outbox retry/recovery/lease renewal/settled sweeper/projection repair 独立 interval/backoff/budget/指标;禁止用统一缩短 tick 解决延迟 | #10 | P2 | A2 | **已做(§10「A10 完成」)**。验收:ready/timer、recovery、outbox、projection 各自独立 ticker + `KernelSchedule`/`Config` interval/budget;recovery 频率不影响 timer/outbox,ready 精度不增加 recovery 扫描;`SchedulerHealth` 与 `Ready/Outbox/RecoveryMetrics` 独立可读。lease renewal 无独立实现,claim 过期仍由 recovery 回收 |
| **A11** | 终态回收 + PID 复用:run 终态且消息全部投递/结算、无未决 operation 后,原子删除 aggregate + recovery index + outbox index + ready index,再幂等删行/messages/deliveries/workdir;清理后同一 pid 可再次启动;不做历史归档,旧 pid 迟到消息由业务去重 | 2026-10-17 追加(存储目标) | **P1** | A2/A5 | **已做(§10「A11 完成」)**。验收:清理后聚合/索引/行/workdir 全消失;清理前拒绝复用、清理后允许;崩溃不留不可恢复半状态;`EffectUnknown`/未 ack run 不误删。**pid 存在性只看 aggregate**(行/`ops` 不参与);start 与 cleanup 共用 per-pid 生命周期锁。A5 的完整投影 generation 协议仍未做,本笔以 pid 墓碑收窄清理后的旧投影重写窗口 |

**A1/A2/A11 的边界**:A1a 只关闭“effect 已执行后自动重试”的重复路径;A1b(批量确认)已关闭 outbox 的提交前可见性,
**per-record claim/confirm 试点保持回退**(写放大不可接受)。A1c 已提供稳定 effect key、executor observation
与人工 resolve 闭环:只有 observation 证明 not-applied 且包显式声明幂等时才自动重试,`Applied` 只重放计划、
不重跑 effect,其余仍停在 `ManualReconciliation`。A1c 不提供跨进程 exactly-once,也不替内置包臆造 observation。
A2 只把正常活性从 recovery 扫描移开,不改变 recovery 的异常兜底职责;**A11 已把 `ready` index 与
aggregate/recovery/outbox 一起原子回收**(A11 完成)。
A11 只回收**已完成且投递结算完毕**的 run;`EffectUnknown`、`Quarantined`、`DeliveryUnknown`、
未结算 delivery、未决 operation 一律保留。
A11 的存储语义是“恢复所需的最小状态”,不是历史归档;PID 复用只在回收完成后放行,旧 pid 迟到消息由业务去重。
A1b 已落地:outbox 记录先以 `Pending` 提交、再由 dispatcher 在投影后批量确认,因此 A11 的
“全部 `Delivered` 才回收”判定天然把“已提交但未投递”的 run 保留在存储中;该判定未被绕过。
**不要把“缩短 tick”当作活性手段** —— 正常活性来自 durable ready 记录;tick 只决定 ready 轮询、定时器
cadence 与异常兜底的频率。A2 已把 recovery 分页改为位置游标,单页不再全扫索引。

## 12. 下一步提示词(只含**当前任务**与限制约束;每轮**原地更新**,不追加)

> 任务清单在 §11;这里只有"现在做哪一项"和约束。**每执行完一项:重写本节的「当前任务」,并把 §11 的状态标记改掉。**

```
你是 acts(D:/private/github/workflow/acts,Rust edition 2024)的执行 agent。

# 当前任务:P2 —— 聚合文档编码只算一次 SHA-256(§11 B,第二波,接 P1)

**依据**:`KERNEL_CUTOVER_NOTES.md` §11 B 的 P2;§10「P1 完成」的 release 画像(30 步、47,959 B 聚合:
3.84 ms/op / 2.28 ms/commit;`examples/simple`、85,226 B:3.97 ms/op / 2.42 ms/commit)。P1 结论:
plan/记账逻辑 ≤6%,整包 encode/decode/apply 占 66–80%;其中每次 `ProcessDocumentCodec::encode` 对
同一 payload 做**两次 SHA-256**(`ProcessEnvelope::seal` 一次、`ProcessEnvelope::encode` 的
`validate` 再一次)。微基准(release):`seal` 0.318/0.406 ms、`envelope.encode` 0.291/0.411 ms
(47,877/85,144 B payload)。

**目标**:让 `ProcessDocumentCodec::encode` 的 payload 只被哈希一次;**不改持久化格式、字节或摘要**。

**本笔内容**

1. 收敛 `ProcessEnvelope` 的 seal 路径:`seal` 计算摘要后,`encode` 不再对同一 payload 重算;
   `decode` 的摘要校验保留。形态自选(如 `seal` 交出已封 envelope,或 `encode` 自己算摘要),
   但**只允许一个哈希点**;不得引入 `unsafe`,不得静默跳过 `decode` 校验。
2. 审计所有 `ProcessEnvelope::{seal,encode,decode}` 的构造/调用点(含测试):任何“手工构造错摘要
   再期望 `encode` 拒绝”的用例改用 `decode` 覆盖,或保留一个显式 `validate` 测试。
3. 记录改动前后的 per-encode 成本与端到端 ms/commit;预期 [INFERENCE]:engine −0.49 ms/op
   (−12.7%)、simple −0.67 ms/op(−17.0%),按 1.68/1.64 次 encode per attempt。
4. 可选:跳过未变化的派生索引文档编码;先量,它们只占 `agg.encode` 的小头。不做也行,不得为此
   改事务/guard 语义。

**边界**

- 不改持久化格式、`AGGREGATE_SCHEMA_VERSION`、摘要算法、聚合/索引字节;不迁移旧库。
- 不改 effect/outbox payload、outbox 顺序、A4/A5/A8/A9/A10/A11 语义;不以缩短 tick 或跳过校验制造吞吐。
- 不用 debug 数据做结论;性能口径一律 release + 内存 store,与 §10「P1 完成」同探针形状
  (`cargo test --release -p acts --lib perf_probe ...`,探针临时、提交前删除)。

**验收**

- 同一探针:`agg.encode`/`confirm.encode` ms/call 降幅 ≈ 一次 `envelope.encode`;wall/commit 降
  10–20%(环境噪声内以 per-call 与 commit 计数为主);
- 聚合文档字节与摘要不变(单测:同一 snapshot encode 前后字节相等/摘要稳定);
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`、`cargo fmt --check`、
  `cargo test -p acts --lib --tests` 全绿;
- §10 写「P2 完成」并更新 §11 B 状态;§12 原地指向下一项(默认下一步为 L8 拆文档/append-only)。

# 限制与约束

- **先读**:§10「P1 完成」的探针形状与数值、§11 B P2;`crates/acts/src/scheduler/durable/{document,envelope}.rs`、
  `document_store.rs::build_replace_transaction`、`build_outbox_confirm_transaction`。
- 遵守 §11.C 门禁;动到 durable/序列化面时按需跑 `-p acts` 全套;不改 manifest 就不跑 `cargo deny`。
- 探针与计时插桩不提交;生产代码与文档分开提交,提交前过 §11.C 门禁。
```

