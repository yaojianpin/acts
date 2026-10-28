# 参与人插件实施计划（assignment + owner）

- 设计文档：
  - [`doc/design/participant-plugin.md`](../design/participant-plugin.md)
  - [`doc/design/owner-expr.md`](../design/owner-expr.md)
  - [`doc/design/shell-extensibility.md`](../design/shell-extensibility.md)
- 状态：P2 已落地，P3 待执行
- 执行方式：一次只执行一个阶段；上一阶段验收通过后，才生成并执行下一阶段。

## 1. 阶段总览

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P1 | `OwnerExpr` + 本地原子 + 上下文变量 + `assign` 集成 | 已落地（owner/assign 测试通过） |
| P2 | `acts.core.owner` IRQ 包迁入参与人插件 + `prepare_params` | 已落地（插件测试/内核/服务端全绿） |
| P3 | `ParticipantResolver` 端口 + 插件注册 | 待执行 |
| P4 | `acts-server` 注册 + 文档 + 端到端验证 | 待执行 |
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
- P3 未开始；`ParticipantResolver` 端口未实现。

## 5. P3：`ParticipantResolver` 端口

### 5.1 目标

公开参与人解析端口，让插件/嵌入方替换 `assign` 的本地解析；默认实现保持内核
行为。

### 5.2 范围

- `crates/acts/src/model/owner.rs` 或新 `participant` 模块：
  ```rust
  pub trait ParticipantResolver: Send + Sync {
      fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>>;
  }
  ```
- `LocalParticipantResolver`：包装现有 `shell::owner::evaluate`。
- `Runtime`/`Engine` 槽位：`register_participant_resolver`；默认实现；
  serve 前装配。
- `Context::resolve_owners` 改为调用当前解析器。
- `ParticipantPlugin::on_init` 注册默认解析器（或自定义）。
- 测试：一个替换解析器的用例证明端口生效；默认用例行为不变。

### 5.3 验收

- 默认引擎的 `assign` 行为与 P1 完全一致。
- 插件/嵌入方替换解析器后，`assign` 使用新结果；授权仍由内核按
  `__assignment` 检查。
- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`。

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
