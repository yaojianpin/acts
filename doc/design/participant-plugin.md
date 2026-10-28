# 参与人插件设计（assignment + owner）

- 状态：方案（首批已落地：`OwnerExpr`/`assign`/IRQ 包在核心；P2 把包迁入插件）
- 日期：2026-10-28
- 范围：`crates/acts`、`plugins/acts-plugin-participant`、`crates/acts-server`

## 1. 目标

“参与人”不是单个字段，而是一组相关能力：

- `assign.owner`：人类活动启动时确定承办人。
- `OwnerExpr`：承办人/参与人的表达式语言（用户、上下文变量、角色、资源、
  关系、集合运算）。
- `acts.core.owner`：把需要业务数据的表达式发给业务系统，取回参与人。
- `__assignment` / `__delegation`：承办关系与代办的持久状态。
- `act:transfer` / `act:delegate` / `act:undelegate`：转办、代办、撤销代办。
- 决策授权：只有 owner（或有效代办人）可以决策。
- 或签/会签：参与人数组交给 `block`/`sequence`/`parallel` + `join`。

本设计把这些能力作为一个**参与人插件**来组织，而不是散落在 `scheduler/shell`。
插件拥有“参与人功能”的扩展部分；内核保留授权和持久状态，因为那是不允许被
插件绕过的安全边界。

## 2. 能力边界

| 能力 | 归属 | 理由 |
| --- | --- | --- |
| `OwnerExpr` 类型/serde | 内核 `model` | 工作流模型的一部分，随模型部署/校验 |
| `assign.owner` 字段 | 内核 `model` | 同上 |
| 本地原子求值（user/context/set） | 内核默认实现，可通过 `ParticipantResolver` 替换 | 无 I/O，`assign` 启动路径需要 |
| `acts.core.owner` IRQ 包 | **参与人插件** | 参与人功能的对外契约；业务数据由客户端解析 |
| 业务目录（角色/组/资源/关系） | **业务系统** | 部署数据，不在引擎/插件配置里复制 |
| `__assignment` / `__delegation` 状态 | 内核 | 决策授权的唯一来源；重启/重放必须一致 |
| 决策授权（owner/代办人） | 内核 | 安全边界；插件不能决定谁可以决策 |
| `act:transfer`/`delegate`/`undelegate` 的校验与计划 | 内核（首批）；未来可经命令端口暴露 | 修改 `__assignment`/`__delegation`，必须由内核授权 |
| 上下文变量（initiator/current_actor/previous_approver） | 内核 | 运行事实；包/插件只读 |
| 或签/会签 | 内核 `block`/`parallel`/`sequence` + `join` | 既有多人审批语义 |
| 参与人默认变量/配置 | 不引入 | 业务数据由 IRQ 应答；没有 `[owner]` 配置 |

原则：**插件可以决定“参与人是谁”，不能决定“谁有权决策”。** 后者永远由内核
在 `(pid, tid)` 上检查持久化的 `__assignment`/`__delegation`。

## 3. 插件组成

### 3.1 `acts-plugin-participant` crate（P2 目标）

```text
plugins/acts-plugin-participant/
  src/lib.rs
  src/package.rs      // acts.core.owner IRQ 包
  src/plugin.rs       // ParticipantPlugin
  src/tests.rs
```

- `OwnerPackage`：`ActRunAs::Irq`，params 是 `OwnerExpr`（原始或 `{ owner: … }`）。
  - `prepare_params` 把本地 context 原子替换成解析出的 `user`，业务原子原样
    发给客户端。
  - 客户端应答 `{ participants: [{id, code, name}], ids, count }`。
- `ParticipantPlugin`：`ActPlugin::on_init` 注册参与人解析端口（默认实现）；
  将来可以在这里注册参与人命令/策略。
- 包通过 `EngineBuilder::add_package::<OwnerPackage>()` 注册（发布包定义是
  async，`on_init` 是同步的）。

### 3.2 内核端口

```rust
/// Resolve the local atoms of an owner expression.
pub trait ParticipantResolver: Send + Sync {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>>;
}
```

- 内核默认 `LocalParticipantResolver`：用户、上下文变量、用户变量、集合运算。
- `assign` 通过 `Context::resolve_owners` 调用当前解析器；插件可以替换
  （例如接入自定义规则、审计、黑白名单），但业务数据仍建议走 IRQ 包。
- 端口是同步的：`assign` 在活动启动路径上，不做 I/O。
- `Engine::register_participant_resolver` 在 serve 前安装；不动态替换。

### 3.3 服务端

`acts-server` 注册：

```rust
.add_package::<acts_plugin_participant::OwnerPackage>()
.add_plugin(&acts_plugin_participant::ParticipantPlugin::new())
```

没有 `[owner]` 配置；业务系统是目录的唯一来源。

## 4. 工作流

### 4.1 单个 owner（本地原子）

```yml
- id: approve
  uses: acts.core.irq
  assign:
    owner:
      kind: context
      var: { type: previous_approver }
```

### 4.2 业务数据解析（IRQ）

```yml
- id: owners
  uses: acts.core.owner
  params:
    kind: owners_of
    resource: { resource: department, name: 研发部 }
    duty: principal

- id: approve
  uses: acts.core.parallel
  params:
    in: "${{ owners.participants }}"
    join: any
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ value.id }}"
```

### 4.3 转办/代办

沿用现有命令：

```text
act:transfer   {pid,tid,to,expect_rev,reason}
act:delegate   {pid,tid,delegatee,expires_at,reason}
act:undelegate {pid,tid}
```

首批仍由内核校验/计划：它们修改 `__assignment`/`__delegation`，是授权边界。
未来如果开放命令端口，端口只负责“如何改”，不能绕过内核的 owner/代办人检查
和 revision 比较。

## 5. 迁移步骤

1. **首批（本次）**：把 `acts.core.owner` 包从核心 `package/core` 移到
   `acts-plugin-participant`；公开 `ParticipantResolver` 端口，默认实现仍是
   内核的本地求值；`acts-server` 注册插件。
2. **P2**：把 `shell/assign` 的解析部分收敛到端口调用；`__assignment` 的
   写入/授权仍在内核。
3. **P3**：如果确实需要，开放 `ParticipantCommand` 端口，让插件参与
   transfer/delegate 的 *payload 校验*，内核保留最终授权和计划。
4. **P4**：参与人审计/通知端口（例如“转办后通知旧 owner”），插件实现。
5. **P5**：参与人目录的缓存/批量解析由业务系统或独立适配器负责，不进入
   引擎配置。

每一步都保持：内核状态机/授权不变，插件只扩展表达能力和外部数据边界。

## 6. 复杂度与风险

| 项 | 规模 | 风险 |
| --- | --- | --- |
| 包移动 + 插件壳 | ~250 行 | 低：包定义/参数准备 |
| `ParticipantResolver` 端口 | ~80 行 + 装配 | 低：同步、默认实现 |
| 服务端注册 | ~10 行 | 低 |
| 包测试迁移 | ~200 行 | 中：需要公开 API 驱动引擎 |
| 未来命令端口 | ~300 行 | 高：授权/revision 边界，必须设计评审 |

风险：

- **授权不能下沉**：插件替换解析器只影响“谁被解析出来”，不影响“谁可以
  决策”。`__assignment` 仍是唯一授权源。
- **启动路径不做 I/O**：`ParticipantResolver` 是同步的；业务解析走 IRQ 活动。
- **包注册时机**：`ActPlugin::on_init` 同步，包定义发布异步；服务端必须同时
  `add_package` 和 `add_plugin`。
- **没有配置目录**：不引入 `[owner]`，避免目录副本漂移。
- **公开 API**：`ParticipantResolver`/`Context::resolve_owners` 一旦公开就是
  semver 契约；默认实现保证行为不变。

## 7. 与 shell 扩展性的关系

参与人插件是 [`shell-extensibility.md`](./shell-extensibility.md) 的 P0/P1
示例：owner 包是 IRQ（无 shell 代码），解析器是端口（默认实现留在内核），
assignment 的持久状态/授权留在确定性内核。新增参与人能力时，优先扩展包契约
或端口，不再往 `scheduler/shell` 加业务模块。

## 8. 结论

参与人功能作为一个插件组织：`acts.core.owner` 包、参与人解析端口、未来的
命令/审计端口都在插件侧；内核只保留 `OwnerExpr` 模型、`__assignment`/
`__delegation` 状态和授权。这样 assignment 和 owner 属于同一个功能域，同时
不把安全边界交给插件。
