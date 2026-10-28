# scheduler/shell 扩展性实施计划

- 设计文档：[`doc/design/shell-extensibility.md`](../design/shell-extensibility.md)
- 状态：P0 生效，P1 待执行
- 执行方式：一次只执行一个阶段；上一阶段验收通过后，才生成并执行下一阶段。

## 1. 阶段总览

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P0 | 冻结 shell 增长（评审规则 + 参与人包走 IRQ） | 生效中 |
| P1 | 公开端口 + `KernelPorts` 装配 | 待执行 |
| P2 | 抽出效果运行器 + `acts-runtime-api` | 待执行 |
| P3 | 谓词、投递、包 schema 默认实现外移 | 待执行 |
| P4 | 定时器注册（schedule 触发端口化） | 待执行 |
| P5 | 投影目标端口 | 待执行 |

## 2. 执行规则

1. 确定性内核不开放替换：操作状态机、plan、提交/CAS、聚合、恢复索引、
   admission/quota、授权、消息投影、读模型一致性。
2. 端口只在 shell 的异步阶段调用，不进 `scheduler/core/plan`。
3. 端口在 serve 前装配；启动后不动态替换。
4. 默认实现保持不变，端口是覆盖层；每个阶段都要有“默认行为不变”的回归。
5. 新功能落点：
   - 业务数据/外部副作用 -> package 或端口。
   - 新持久状态/新操作 -> 内核（设计评审）。
   - 新传输 -> 插件。
   - 新报表/投影字段 -> 投影端口/package。

## 3. P0：冻结 shell 增长（生效中）

### 3.1 规则

- 新增可选能力不得新增 `scheduler/shell` 模块；优先 package/端口。
- `acts.core.owner` 是示例：IRQ 包，业务解析在客户端；shell 只保留
  `OwnerExpr` 的本地原子求值（无 I/O）。
- `ActPackage::prepare_params` 是 package 级钩子，不把 owner 逻辑写进 shell。

### 3.2 验收

- 新功能 diff 不增加 shell 的模块数；评审记录该判断。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`
  通过。

## 4. P1：公开端口 + `KernelPorts`

### 4.1 目标

把现有七个 `pub(crate)` 端口变成外部可实现的公开 API，并用一个装配结构
替代 `Runtime::create` 里的硬编码 `attach_*`。

### 4.2 范围

- 新增 `acts::ext`（或 `acts::runtime::ext`）：
  - `EffectRunner`、`PredicateRunner`、`AssignmentResolver`（或
    `ParticipantResolver`）、`OutboxTransport`、`LifecycleSink`、
    `RowProjector`、`TimerGuider`。
  - 端口数据：`EffectRequest`、`ResolvedEffect`、`EffectOutcome`、
    `ScopeSpec` 等，只暴露端口需要的字段。
- `KernelPorts`：
  ```rust
  pub struct KernelPorts {
      pub effects: Option<Arc<dyn EffectRunner>>,
      ...
  }
  ```
- `EngineBuilder::ports(KernelPorts)` 或 `Engine::register_*`；`ActPlugin::on_init`
  可以注册。
- `KernelHost` 只保留一个 `ports` 字段和 getter；`Runtime::create` 先装默认。
- 默认实现仍在原处，行为不变。

### 4.3 主要落点

- `crates/acts/src/scheduler/shell/mod.rs`
- `crates/acts/src/scheduler/runtime.rs`
- `crates/acts/src/builder.rs` / `engine.rs`
- `crates/acts/src/plugin/mod.rs`（文档/示例）
- 端口定义所在模块的可见性调整。

### 4.4 验收

- 一个外部测试 crate（或 `crates/acts/tests` 的公开 API 用例）实现一个端口，
  在引擎启动时注册并生效。
- 默认引擎全量测试通过；`KernelHost` 不再在 `Runtime::create` 里逐个
  `attach_*`（装配集中到 `KernelPorts`）。
- `cargo clippy ... -D warnings`。

## 5. P2：效果运行器 + 运行时 API

### 5.1 目标

把 `PackageEffectRunner`（`shell/effects.rs`）和 package 作者真正使用的
`Context`/`Scope`/`ScopeTask` 表面移出 shell，形成 `acts-runtime-api` 与
`acts-effect-package`。

### 5.2 范围

- `acts-runtime-api`（或 `acts::runtime` 公开模块）：
  - `Context`、`Scope`、`ScopeTask`、`ScopeSpec` 的稳定读取面。
  - `Context::workdir`、`effect_key`、`cancellation_token`、
    `resolve_owners`、`initiator`/`current_actor`/`previous_approver`。
- `ShellHost` 窄宿主 trait（`store`/`config`/`env`/`package`/`snapshot`/
  `scope_spec`/`workdir`），端口不拿 `Arc<Runtime>`。
- `PackageEffectRunner` 移到 `acts-effect-package`；`effects.rs` 只剩 trait
  和默认装配。
- `ActPackage::prepare_params` 的调用留在效果运行器（默认 no-op）。

### 5.3 验收

- 默认引擎的包执行、sealed、超时/取消、`prepare_params` 行为不变。
- `acts-effect-package` 有自己的单元测试；`acts` 不再依赖它的实现。
- 无循环依赖；`cargo deny check bans` 通过。

## 6. P3：谓词、投递、包 schema

### 6.1 范围

- 谓词默认实现移到 `acts-predicate-expr`；`predicate.rs` 只剩端口。
- `MessageForwarder`/`ShadowTransport` 跟随 `acts-channel` 或传输插件；
  `delivery/mod.rs` 只剩端口调用。
- 包 schema 校验由包定义/校验端口负责；`validation.rs` 缩到缓存和错误映射。

### 6.2 验收

- 分支谓词、投递重试/ack、包校验的现有测试全绿。
- 默认引擎不依赖具体谓词/投递实现即可启动（端口缺失时用内核默认）。

## 7. P4：定时器

### 7.1 范围

- retry/cleanup/purge 保持内核 liveness。
- schedule 触发变成可选端口/插件；`TimerGuider` 负责装配。
- 定时器的 due 计算/去重/重放仍在内核。

### 7.2 验收

- schedule 测试、恢复测试、关闭/取消测试通过。
- 没有 schedule 插件时，其他定时器不受影响。

## 8. P5：投影目标

### 8.1 范围

- `RowProjector` 默认实现保留；允许注册额外投影目标。
- `source_revision`/CAS/修复仍是内核。
- 投影目标的失败不阻塞提交，进入修复队列。

### 8.2 验收

- 读模型一致性/投影修复测试通过。
- 额外投影失败可观测、可重试，不改变内核状态。

## 9. 风险与回滚

| 风险 | 处理 |
| --- | --- |
| 公开 API semver | P1 先公开最小面；用 `#[doc(hidden)]`/默认实现过渡一个版本 |
| 循环依赖 | P2 用 `ShellHost` 窄宿主断开；`acts-effect-package` 只依赖 runtime-api |
| 性能 | 端口 `Arc<dyn>` 动态分发；对效果路径做 bench，无回归才合并 |
| 安全 | 端口不能绕过 ACL/快照密封；宿主 trait 不暴露写聚合入口 |
| 测试矩阵 | 每个端口提供 fake；默认实现继续走 `FlowFixture` |
| 行为漂移 | 每个阶段先加“默认行为不变”的回归，再移动实现 |

## 10. 完成定义

- shell 只保留确定性内核、端口装配和默认实现的最小胶水。
- 新增可选能力的默认落点是 package/端口，不是 shell 模块。
- 默认引擎行为、测试、性能与当前一致。
