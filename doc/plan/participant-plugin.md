# 参与人插件实施计划（assignment + owner）

- 设计文档：
  - [`doc/design/participant-plugin.md`](../design/participant-plugin.md)
  - [`doc/design/owner-expr.md`](../design/owner-expr.md)
  - [`doc/design/shell-extensibility.md`](../design/shell-extensibility.md)
- 状态：P3 已落地（能力类型 + provider key + 解析器端口），P4 已执行（服务端测试/文档/端到端）
- 执行方式：一次只执行一个阶段；上一阶段验收通过后，才生成并执行下一阶段。

## 1. 阶段总览

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P1 | `OwnerExpr` + 本地原子 + 上下文变量 + `assign` 集成 | 已落地（owner/assign 测试通过） |
| P2 | `acts.core.owner` IRQ 包迁入参与人插件 + `prepare_params` | 已落地（插件测试/内核/服务端全绿） |
| P3 | `ParticipantResolver` 端口 + 能力类型/provider key + 环境变量/方法扩展 | 已落地 |
| P4 | `acts-server` 注册 + 文档 + 端到端验证 | 已执行 |
| P5 | 参与人命令/审计端口（transfer/delegate 的扩展点） | 待设计 |

P1 先在内核落地，因为 `OwnerExpr` 是工作流模型的一部分；P2/P3 把参与人的
扩展部分收进插件，同时不改授权。

## 2. 执行规则

1. 每个阶段只改该阶段范围；P2 不再改 `assign` 的授权/状态语义。
2. 每个阶段结束跑仓库 gate：
   ```sh
   cargo fmt
   cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
   ```
   以及 `.github/workflows/test.yml` 对应的 crate 测试（至少
   `cargo test -p acts --lib`、`cargo test -p acts-plugin-participant --lib`、
   `cargo test -p acts-server --lib`）。
3. 阶段完成条件：设计文档的验收项都有证据；未通过不得进入下一阶段。
4. 不允许通过把业务目录写进核心/插件配置、或把授权下沉到插件来“简化”。
5. 业务数据只通过 `acts.core.owner` IRQ 应答进入工作流。

## 3. P1：OwnerExpr 与 assign（已落地）

### 3.1 目标

`assign.owner` 接受 `OwnerExpr`，解析本地原子；业务原子由
`acts.core.owner` 的 IRQ 请求交给业务系统。

### 3.2 已落地范围

- `crates/acts/src/model/owner.rs`：
  `OwnerExpr`、`ContextVar`、`Duty`、`ResourceRef`、`Relation`、`Participant`、
  serde（字符串旧形式 + `kind` 对象形式）。
- `crates/acts/src/model/step.rs`：`Assign.owner: OwnerExpr`、
  `with_assign_expr`。
- `crates/acts/src/scheduler/shell/owner.rs`：本地原子求值
  （user/context/用户变量/集合运算）；业务原子报错并点名
  `acts.core.owner`。
- `crates/acts/src/scheduler/shell/context.rs`：
  `initiator`/`current_actor`/`previous_approver(n)`。
- `crates/acts/src/scheduler/shell/scope.rs`：决策链走前驱图（顶层步骤的父节点
  是流程根）。
- `crates/acts/src/scheduler/shell/assign/mod.rs`：`assign` 通过
  `Context::resolve_owners` 解析，单 owner 校验。
- `crates/acts/src/env/functions.rs`：`$initiator`/`$actor`/
  `$previous_approver(n)`。

### 3.3 验收

- 字符串 owner 行为不变。
- 结构化 owner 的本地原子可解析；多结果/空结果 fail closed。
- 业务原子在 `assign` 中失败并点名 `acts.core.owner`。
- `previous_approver` 跨两个顶层步骤正确。
- 端到端：`acts.core.owner` 的输出驱动 `parallel` fan-out。
- 已执行：
  - `cargo test -p acts --lib`：1715 passed。
  - `cargo test -p acts --lib sch_act_assign`：10 passed。
  - `cargo test -p acts --lib owner`：38 passed（含 owner 包/模型/assign）。
  - `cargo test -p acts --lib pack_owner`：2 passed。
  - `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`：
    通过。
  - `cargo fmt --check`：通过。

### 3.4 遗留

- `parallel` 每项的 `assign.owner: "${{ value }}"` 在容器内联步骤上的可见性
  仍需用显式 `Step` id 的用例固定（现有 package 测试用 `Step`，参与人测试应
  同样构造）。
- 最终 gate：`cargo test -p acts --lib` 全量。

## 4. P2：`acts.core.owner` 迁入参与人插件（已落地）

### 4.1 目标

把 `acts.core.owner` 从内核 `package/core` 移到
`plugins/acts-plugin-participant`，让参与人功能有独立 crate；内核只保留
`OwnerExpr` 模型、`assign` 的本地解析和授权。

### 4.2 范围

- 新建 `plugins/acts-plugin-participant`：
  - `Cargo.toml`（依赖 `acts`、`async-trait`、`serde`、`serde_json`）。
  - `src/lib.rs`：导出 `OwnerPackage`、`ParticipantPlugin`。
  - `src/package.rs`：IRQ 包定义、`prepare_params`（本地 context 原子替换）。
  - `src/plugin.rs`：`ActPlugin`；`on_init` 通过 `Engine::add_package` 注册
    `OwnerPackage`（直接写 runtime 注册表，引擎在 `start` 中发布定义）；P3 在
    同一钩子注册解析器。
  - `src/tests.rs`：包契约、context 准备、IRQ 应答、fan-out；用
    `acts::testing::FlowFixture`（`test-utils`）复用内核的测试夹具。
- 删除 `crates/acts/src/package/core/owner.rs` 和 `package/core/mod.rs` 的
  `mod owner;`；从内核 `package/tests` 移除 owner 测试。
- 根 `Cargo.toml`/`Cargo.lock` 增加 workspace 依赖；`Engine` 增加同步的
  `add_package` 启动钩子（直接写 runtime 注册表，`start` 时发布定义）。
- `acts` 新增 `test-utils` feature，导出 `acts::testing::FlowFixture`，让
  包/插件测试复用同一套夹具（不再各自复制消息收集与等待逻辑）。
- `crates/acts-server` 增加依赖，`engine_builder` 只安装 `ParticipantPlugin`；
  包由插件的 `on_init` 注册。

### 4.3 验收

- `cargo test -p acts-plugin-participant --lib` 全绿。
- 包定义是 `ActRunAs::Irq`，schema 接受文档中的表达式。
- `prepare_params` 把 `initiator` 替换为 `user`；业务原子原样保留。
- 客户端应答 `{participants, ids, count}` 后 `parallel` 能展开。

### 4.4 验证

```sh
cargo fmt --check
cargo test -p acts-plugin-participant --lib
cargo test -p acts --lib
cargo test -p acts-server --lib
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
rg -n "acts.core.owner" crates plugins doc CHANGELOG.md
```

已执行（2026-10-28）：

- `cargo fmt --check`：通过。
- `cargo test -p acts-plugin-participant --lib`：5 passed（包定义 2；运行时 3：
  文档表达式/IRQ 请求、context 准备、客户端应答 + fan-out）。
- `cargo test -p acts --lib`：1711 passed。
- `cargo test -p acts-server --lib`：21 passed。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`：
  通过。
- `rg`：`crates/acts/src/package/core` 已无 owner 包/测试；
  `plugins/acts-plugin-participant` 有 `OwnerPackage` + `ParticipantPlugin`；
  `acts-server` 只安装插件（包由 `on_init` 注册）。剩余引用是文档、错误提示
  和模型注释。

验收项：

- 包定义仍是 `ActRunAs::Irq`，id/schema/描述与迁移前一致。
- `prepare_params` 仍把 `initiator` 换成 `user`；业务原子与
  `{ owner: … }` 包装按原语义处理。
- 客户端返回 `participants`/`ids`/`count` 后，`parallel` 展开两个 IRQ 子活动，
  子活动 params 的 `value` 分别为 u1/u2。
- 插件测试用 `acts::testing::FlowFixture`（`test-utils`）复用公开 API 夹具。
- P2 验收时 P3 尚未开始；解析端口随后在 P3 落地（见 §5）。

## 5. P3：`ParticipantResolver` 端口

### 5.1 目标

`assign.owner` 保留为模型原语；它的解析由插件实现。引擎定义能力类型
`CapabilityKind::Participant` 和类型化 `ParticipantResolver` 端口；插件用自己的
provider key 注册实现。没有提供者时，发布模型和启动活动都明确失败。

### 5.2 范围

- `crates/acts/src/participant.rs`：
  ```rust
  pub trait ParticipantResolver: Send + Sync {
      fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>>;
  }
  ```
  同步、无 I/O；业务原子仍由默认实现报错并点名 `acts.core.owner`。
- 能力模型：
  - `CapabilityKind::Participant`（内核能力类型）；
  - `assign.owner -> Participant` 的属性映射；
  - 插件 provider key（如 `acts-plugin-participant`）由插件定义，不进内核常量；
  - `Workflow::required_capabilities` 返回能力需求；发布时选择提供者并固化
    `(kind, key, version)` 到 process spec。
- `Context::resolve_local_owners`：确定性本地求值原语（包装现有
  `shell::owner::evaluate`，不复制逻辑）；`Context::resolve_owners` 按固化 key
  取类型化端口，无能力/无实现时 fail closed。
- `Runtime`：类型化 `PortSet<dyn ParticipantResolver>` 槽位；启动 pass 封口。
- `Workflow::has_assignment` / `Step::has_assignment`；`model:deploy` 缺能力时
  返回 `ActError::Model`，点名属性和 provider。
- 插件新增 `src/resolver.rs` 的 `LocalParticipantResolver`，委托
  `Context::resolve_local_owners`；`ParticipantPlugin::new` 使用它，
  `with_resolver` 替换，`on_init` 注册包、能力和解析器。
- 环境扩展：`ActFunction`（表达式方法）与既有 `ActUserVar`（变量）；
  `Environment` 保存函数表，`functions::resolve` 注入被引用的方法；
  `Engine::register_env_var` / `register_env_function` 供插件 `on_init` 使用，
  `ExtExecutor` 提供 ACL 检查的运行时入口。
- `acts::testing::FlowFixture` 的测试引擎通过启动钩子注册内部 provider key 和
  测试解析器；bare engine 用于验证发布拒绝。
- 测试：provider key 注册、替换解析器驱动 `assign.owner`、授权仍按
  `__assignment`、启动后注册被拒绝、多参与者仍按单 owner 规则失败、无提供者
  发布失败、插件函数可被表达式调用。

### 5.3 验收

- `assign.owner` 的 provider key 由插件注册；内核不需要为插件改名字。
- 安装 `ParticipantPlugin` 后，默认解析器的本地 `assign` 行为与 P1 一致。
- 插件/嵌入方替换解析器后，`assign` 使用新结果；授权仍由内核按
  `__assignment` 检查。
- 无提供者时发布含 `assign.owner` 的模型返回错误，点名属性和安装提示；
  活动启动路径保留同样的 fail-closed 错误。
- 同一能力类型多提供者且模型未选择时发布失败；固化 key 缺失时恢复失败，
  不静默回退。
- 启动后注册能力/解析器返回 `ActError::Runtime`，启动时的实现保持生效。
- `ParticipantPlugin::on_init` 注册 `OwnerPackage`、能力广播和解析器。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`。

### 5.4 验证（2026-10-29）

- `cargo test -p acts --lib`：1719 passed。
- `cargo test -p acts-plugin-participant --lib`：12 passed（含 provider 选择、
  无提供者发布失败、启动后封口、替换解析器、授权回归）。
- `cargo fmt --check`：通过。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`：
  通过。

## 6. P4：服务端与端到端

### 6.1 范围

- `acts-server` 安装 `ParticipantPlugin`（`on_init` 注册 `OwnerPackage`；P2
  已接入，P4 补文档与端到端）。
- 文档：
  - `doc/src/model/assign.md`（已写 IRQ 版本，补插件注册说明）。
  - `doc/design/participant-plugin.md`（状态更新）。
  - `CHANGELOG.md`（Unreleased）。
- 端到端：
  - 业务系统应答参与人 -> `parallel` -> 每项 owner。
  - `assign` 本地 context -> 单 owner。
  - 转办/代办保持现有行为（回归）。

### 6.2 验收

- `cargo test -p acts-server --lib`。
- `cargo test -p acts --lib`。
- `cargo test -p acts-plugin-participant --lib`。
- 手动/测试驱动一次 `acts.core.owner` 请求，确认 params 中只有业务原子和
  `user`。

### 6.3 验证（2026-10-29）

- `cargo test -p acts-server --lib`：22 passed；新增
  `server_engine_resolves_assign_owner_through_the_participant_plugin`，走
  server 的 `engine_builder` + `ParticipantPlugin` 完成发布、本地 owner 解析
  和决策。
- `cargo test -p acts --lib`：1719 passed。
- `cargo test -p acts-plugin-participant --lib`：12 passed；
  `pack_owner_prepares_context_atoms_for_the_business_system` 覆盖 `acts.core.owner`
  请求中的本地 context 原子变成 `user`。
- `cargo fmt --check`、workspace clippy：通过。

## 7. P5：命令/审计端口（未来）

### 7.1 候选

- `ParticipantCommand` 端口：插件校验 transfer/delegate 的业务规则
  （目标用户存在、岗位允许），内核保留 owner/代办人授权和 revision 比较。
- `ParticipantAudit` 端口：转办/代办/决策的审计 sink。
- 参与人通知：转办/代办后通知旧 owner/新 owner。

### 7.2 约束

- 端口不能修改 `__assignment`/`__delegation` 的授权语义。
- 命令端口在 serve 前装配；失败不改变内核状态。
- 不做 standing delegation/候选池。

## 8. 风险与回滚

| 风险 | 处理 |
| --- | --- |
| 包迁出后 `acts` 单测找不到包 | P2 把 owner 测试一并迁移；`acts` 不再依赖该包 |
| `prepare_params` 影响 Func 包 | 默认 no-op；只在 owner 包实现；效果运行器先 validate 再 prepare |
| 解析器端口引入 I/O | 端口同步；业务解析走 IRQ，启动路径不做 I/O |
| 授权下沉 | 禁止；`__assignment`/`__delegation` 仍由内核检查 |
| fan-out 的 `value` 可见性 | P1 遗留项；用显式 `Step` id 固定用例，必要时修 `scope_vars`/assign 作用域 |
| 公开 API semver | P3 的 `ParticipantResolver`/`Context::resolve_owners` 一旦公开按 semver 维护 |

## 9. 完成定义

- 参与人插件拥有 `acts.core.owner` 包和解析器注册；内核不再有 owner 包。
- `assign` 的本地原子、单 owner 校验、授权行为不变。
- 没有 `[owner]` 配置、没有业务目录副本。
- 文档、changelog、全量 gate 通过。
