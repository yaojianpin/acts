# 启动就绪与初始化事务实施计划

- 日期：2026-11-18
- 设计依据：`doc/design/startup.md`
- 输入：审计《第一卷 A》RUN-START-01…07（逐条核对，两处改判见设计 §1）
- 范围：`crates/acts/src/scheduler/{runtime.rs,shell/}`、`engine.rs`、`builder.rs`

## 阶段与任务

### P0 — 装配顺序与启动屏障（止血）

| # | 任务 | 位置 |
| --- | --- | --- |
| 0.1 | `Runtime::create` 重排：构造 `Arc<Runtime>` → attach 全部端口/timers → spawn driver → spawn cleanup（消除“spawn 先于 attach”的竞态） | `scheduler/runtime.rs:599-775` |
| 0.2 | 新增 `StartupGate`（watch 单次门、幂等 open、取消安全的 wait） | `scheduler/shell/startup.rs`（新） |
| 0.3 | `Workers` 持有 `startup`；`new_with_admissions(..., start_gated)`；`open_startup`/`startup_gate`；`serve` 在起 lane 与 boot repair **之前** `select!` 等门与 shutdown | `scheduler/shell/mod.rs:422-470`、`:1119-1131` |
| 0.4 | cleanup task 同样等门（`#[cfg(not(test))]` 路径） | `scheduler/runtime.rs:740-770` |
| 0.5 | `Runtime::new_gated`（builder 用）、`Runtime::open_startup`；`Engine::initialize` 末尾开门的唯一位置 | `scheduler/runtime.rs:94-97,232`、`engine.rs:242-262`、`builder.rs:322` |
| 0.6 | 测试：held 时 run 不结算、open 后结算；held 时 close 不驱动；屏障原语单测 | `scheduler/runtime.rs` tests、`scheduler/shell/startup.rs` tests |
| 0.7 | 文档：`design/startup.md`；`design/caps.md` §2.2 补一句“gate 与 attach 顺序”；CHANGELOG Unreleased | `doc/design/*`、`CHANGELOG.md` |

验收：

```sh
cargo test -p acts --lib
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo fmt --check
```

P0 的边界：不改变 `Runtime::new`/`Engine` 的对外语义；不改任何能力类型；
不改变 `ShadowTransport` 的默认地位（其生产路径替换仍在 `create` 内完成）。

### P1 — 能力缺失的类型化（消除成功型降级，已完成）

落地时的两处调整（与初稿不同，理由见 `design/startup.md` §7）：

- `CapabilitySlot` **未引入**：`DisabledByPolicy` 没有合法构造路径，两态与
  `Option` 同构。缺失语义由类型化的 `StepEffect` + 构造器默认值承担。
- `PackageEffectRunner::run` 的"定义存在但实例缺失"仍走 `Effect::Failed`：
  P0 屏障下只有真正损坏的部署会到达，且 start 的 `resolve` 已先失败；升级为
  可重试错误需要给公开 `ActError` 加变体，归入 P2。

| # | 任务 | 结果 |
| --- | --- | --- |
| 1.1 | `StepEffect` 三态（`NoEffect`/`Ready`/`NotReady`），先读声明再看槽位；`ApplyAction` 只在 `NoEffect` 时 `Completed(None)`，`NotReady` 返回 `Outcome::Nothing`（释放 claim、下轮重试，不进 `catches`） | 完成 |
| 1.2 | `CapServices.delivery` 改为 `Option`，`ShadowTransport` 移入 `#[cfg(test)]`；kernel 测试宿主在 `install_test_caps` 显式安装 shadow + `TestEffectRunner`；未装配的投递在领取前返回、记录保持 `Pending` | 完成 |
| 1.3 | `EffectRunner::resolve` 返回 `Result<Option<ResolvedEffect>>`（BREAKING）；`Func` 包缺实例在 start 失败，`Irq`/`Msg` 的"只发布定义"部署保留声明 params | 完成 |
| 1.4 | 打开屏障前校验 effect runner 已装配 | 完成 |
| 1.5 | push 规划无 runner 时 fail closed（不再猜 `Irq`）；恢复的 `NotReady` 不产生 observation；等待中的 act 只在其包 `executes_while_waiting` 时才运行 effect（裸 `ApplyAction` 仍是空完成） | 完成 |
| 1.6 | `receipt="shadow"` 历史审计 | 移到 P2（outbox 无 receipt 索引，启动路径不做全扫） |

验收（已过，见进度）：声明包而 Effect 缺失时操作被推迟而非完成；普通无包 step
仍 `Completed`；测试宿主的空完成必须来自显式安装的替身；投影/投递在未装配时
不静默成功。

### P2 — 定义内容寻址、绑定与审计（已完成；deployment 记录于 11-19 删除）

落地时的三处调整：

- **定义按 id 存储**（现状），因此"同 id 不同版本并存"没有实现；改为
  同版本不同内容**拒绝**（`ActError::Conflict`），新版本替换行。运行中的
  operation 靠定义绑定（2.4）拒绝执行被替换的新内容，而不是读旧版本。
- **`Operation` 记录绑定**没有改聚合 schema：绑定写在 start 已有的
  `#def` 私有事实里（与 `#params`/`#options` 同一次提交），执行时按它校验
  存储定义的哈希。旧 run 无绑定，允许通过（升级兼容）。
- **deployment 行整体删除（2026-11-19，用户决定）**：`Preparing/Active`
  清单不是互斥（互斥是单库的 `DbLease`，且在 engine 之前 acquire），写入在
  包注册之后、CAS 在插件 start 之后（做不了早退），内容无读者。多分片时
  `lease = true` 被 server 拒绝（lease 是全局行，跨库 guard 无法原子），
  分片部署没有内建单写者保护，需外部协调；一个 server 拥有整个 shard set，
  不是"每 shard 一个 engine"。分片级 fence 需要 per-shard lease，属独立特性。
  设计见 `design/startup.md` §8.4。

| # | 任务 | 结果 |
| --- | --- | --- |
| 2.1 | `data::Package.content_hash`；`Store::publish` 内容寻址：相同内容幂等、同版本不同内容 `Conflict`、新版本替换；旧行哈希按存储内容回填 | 完成 |
| 2.2 | registry `generation`/`freeze` | 删除（清单移除后无消费者） |
| 2.3 | `deployments/active` + `DeploymentManifest` + `Preparing → Active` | 删除（见上） |
| 2.4 | `DefinitionBinding` 进 `EffectRequest`；start 写 `#def`；`PackageEffectRunner` 在 resolve/run 前校验哈希，不匹配报错（明确要求重部署） | 完成 |
| 2.5 | `Preparing` 续跑/诊断 | 随 2.3 删除 |
| 2.6 | （原 1.6）shadow receipt 审计：`Engine::shadow_deliveries()` 按读模型列出 `receipt="shadow"` 的 run 与记录键（只读，不自动重发） | 完成 |

验收（已过）：相同定义重发不产生写；同版本不同内容报冲突；绑定不匹配的
effect 在 start 即失败；审计能列出测试 transport 的确认记录；启动不再写任何
deployment 行。

### P3 — 启动事务与关闭所有权（已完成）

| # | 任务 | 结果 |
| --- | --- | --- |
| 3.1 | `ActPlugin::{prepare,start,shutdown}`（async，默认空实现）；engine 先 prepare 全部，再统一 start；listener 只在 start 内 bind | 完成 |
| 3.2 | Engine 持有插件列表与 started 计数；失败/关闭按逆序 `shutdown().await`；未达 Ready 的关闭输出 `startup rollback report`（prepared/started/stopped/errors） | 完成 |
| 3.3 | Runtime 持有 driver/cleanup JoinHandle、Workers 持有 lane JoinHandle；`close` = cancel → join driver → join lanes → join cleanup → writer drain → flush → emitter close；幂等 | 完成 |
| 3.4 | NATS 在 `prepare` 连接 broker，失败使 `start()` 返回 Err（不再 spawn 后静默禁用） | 完成 |
| 3.5 | 启动阶段结构化日志：`phase=building/prepared/preparing/ready/quiescing/closed`，含 generation 与 manifest_hash | 完成（tracing 字段，不引入指标后端） |

验收：prepare 失败零监听；第 N 个 start 失败时前 N-1 个 shutdown 且逆序；
Builder 返回 Err 后端口可重绑、无后台 task；插件任务在 close 内 join。

## 统一门禁（每阶段交付前）

```sh
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo fmt
cargo deny --locked check
cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0459 --ignore RUSTSEC-2025-0057 --ignore RUSTSEC-2024-0384
cargo test -p acts --lib
cargo test -p acts --doc
```

P0 不涉及依赖。P2/P3 的 manifest 变更只有 `plugins/acts-plugin-nats` 的
`[dev-dependencies] toml = { workspace = true }`（`toml` 已在依赖图中，只多一条
开发依赖边，`Cargo.lock` +1 行）；`deny`/`audit` 的结论不变，未在本地重跑，
以 CI 为准（AGENTS 的提交门禁仍需在提交前执行）。

## 进度

- [x] P0（2026-11-18：`Runtime::create` 装配后 spawn；`StartupGate` +
      `Runtime::new_gated` + `Engine::initialize` 开门；cleanup 同门；
      `acts` lib 1823 例全绿，新增三例：
      `a_held_startup_gate_keeps_the_driver_off_the_store`、
      `closing_a_held_runtime_stops_before_the_gate_opens`、
      `a_plain_runtime_starts_with_an_open_barrier`）
- [x] P1（2026-11-18：`StepEffect` 三态 + 推迟；投递槽位显式缺失、shadow
      进 `cfg(test)`；`resolve` 返回 `Option` 且 `Func` 缺实例在 start 失败；
      push 无 runner fail closed；等待中的 act 只在 `executes_while_waiting`
      时跑 effect。`acts` lib 1823 例全绿；1.6 移到 P2）
- [x] P2（2026-11-18：内容哈希 + publish 幂等/冲突；`#def` 定义绑定；
      shadow receipt 审计。11-19：deployment 清单/generation 按决定删除，
      单库互斥由 `DbLease` 承担；`acts` lib 全绿）
- [x] P3（2026-11-18：插件 prepare/start/shutdown；逆序关闭与 rollback
      report；driver/lane/cleanup join；NATS prepare 连接；阶段日志。
      `acts` lib、插件套件全绿）
