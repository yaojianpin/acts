# 启动就绪与初始化事务（2026-11-18）

- 状态：P0（屏障 + 装配顺序）、P1（能力缺失类型化）、P2（内容寻址 + 定义
  绑定 + shadow 审计；deployment 记录已按决定删除，见 §8.4）、P3（插件生命周期
  + 关闭所有权）已落地；计划文档保留验收与偏差记录
- 范围：`crates/acts/src/scheduler/{runtime.rs,shell/}`、
  `crates/acts/src/engine.rs`、`crates/acts/src/builder.rs`
- 输入：审计《第一卷 A》RUN-START-01…07 的逐条核对结果（含改判）

本文只描述**已实现或已决**的启动协议。审计提出的类型化能力槽位、Plugin
Prepare/Start/Shutdown、Deployment Manifest、close 顺序等，作为后续阶段
写在计划里，本文不把它们写成现状。

## 1. 已核实事实（2026-11-18，file:line）

> 行号是核对时（改造前）的版本：本次改动把 `shell/mod.rs` 与 `runtime.rs`
> 的行号后移了十几到二十几行；符号名不变。

| 事实 | 位置 |
| --- | --- |
| `Runtime::create` 先 `tokio::spawn(driver.serve)`，再构造 `Arc<Runtime>` 并 attach 端口；spawn 与首个 attach 之间**没有** `.await` | `scheduler/runtime.rs:652`、`:674-748` |
| 端口的装配默认值：`effects: None`、`delivery: ShadowTransport`、`lifecycle: None`、`read: None`、`assignment: None` | `scheduler/shell/mod.rs:435-446` |
| `Workers::serve` 没有就绪屏障：先启动 lane worker，再 boot repair，再进 ready/recovery/outbox/projection 循环 | `scheduler/shell/mod.rs:1119-1131` |
| Effect 槽位为空时 `step_effect` 返回 `Ok(None)`；`ApplyAction` 把它当成 `Effect::Completed(None)` | `scheduler/shell/mod.rs:2577-2579`、`scheduler/shell/operation/action.rs:2308` |
| 包定义存在但运行时实例未注册时，`PackageEffectRunner::run` 的 `package().create` 返回 `Err`，走 `Effect::Failed`（即 task 失败/`catches`） | `scheduler/shell/caps/effect.rs:252-256`、`operation/action.rs:2296-2300` |
| 恢复的幂等判定读注册表：`idempotent()` 在实例缺席时回答 `false`，planner 据此把 `NotApplied` 变成人工对账而不是重试 | `caps/effect.rs:289-296`、`scheduler/core/recovery/planner.rs:257-258` |
| `run_as()` 在实例缺席时回答 `None`；push 出的 act 按 `Irq` 默认规划 | `caps/effect.rs:310-316`、`operation/action.rs:866-875` |
| `resolve()` 在“定义已发布、实例未注册”时**静默**使用声明 params，跳过 `prepare_params` | `caps/effect.rs:210-221` |
| 初始化顺序：snapshot 注册 → 逐个 `plugin.on_init` → 发布插件包定义 → 内置包 → 显式包定义 → 显式实例注册 | `crates/acts/src/engine.rs:264-301` |
| 插件在 `on_init` 内 bind/spawn；NATS 的 `on_init` 在连接失败时仍返回 `Ok`（只记 “nats plugin disabled”） | `plugins/acts-plugin-web/src/lib.rs:34-46`、`plugins/acts-plugin-nats/src/lib.rs:383-423` |
| 包定义 `publish` 以 id 为唯一键 upsert，无 version/hash/revision 维度 | `crates/acts/src/store/store.rs:165-189` |
| `Runtime::close` 只 cancel + writer drain + flush + emitter close，不 Join driver/lane/cleanup；lane 只在取下一个 job 时观察取消 | `scheduler/runtime.rs:232-252`、`scheduler/shell/mod.rs:1428-1448` |
| Builder 初始化失败时确实调用 `runtime.close().await` | `crates/acts/src/builder.rs:326-333` |

两条对审计的改判（决定了本文的优先级）：

1. 01/02/03 的窗口是 `create()` 里“spawn 之后、attach 之前”的**同步尾部**，
   不是整个 `Engine::initialize`：四个端口都在 `create()` 返回前挂载完毕，
   期间没有 await。竞态仍可达（多线程 executor 可抢占构造线程），但业务
   层面的宽窗口只有一条——Package Registry 在 `initialize` 里才完成（04）。
2. 因此 P0 的收益不在“缩短微秒窗口”，而在**把 Package Registry 的宽窗口
   关掉**：驱动在 `initialize` 提交前不碰 store。

## 2. 决策

### D1 装配先于驱动

`Runtime::create` 的顺序改为：构造 `Arc<Runtime>` → attach 全部端口与 timers
→ spawn driver → spawn cleanup。`attach_*` 的文档承诺（“attach before driving
operations”）由此成为字面事实，不再依赖“窗口很短”。

### D2 屏障归 host，默认打开

`Workers` 持有 `startup: StartupGate`（`scheduler/shell/startup.rs`）：

```rust
Workers::new_with_admissions(..., start_gated: bool)
Workers::open_startup(&self)          // 单向、幂等
Workers::startup_gate(&self) -> StartupGate
```

- `start_gated == false`（`Workers::new` / `in_memory` / 全部既有测试）→ 屏障
  构造即开，`serve` 立即通过，行为与改造前一致。
- `start_gated == true` 只有 `Runtime::new_gated`（builder 的启动事务）使用。
- `Runtime::new` 保持公开语义：构造完成即可驱动，不改变嵌入方用法。

### D3 屏障挡的是“驱动”，不是 start 自身的投影

`serve` 在**启动 lane worker 与 boot repair 之前**等待屏障；cleanup task 同样
等待。`Runtime::start` / `do_action` 的第一次投影是调用方事务的一部分，不经
屏障——它们写入的是调用方明确要写的记录。

### D4 失败启动不打开屏障，靠取消退出

`close()` **不**打开屏障：如果先 open 再 cancel，`select!` 的两个分支同时
就绪，driver 有概率先跑一轮 `start_scheduler_workers` + `ensure_boot`，在
close 之后读 store。正确做法是 `serve` 用

```rust
tokio::select! {
    _ = self.startup.wait() => {}
    _ = shutdown.cancelled() => return,
}
```

让取消成为唯一的 abort 路径：失败启动后 driver 一行 store 代码都不执行。

### D5 打开点 = `Engine::initialize` 末尾

注册提交（snapshot / plugin 包 / 内置包 / 显式包 + 实例）全部成功后
`self.runtime.open_startup()`。Ready 是单向的：没有回到 Initializing 的路径；
热重载将来用新的装配快照切换，而不是重关屏障。

### D6 本阶段不做的

类型化能力槽位（`CapabilitySlot`）、Package Registry freeze、Plugin
Prepare/Start/Shutdown、Deployment Manifest、close 的 Join 顺序。理由与顺序
见计划 P1–P3：它们各自需要改类型或改存储格式，不能借 P0 一并夹带。

> **P1 修订（2026-11-18）**：`CapabilitySlot` **没有**引入。三态里的
> `DisabledByPolicy` 没有合法构造路径——effect 缺失必须 fail closed，
> delivery "禁用"只会让 outbox 永远 `Pending`、run 永远不可回收——而两态
> 槽位与 `Option` 同构，只是多一层命名。缺失的语义改由**类型化的
> `StepEffect`**（§7）和构造器默认值承担：`Capabilities::delivery` 从
> "默认 shadow"改为 `None`，`ShadowTransport` 移入 `#[cfg(test)]`，
> kernel 测试宿主在 `install_test_caps` 里显式安装替身。

## 3. 状态机（当前实现）

```text
Created
  └─ Runtime::create
       Building     端口 attach、timers attach（屏障 held）
       └─ spawn driver（阻塞在 StartupGate，不触达 store）
            Preparing     Engine::initialize：ACL load → snapshot →
                          plugin prepare（注册包）→ package definitions →
                          instances
                 ├─ Starting  全部插件 start（listener 此时才 bind）
                 │     ├─ Ok:  open_startup() → Ready → Serving（claim/recovery/outbox）
                 │     └─ Err: Engine::close() → 逆序 shutdown → cancel →
                 │             driver 从屏障 select 退出
                 └─ Err: Engine::close() 同上
```

`Building → Ready` 是构造线程上的顺序代码；屏障是唯一允许 driver 越过
`Preparing`/`Starting` 的信号。

## 4. 不变量与验收

Ready 之前：

- 零次 ready claim、零次 recovery 扫描、零次 outbox 投递、零次 cleanup；
- 零次 `start_scheduler_workers`（lane 任务尚未存在）；
- store 只被 `initialize` 自己的注册写入（以及插件在 `on_init` 内主动发起的
  调用），不被驱动读取。

Ready 之后：`Runtime::start`/`do_action` 的语义不变，`sys:status` 的 caps 列表
不变。

失败启动：`close()` 返回后 driver 已退出（或从未越过屏障），不产生 store 访问。

验收测试（`scheduler/runtime.rs` tests）：

1. `a_held_startup_gate_keeps_the_driver_off_the_store`：held runtime + 一个
   无包 step 的 run，等待 200 ms 断言未结算（seed 只写根操作，outbox 记录由
   执行产生，所以"没有操作执行"就是"没有投递"）；`open_startup` 后在超时
   内结算。
2. `closing_a_held_runtime_stops_before_the_gate_opens`：held runtime + 同一
   run，`close()` 在超时内返回，run 仍未结算。
3. `startup::tests`：屏障原语本身（held 不通过、open 释放、open 幂等）。

## 5. 残留风险与取舍（P2/P3 之后）

| 风险 | 现状 | 阶段 |
| --- | --- | --- |
| 插件 listener 在 Ready 前可接外部流量 | 插件在 `start` 才 bind，但那发生在开屏障之前；请求写入的 ready work 仍不会在屏障前被驱动 | 设计取舍（P3） |
| `idempotent()` 读注册表 | 屏障保证恢复只在注册完成后运行；恢复判定仍读当前注册表 | 已接受（P2） |
| 包定义按 id 存储，无版本并存 | 同版本不同内容拒绝；新版本替换，运行中的 act 靠 `#def` 绑定拒绝新内容 | 已接受（P2） |
| `close()` 的 join 会等待在途 effect 完成 | 这是正确性要求（提交先于 flush），代价是关闭可能等待一次外部调用 | 已接受（P3） |
| `receipt="shadow"` 审计依赖读模型 | 已回收的 run 无聚合可查，审计只覆盖仍可读的 run | 已接受（P2.6） |
| 没有指标后端 | 阶段信息是 tracing 字段；`sys:status` 未增加 phase | 已接受（P3.5） |

## 6. 兼容性

- 公开 API 无变化：`Runtime::new`、`Engine::builder().start()` 的对外语义不变。
- 新增 `Runtime::new_gated` / `Runtime::open_startup` / `Workers::*startup*` 均为
  `pub(crate)`。
- `Workers::new_with_admissions` 增加 `start_gated` 参数（crate 内构造器）。
- **插件语义**：`on_init` 内 `runtime.start(...)` 仍是合法的 durable 写入，但
  该 run 不会在 `Engine::initialize` 返回前被驱动（屏障未开）。在 `on_init`
  内“启动并等待完成”的插件会等不到；正确写法是在 listener/task 内启动，
  或等 `engine.start()` 返回。树内三个插件（web/gRPC/NATS）都只 bind/spawn，
  不受影响。

## 7. 能力缺失的类型化（P1，2026-11-18）

P0 用屏障保证"驱动不会在装配完成前运行"。P1 处理另一半：即使缺能力，
内核也不能把缺失解释成成功。

### 7.1 `StepEffect`：一次解析，三种事实

```rust
pub(in crate::scheduler::shell) enum StepEffect {
    NoEffect,                                    // 声明里没有包
    Ready { runner, request: Box<EffectRequest> }, // 有包，宿主能执行
    NotReady { package: String },                // 有包，宿主没有 runner
}
```

- **先读声明，再看槽位**：没有 `uses` 的 step 永远是 `NoEffect`，与 runner
  是否装配无关。这样"没装 runner"不可能被读成"这个 act 本来就没活"。
- `NoEffect` → `Effect::Completed(None)`（唯一允许的空完成）。
- `NotReady` → `ApplyAction::run` 返回 `Outcome::Nothing`：不提交、不完成、
  不走 `Effect::Failed`/`catches`；`execute_one` 释放 claim 并推迟下一次
  ready 轮（`release_claim(&claimed, 1)`）。生产宿主在驱动启动前已装配
  runner，所以这是装配错误，不是业务状态；每次推迟记一条 `warn`。
- `Ready` → 原有执行路径，不变。

### 7.2 `resolve`：不再用声明 params 冒充已准备

`EffectRunner::resolve` 返回 `Result<Option<ResolvedEffect>>`：

- `PackageEffectRunner`：恒为 `Some`。定义存在但**没有运行时实例**时，
  `Func` 包返回错误（引擎要执行它，缺实例是部署错误，act 在 start 失败）；
  `Irq`/`Msg` 包沿用"只发布定义、由客户端执行"的部署，携带声明 params。
- `TestEffectRunner`（`#[cfg(test)]`）：`None`。测试宿主没有包注册表，
  不产生 facts——与 P0 之前"无 runner 时不 pin facts"的字节一致。

### 7.3 投递：未装配 ≠ 已投递

- `CapServices.delivery` 从"默认 `ShadowTransport`"改为
  `Option<Arc<dyn OutboxTransport>>`；`ShadowTransport` 变为
  `#[cfg(test)]`，生产构建里不可构造。
- 未装配时，`deliver_committed_outbox` 直接返回，`dispatch_outbox_record`
  在领取前返回 `Ok(false)`：记录保持 `Pending`、不产生 claim/重试计数。
- `Workers::new` / `new_with_workers`（kernel 测试宿主）在
  `install_test_caps` 里显式安装 shadow 与 `TestEffectRunner`；测试要别的
  行为就自己 attach（last wins）。`Runtime::create` 无条件装配
  `MessageForwarder` 与 `PackageEffectRunner`。

### 7.4 其余成功型降级

- push 规划不再用 `None` 猜 `Irq`：`Workers::effect_run_as` 在无 runner 时
  返回错误（fail closed）；runner 认识包但未注册仍回答 `None`（导入期的
  `Irq` 默认）。
- 恢复侧 `step_effect` 为 `NotReady` 时不产生 observation，操作保持
  `EffectUnknown`（不自动重跑）。
- `Engine::initialize` 打开屏障前检查 effect runner 已装配。

### 7.5 已知残余

- `PackageEffectRunner::run` 在 `Func` 实例缺失时仍走 `Effect::Failed`
  （task 失败）。P0 的屏障让它只在真正损坏的部署上出现，且 start 的
  `resolve` 已先失败；把它升级成可重试的基础设施错误需要给公开
  `ActError` 增加变体，留到后续。
- `receipt="shadow"` 的历史数据审计已在 P2 落地为
  `Engine::shadow_deliveries()`（§8.3）：只读、按读模型列出，不自动重发。

### 7.6 兼容性（破坏性）

- `acts::caps::EffectRunner::resolve` 的返回类型由 `ResolvedEffect` 变为
  `Option<ResolvedEffect>`；树内实现与 `tests/runtime_api.rs` 已同步。
  这是 0.x 的内部扩展缝，CHANGELOG 记为 BREAKING。

## 8. 定义内容寻址、绑定与审计（P2，2026-11-18；deployment 记录于 11-19 删除）

### 8.1 定义内容哈希

`data::Package` 增加 `content_hash`（serde default，旧行缺失即空）。
`Store::publish` 按内容寻址：

- 相同 id + 相同内容 → 幂等 no-op（每次启动都会重发内置/插件定义，不能当成更新）；
- 相同 id + 相同版本 + 不同内容 → `ActError::Conflict`（同一版本不能有两个实现）；
- 相同 id + 新版本 → 替换行并写入新哈希；
- 旧行（哈希为空）按存储内容先算一次再比较，相同则回填哈希、不动 update_time。

### 8.2 定义绑定（`#def`）

Start 解析包 facts 时把 `DefinitionBinding{id, version, hash}` 写进
`#def`（与 `#params`/`#options` 同一提交）。`step_effect` 把它带进
`EffectRequest.binding`；`PackageEffectRunner` 在 `resolve` 与 `run` 之前
比较存储定义的 `content_hash`：

- 一致（或双方为空）→ 继续；
- 不一致 → `ActError::Package("... changed since the act started ...")`，
  start/effect 明确失败，要求重新部署；不会静默执行新内容。

旧 run 没有 `#def`（升级前启动）→ 无绑定，允许通过。

### 8.3 Shadow receipt 审计

`Engine::shadow_deliveries()`（`Runtime::shadow_deliveries`）按 `procs` 读模型
逐 run 加载聚合，列出 `Delivered { receipt: "shadow" }` 的记录键。只读、
不自动重发；已回收的 run 没有聚合可查，不在结果中。`shadow_receipts(snapshot)`
是该过滤器的单元测试入口。

### 8.4 为什么没有 deployment 记录（2026-11-19 删除）

初版实现过 `deployments/active`：`Preparing` 在插件启动前写入、`Active` 在
插件启动后 CAS，内容含 definitions/snapshots/capabilities/manifest_hash/
generation。核对后按用户决定整体删除，理由：

- **它不是多实例互斥**。互斥是 [`DbLease`]：**单物理库**同一时刻只有一个活
  实例，`acts-server` 在 engine 之前 `acquire`，第二个实例直接 `LeaseHeld`
  退出；运行期 `FencedStore` 用 fence 拒绝旧持有者的每一次写，过期后允许
  TTL 接管。
- **多分片根本没有 lease**。`lease = true` + 多分片被 server 拒绝
  （`acts-server/src/lib.rs:714`）：lease 是一条全局行、作为每次写的 guard，
  而跨物理库的 batch 没有共享事务。分片部署只能 `lease = false`，内建的单写者
  保护不存在，由运维保证；要内建需要**分片感知的 lease**（每 shard 一行、
  一起 acquire/renew/release、逐 shard fence），不是一行 startup 状态能替代的。
  分片也不是"每实例一个物理库"：路由在 Engine 内，一个 server 拥有整个
  shard set。
- **位置太晚，做不了早退**。`Preparing` 在包注册之后才写、`Active` CAS 在插件
  `start` 之后；两个并发启动可以都绑定端口，输的一个才在激活时失败。
- **内容没有读者**。definitions/snapshots/capabilities/manifest_hash 只被激活时
  的自比较使用；generation 只被下一次启动 `+1`；没有任何 API/状态面读取它。
- **"读状态→退出"本身是竞态**：需要原子 claim + owner + TTL + 过期接管，
  这正是 `DbLease` 已有的形状；再加一个 startup 集合等于两套 TTL、两处崩溃
  恢复、两个真相源。

因此启动提交点回到最简单的形态：注册与插件 start 全部成功后，
`Runtime::open_startup()` 开门。单写者约束由 lease 提供（单库），
分片部署则由外部协调提供——engine 不假装自己有。

[`DbLease`]: ../../crates/acts/src/store/lease.rs

## 9. 插件生命周期与关闭所有权（P3，2026-11-18）

### 9.1 三阶段插件

```rust
#[async_trait]
pub trait ActPlugin: Send + Sync {
    async fn prepare(&self, engine: &Engine) -> Result<()> { Ok(()) } // 校验/注册，不监听
    async fn start(&self, engine: &Engine) -> Result<()> { Ok(()) }   // bind + spawn
    async fn shutdown(&self) -> Result<()> { Ok(()) }                 // 逆序、幂等
}
```

`Engine::initialize` 的顺序：snapshot 注册 → 全部 `prepare` → 包注册 →
全部 `start`（listener 此时才 bind）→ 开屏障。任何 `prepare` 失败发生在监听
之前；任何 `start` 失败由 builder 的错误路径 `Engine::close()` 逆序关闭。

树内插件：web/gRPC 的 bind 移到 `start`，任务句柄存入 `tokio::sync::Mutex`，
`shutdown` join；NATS 在 `prepare` 连接 broker（失败即 start 失败），`start`
注册 channel 并 spawn，`shutdown` 有界 join（5s）后丢弃 client。

### 9.2 关闭所有权

`Engine::close`（幂等）：

```text
cancel shutdown token
plugins (reverse) shutdown().await
runtime.close():
    join driver → join lanes → join cleanup → writer.close → kv.flush → emitter.close
```

`Runtime` 持有 driver/cleanup 的 `JoinHandle`，`Workers` 持有 lane 的
`JoinHandle`。lane 只在两个 job 之间观察取消，因此在途 operation 会先完成
自己的提交；flush 之后不会再有写者。

### 9.3 回滚报告与阶段日志

未达 Ready 的 `close` 输出
`startup rollback report {prepared, started, stopped, errors}`；阶段日志为
`phase=building|prepared|starting|ready|quiescing|closed`。没有引入指标后端：
仓库的观测面是 tracing 字段（`scheduler_health` 仍按既有字段报告循环健康）。
