# scheduler/shell 扩展性实施计划

- 设计文档：[`doc/design/shell-extensibility.md`](../design/shell-extensibility.md)
- 状态：P0 生效；P1 待执行；P2 能力契约已定，participant 原型暂停
- 执行方式：一次只执行一个阶段；上一阶段验收通过后，才生成并执行下一阶段。

## 1. 阶段总览

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P0 | 冻结 shell 增长（评审规则 + 参与人包走 IRQ） | 生效中 |
| P1 | 类型化端口 + `PortSet` + `KernelPorts` 装配 | 待执行 |
| P2 | 能力类型/属性映射/提供者 key + 发布/启动检查；participant 首个外部能力 | 契约已定，原型暂停 |
| P3 | 抽出效果运行器 + `acts-runtime-api` | 待执行 |
| P4 | 谓词、投递、包 schema 默认实现外移 | 待执行 |
| P5 | 定时器注册（schedule 触发端口化） | 待执行 |
| P6 | 投影目标端口 | 待执行 |
| P7 | 插件自定义原语信封 | 待设计 |

## 2. 执行规则

1. 确定性内核不开放替换：操作状态机、plan、提交/CAS、聚合、恢复索引、
   admission/quota、授权、消息投影、读模型一致性。
2. 端口只在 shell 的异步阶段调用，不进 `scheduler/core/plan`。
3. 端口在 serve 前装配；启动 pass 后不动态替换。
4. 默认实现保持不变，端口是覆盖层；每个阶段都要有“默认行为不变”的回归。
5. 新功能落点：
   - 业务数据/外部副作用 → package 或端口。
   - 新持久状态/新操作 → 内核（设计评审）。
   - 新传输 → 插件。
   - 新报表/投影字段 → 投影端口/package。
6. 能力模型：
   - 属性（`assign.owner`）归 Model。
   - 能力类型（`CapabilityKind::Participant` + trait）归 Kernel。
   - 提供者 key（`acme.participant`）由插件定义，不在 `acts` 做插件名常量。
   - 内核常量只用于内部默认实现。
7. 不使用统一的 `dyn Resolver`/`dyn Any` 执行面；每种能力类型一个 trait，
   由 `KernelHost` 按类型化 `PortSet` 组装和调度。
8. 同一能力类型有多个提供者时，模型必须显式选择 `provider`，或该类型只有
   唯一提供者；解析后的 `(kind, key, version)` 固化到 process spec，恢复时
   缺失就失败，不静默换实现。

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

## 4. P1：类型化端口 + `KernelPorts`

### 4.1 目标

把现有端口从 `Runtime::create` 的硬编码 `attach_*` 收敛到类型化装配结构，
公开外部可实现的 trait；默认实现仍可用，行为不变。

### 4.2 范围

- 新增 `acts::capability`（或 `acts::ports`）：
  - `EffectRunner`、`PredicateRunner`、`ParticipantResolver`、
    `OutboxTransport`、`LifecycleSink`、`RowProjector`、`TimerGuider`。
  - 端口请求/响应类型，只暴露端口需要的公开字段。
  - `PortSet<T>`：`BTreeMap<String, Arc<T>>` + `default`，不存 `dyn Any`。
  - `KernelPorts`：每种能力类型一个 `PortSet` 字段。
- `KernelHost` 只保留 `ports` 和 getter；`Runtime::create` 集中装配默认实现。
- `EngineBuilder`/`ActPlugin::on_init` 注册端口；插件使用自己的 key。
- 默认实现仍在原处，行为不变。

### 4.3 验收

- 一个外部测试 crate（或 `crates/acts/tests` 的公开 API 用例）实现一个端口，
  在引擎启动时注册并生效。
- 默认引擎全量测试通过；`KernelHost` 不再逐个 `attach_*`。
- `cargo clippy ... -D warnings`。

## 5. P2：能力注册 + 属性映射 + 发布/启动检查

### 5.1 目标

模型属性通过静态映射找到能力类型，再由提供者 key 找到实现；缺失能力在发布
或启动时明确报错。participant 是第一个外部能力。

### 5.2 范围

- `CapabilityKind`：`Participant` 等类型枚举；每个 kind 对应一个 trait。
- `AttributeMap`：`assign.owner` → `CapabilityKind::Participant`。
- `CapabilityProvider { kind, key, plugin, version }`；能力表只用于检查和提示。
- 模型：
  - `Workflow::required_capabilities()` 返回 `CapabilityRequirement`；
  - 可选 `provider` 字段选择具体实现；未指定时使用唯一提供者，否则报错；
  - 发布时解析并固化 `ResolvedCapability` 到 process spec。
- 插件：
  - `ActPlugin::on_init` 注册 `(kind, key)` 和类型化端口；
  - 同一 `(kind, key)` 重复注册报错；启动 pass 后封口。
- 检查：
  - `model:deploy` 缺能力 → `ActError::Model`，点名属性和 key；
  - 活动启动时按固化 key 取端口，缺失 → fail closed。
- `assign.owner`：核心模型保持 `OwnerExpr`；默认插件 provider 仅处理本地原子，
  业务原子仍走 `acts.core.owner` IRQ 包。
- 环境扩展：`ActUserVar` / `ActFunction`，供插件实现解析规则。

### 5.3 验收

- 无插件发布含 `assign.owner` 的模型失败，错误点名 `assign.owner` 和安装
  参与人插件。
- 插件注册 provider key 后发布成功；解析器生效；单 owner/授权回归不变。
- 启动后注册能力或端口失败；启动时解析器保持生效。
- 多提供者未显式选择时发布失败。
- `cargo clippy ... -D warnings`。

## 6. P3：效果运行器 + 运行时 API

### 6.1 目标

把 `PackageEffectRunner`（`shell/effects.rs`）和 package 作者真正使用的
`Context`/`Scope`/`ScopeTask` 表面移出 shell，形成 `acts-runtime-api` 与
`acts-effect-package`。

### 6.2 范围

- `acts-runtime-api`（或 `acts::runtime` 公开模块）：
  - `Context`、`Scope`、`ScopeTask`、`ScopeSpec` 的稳定读取面。
  - `Context::workdir`、`effect_key`、`cancellation_token`、
    `resolve_owners`、`initiator`/`current_actor`/`previous_approver`。
- `ShellHost` 窄宿主 trait（`store`/`config`/`env`/`package`/`snapshot`/
  `scope_spec`/`workdir`），端口不拿 `Arc<Runtime>`。
- `PackageEffectRunner` 移到 `acts-effect-package`；`effects.rs` 只剩 trait
  和默认装配。
- `ActPackage::prepare_params` 的调用留在效果运行器（默认 no-op）。

### 6.3 验收

- 默认引擎的包执行、sealed、超时/取消、`prepare_params` 行为不变。
- `acts-effect-package` 有自己的单元测试；`acts` 不再依赖它的实现。
- 无循环依赖；`cargo deny check bans` 通过。

## 7. P4：谓词、投递、包 schema

### 7.1 范围

- 谓词默认实现移到 `acts-predicate-expr`；`predicate.rs` 只剩端口。
- `MessageForwarder`/`ShadowTransport` 跟随 `acts-channel` 或传输插件；
  `delivery/mod.rs` 只剩端口调用。
- 包 schema 校验由包定义/校验端口负责；`validation.rs` 缩到缓存和错误映射。

### 7.2 验收

- 分支谓词、投递重试/ack、包校验的现有测试全绿。
- 默认引擎不依赖具体谓词/投递实现即可启动（端口缺失时用内核默认）。

## 8. P5：定时器

### 8.1 范围

- retry/cleanup/purge 保持内核 liveness。
- schedule 触发变成可选端口/插件；`TimerGuider` 负责装配。
- 定时器的 due 计算/去重/重放仍在内核。

### 8.2 验收

- schedule 测试、恢复测试、关闭/取消测试通过。
- 没有 schedule 插件时，其他定时器不受影响。

## 9. P6：投影目标

### 9.1 范围

- `RowProjector` 默认实现保留；允许注册额外投影目标。
- `source_revision`/CAS/修复仍是内核。
- 投影目标的失败不阻塞提交，进入修复队列。

### 9.2 验收

- 读模型一致性/投影修复测试通过。
- 额外投影失败可观测、可重试，不改变内核状态。

## 10. P7：插件自定义原语信封（待设计）

- 模型支持 `{ capability, payload }` 信封；内核校验信封/能力存在，插件解析
  payload 并实现 `AttributeHandler`。
- 只对插件新增原语开放；核心原语保持类型化。
- 验收：插件定义新原语并通过发布/启动/恢复测试；核心模型不变。

## 11. 风险与回滚

| 风险 | 处理 |
| --- | --- |
| 公开 API semver | P1 先公开最小面；用默认实现过渡一个版本 |
| 循环依赖 | P3 用 `ShellHost` 窄宿主断开；`acts-effect-package` 只依赖 runtime-api |
| 性能 | 端口 `Arc<dyn>` 动态分发；对效果路径做 bench，无回归才合并 |
| 安全 | 端口不能绕过 ACL/快照密封；`KernelHost` 不暴露写聚合入口 |
| 测试矩阵 | 每个端口提供 fake；默认实现继续走 `FlowFixture` |
| 行为漂移 | 每个阶段先加“默认行为不变”的回归，再移动实现 |
| 提供者选择 | 多提供者必须显式或用唯一默认；解析结果固化，缺失不静默回退 |
| key 归属 | 内核只定义能力类型/属性映射；插件 key 不进内核常量 |

## 12. 完成定义

- shell 只保留确定性内核、端口装配和默认实现的最小胶水。
- 新增可选能力的默认落点是 package/端口，不是 shell 模块。
- 插件通过类型化 trait + 自定义 provider key 扩展；缺失能力在发布/启动明确报错。
- 默认引擎行为、测试、性能与当前一致。
