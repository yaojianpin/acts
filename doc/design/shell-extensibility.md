# scheduler/shell 扩展性设计

- 状态：方案（P1 待实施）
- 日期：2026-10-28
- 范围：`crates/acts/src/scheduler/shell`、`crates/acts/src/scheduler/runtime.rs`、`crates/acts/src/plugin`、`crates/acts/src/package`

## 1. 问题

`scheduler/shell` 是引擎的异步半部：加载聚合、执行效果/谓词、提交计划、投影、
投递、恢复和定时器。随着功能增加，它已经从“操作驱动”长成了“什么都放这里”的
目录：

| 模块 | 行数（含测试） | 说明 |
| --- | ---: | --- |
| `shell/mod.rs` | 2,519 | `KernelHost`、驱动循环、端口、提交/投递编排、`scope_spec` |
| `shell/operation/action.rs` | 3,548 | 最大的单个操作实现 |
| `shell/tests/mod.rs` | 10,653 | 测试 |
| `shell/model.rs` | 1,544 | 模型导入/容器物化 |
| `shell/projection.rs` | 1,439 | 读模型投影 |
| `shell/facts.rs` | 1,130 | 聚合事实读取 |
| `shell/scope.rs` | 905 | package-facing scope |
| `shell/operation/*` | ~6,900 | 其余操作 |
| `shell/assign/mod.rs` | 749 | 承办关系/授权 |
| `shell/context.rs` | 638 | package-facing context |
| 其余（recovery/message/planning/admission/delivery/lifecycle/timers/…） | ~4,500 | |

症状：

- 新功能默认往 shell 加模块，评审面越来越大，边界靠记忆维护。
- 插件作者无法替换可选能力，只能 fork 或等上游。
- 编译时间、测试时间和 `mod.rs` 的耦合一起增长。
- “确定性核心”与“I/O 编排”混在同一个目录，容易把 I/O 带进纯计划层。

## 2. shell 的职责

一次操作的生命周期：

1. 读取聚合/恢复索引，claim 操作（`KernelHost` + `recovery`）。
2. 预检、固定 durable intent（`operation/*`、`planning`）。
3. 在 shell 阶段执行外部工作：包效果（`effects`）、分支谓词（`predicate`）、
   承办人解析（`assign`）。
4. 计划提交（`core/plan` + 聚合仓库），投影读模型（`projection`）。
5. 投递 outbox（`delivery`）、发生命周期事件（`lifecycle`）、跑定时器
   （`timers`）。

其中 1、2、4 的状态机/提交/恢复是**正确性内核**；3、5 的外部数据和副作用是
**可替换能力**。shell 变大，主要是后者的默认实现和宿主胶水堆在了一起。

## 3. 原则

1. **确定性内核保持封闭。** 操作状态机、plan、提交/CAS、聚合、恢复索引、
   admission/quota、授权、消息投影不开放插件替换；它们是引擎的语义。
2. **可选能力一律走端口。** 外部数据、外部副作用、传输、定时触发、读模型目标
   都是端口；默认实现放在独立 crate/插件，shell 只编排。
3. **纯计划层不做 I/O。** 端口只在 shell 的异步阶段调用；`scheduler/core/plan`
   不感知端口。
4. **端口在 serve 前装配。** 端口是启动配置的一部分，启动后不动态替换；
   需要换实现时重启引擎。这样没有并发替换的语义。
5. **新功能的落点规则。**
   - 新业务数据/外部副作用 → package（`ActPackage`）或端口。
   - 新持久状态/新操作 → core（必须走设计评审）。
   - 新传输 → 插件。
   - 新报表/投影字段 → 投影端口或 package，不进 `KernelHost`。

## 4. 清单与分类

| 模块 | 分类 | 结论 |
| --- | --- | --- |
| `mod.rs`/`KernelHost` | 宿主 | 保留，但只保留端口装配与驱动 |
| `operation/*` | 内核 | 保留；效果调用点走端口 |
| `core/*` | 内核 | 保留 |
| `model.rs`/`facts.rs`/`planning.rs`/`graph.rs` | 内核 | 保留 |
| `read_model.rs`/`message.rs`/`projection.rs` | 内核读模型 | 保留；投影目标可端口化 |
| `recovery/`/`admission/`/`liveness/` | 内核 | 保留 |
| `scope.rs`/`context.rs` | 运行时 API | 稳定化后抽到 `acts-runtime-api`（P2） |
| `assign/mod.rs` | 内核语义 + 端口 | 保留语义；`AssignmentResolver` 端口化 |
| `effects.rs` | 端口 + 默认实现 | 默认 `PackageEffectRunner` 抽到 crate（P2） |
| `predicate.rs` | 端口 + 默认实现 | 默认 acts-expr 适配器抽到 crate（P3） |
| `delivery/mod.rs` | 端口 + 默认实现 | 投递跟随 `acts-channel`/传输插件（P3） |
| `lifecycle/mod.rs` | 端口 | sink 公开（P1） |
| `timers/*` | 内核 liveness + 可选触发 | retry/cleanup/purge 保留；schedule 触发端口化（P4） |
| `validation.rs` | 包的 schema | 包定义/端口负责（P3） |
| `snapshot.rs` | 内核策略 + 解析器 | 策略保留；解析器可端口化（P5） |

## 5. 现有端口

`KernelHost` 已经持有七个端口，方向是对的：

- `EffectRunner`（`effects.rs`）
- `PredicateRunner`（`predicate.rs`）
- `AssignmentResolver`（`assign/mod.rs`）
- `OutboxTransport`（`core::outbox::outbox_dispatch`）
- `LifecycleSink`（`lifecycle`）
- `RowProjector`（`projection`）
- `TimerGuider`（`timers`）

问题：

- 全部是 `pub(crate)`，外部 crate 无法实现。
- 端口参数里带着私有类型（`ScopeSpec`、`EffectRequest`、`ResolvedEffect`、
  `PackageEffectRunner`），即使 trait 公开也不可用。
- 默认实现在 `Runtime::create` 里硬编码，不能按配置选择。
- 没有统一的装配结构，新增端口要改 `KernelHost` 字段、`attach_*` 和
  `Runtime::create` 三处。

## 6. 目标架构

### 6.1 `KernelPorts`

```rust
pub struct KernelPorts {
    pub effects: Option<Arc<dyn EffectRunner>>,
    pub predicates: Option<Arc<dyn PredicateRunner>>,
    pub assignment: Option<Arc<dyn AssignmentResolver>>,
    pub delivery: Option<Box<dyn OutboxTransport>>,
    pub lifecycle: Option<LifecycleSink>,
    pub projection: Option<Arc<RowProjector>>,
    pub timers: Option<Arc<TimerGuider>>,
}
```

- `Runtime::create` 先装默认实现，再用 `EngineBuilder`/`ActPlugin::on_init`
  注册的 ports 覆盖；`None` 表示“用默认”。
- 端口 trait 移到 `acts::ext`（`pub`），参数只使用公开类型。
- 装配在第一次 serve 前完成；之后只读。

### 6.2 窄宿主

端口不拿 `Arc<Runtime>`。新增一个公开的宿主 trait：

```rust
#[async_trait]
pub trait ShellHost: Send + Sync {
    fn store(&self) -> Arc<Store>;
    fn config(&self) -> Arc<Config>;
    fn env(&self) -> Arc<Environment>;
    async fn package(&self, uses: &str) -> Result<CachedPackage>;
    fn snapshot(&self, name: &str) -> Option<Arc<SnapshotStore>>;
    fn scope_spec(&self, ...) -> Result<ScopeSpec>;
    fn workdir(&self) -> Option<PathBuf>;
}
```

这样 `PackageEffectRunner`、`PredicateRunner` 等实现可以搬出 `acts`，只依赖
`acts` 的公开类型。

### 6.3 包运行时 API

`Context`/`Scope`/`ScopeTask` 是 package 作者真正使用的表面。它们应成为
`acts-runtime-api`（或 `acts::runtime` 公开模块），而不是 `shell` 的私有结构。
这一步与端口公开一起做，避免“trait 公开了但参数类型不可命名”。

### 6.4 默认实现的位置

| 端口 | 默认实现 | 目标位置 |
| --- | --- | --- |
| effects | `PackageEffectRunner` | `acts-effect-package`（依赖 runtime-api） |
| predicates | acts-expr 适配器 | `acts-predicate-expr` |
| delivery | `MessageForwarder`/`ShadowTransport` | `acts-channel` 或传输插件 |
| lifecycle | emitter sink | 保留在 `acts`（很薄） |
| assignment | `RuntimeAssignmentResolver` | 保留在 `acts`（与 `assign` 语义同层） |
| timers | 内核 liveness | 保留；schedule 触发可独立 |
| projection | 行投影器 | 保留；目标端口在 P5 |

## 7. 迁移阶段

### P0：冻结 shell 增长（现在）

- 评审规则：新增可选能力必须是 package 或端口；禁止往 `shell/` 加业务模块。
- 本次 `acts.core.owner` 就是该规则的示例：IRQ 包，业务解析在客户端，shell
  只保留 `OwnerExpr` 的本地原子求值（无 I/O）。它用 `ActPackage::prepare_params`
  这个 package 级钩子在请求发出前替换本地 context 原子；钩子由效果运行器调用，
  不增加 shell 的业务逻辑。
- 验收：新功能的 diff 不增加 `shell/` 的公开/私有模块数。

### P1：公开端口 + `KernelPorts`（低风险）

- 新增 `acts::ext`，公开七个端口 trait 和它们的数据类型。
- `KernelHost` 用 `KernelPorts` 装配；`EngineBuilder::ports`/`ActPlugin::on_init`
  可以注册。
- 默认实现留在原处（仍可用），只是可以通过端口覆盖。
- 风险：公开 API 的 semver；参数类型必须稳定。
- 验收：一个外部测试 crate 实现其中一个端口并在引擎里生效；现有测试不变。

### P2：抽出效果运行器 + 运行时 API（中风险）

- 把 `Context`/`Scope`/`ScopeTask` 稳定成 `acts-runtime-api`。
- 把 `PackageEffectRunner` 移到 `acts-effect-package`；`effects.rs` 只剩 trait
  和宿主胶水。
- 风险：循环依赖（`acts` ↔ `acts-effect-package`）；用宿主 trait 断开。
- 验收：默认引擎行为不变；`acts-effect-package` 的测试覆盖包执行、sealed、
  超时/取消。

### P3：谓词、投递、包 schema（中风险）

- 谓词默认实现移到 `acts-predicate-expr`。
- 投递跟随 `acts-channel`/传输插件；`delivery/mod.rs` 只保留端口调用。
- 包 schema 由包定义/校验端口负责，`validation.rs` 缩到缓存和错误映射。
- 验收：分支/投递/包校验的全量测试通过。

### P4：定时器注册（低-中风险）

- retry/cleanup/purge 保持内核 liveness；schedule 触发变成一个可选端口/插件。
- 验收：schedule 测试、恢复测试、关闭测试通过。

### P5：投影目标端口（中风险）

- `RowProjector` 的默认实现保留；允许插件注册额外的投影目标。
- 读模型正确性（source revision、CAS、修复）仍是内核。
- 验收：读模型一致性测试、投影修复测试通过。

## 8. 复杂度与风险

| 阶段 | 规模 | 风险 | 回滚 |
| --- | --- | --- | --- |
| P0 | 评审规则 | 低 | 无代码 |
| P1 | ~400 行公开 API + 装配 | 中：semver/隐私 | 默认实现仍可用，端口可忽略 |
| P2 | ~600 行移动 + 宿主 trait | 中高：循环依赖 | 保留旧实现，双路径一个版本 |
| P3 | ~500 行移动 | 中：传输行为 | 传输插件可回退 |
| P4 | ~200 行 | 低中：定时器时序 | 定时器默认不变 |
| P5 | ~300 行 | 中：投影一致性 | 默认投影不变 |

其他风险：

- **性能**：端口是 `Arc<dyn Trait>` 动态分发，效果路径本来就有一次；需要
  bench 确认没有热路径回归。
- **安全**：端口实现运行在同样的 ACL/scope 下；不能通过端口绕过授权或快照
  密封。宿主 trait 不暴露写聚合的入口。
- **测试**：每个端口提供 fake；现有 `FlowFixture` 继续覆盖默认实现。
- **兼容**：端口公开是新增 API；默认行为不变。移除 `crate-private` 路径前保留
  一个版本的 deprecated 别名。

## 9. 不抽离的部分

以下能力留在 `acts` 内核，不开放替换：

- 操作状态机、plan、提交/CAS、聚合文档、恢复索引。
- admission/quota/并发、ready 队列、claim/fence。
- 授权与 per-task assignment 的判定（`__assignment`/`__delegation` 是授权源）。
- 消息投影、生命周期事件的语义。
- 读模型的一致性/修复（`source_revision`、CAS、投影修复）。

这些是“引擎是什么”的定义；开放它们会让不同部署的行为不可比较，也会把正确性
责任推给插件。

## 10. 结论

shell 不应该继续按“新功能 = 新模块”增长。把可选能力收敛到公开端口，默认实现
放到独立 crate/插件，shell 只保留确定性内核和编排。本次参与人改造已经按这个
方向落地：`acts.core.owner` 是 IRQ 包，业务数据在业务系统，不在 shell，也不在
插件配置里。

下一步是 P1：公开七个端口和 `KernelPorts`，用默认实现保持行为不变；随后按 P2
抽出效果运行器和运行时 API。
