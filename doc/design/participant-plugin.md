# 参与人插件设计（assignment + owner）

- 状态：P3 已落地（`ParticipantResolver` 端口、`CapabilityKind::Participant`、插件 provider key 注册与固化）；P4 服务端/文档/端到端已接入
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
| 本地原子求值（user/context/set） | 内核提供确定性原语；默认 `LocalParticipantResolver` 在**参与人插件** | 无 I/O；解析端口可替换，无插件时不支持 |
| 表达式变量/方法扩展（`ActUserVar`/`ActFunction`） | 插件/嵌入方注册，内核环境解析 | 让插件能用变量和方法实现自己的解析规则 |
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

### 3.1 `acts-plugin-participant` crate（P2 已落地；P3 原型）

```text
plugins/acts-plugin-participant/
  src/lib.rs
  src/package.rs      // acts.core.owner IRQ 包
  src/plugin.rs       // ParticipantPlugin
  src/resolver.rs     // LocalParticipantResolver（默认解析器）
  src/tests.rs
```

- `OwnerPackage`：`ActRunAs::Irq`，params 是 `OwnerExpr`（原始或 `{ owner: … }`）。
  - `prepare_params` 把本地 context 原子替换成解析出的 `user`，业务原子原样
    发给客户端。
  - 客户端应答 `{ participants: [{id, code, name}], ids, count }`。
- `ParticipantPlugin`：`ActPlugin::on_init` 注册 `acts.core.owner` 包
  （`Engine::add_package` 直接写 runtime 包注册表，引擎在 `start` 期间发布定义），
  广播 `CapabilityKind::Participant`，并用自己的 provider key 注册解析器。
  默认是插件自己的 `LocalParticipantResolver`；
  `ParticipantPlugin::with_resolver` 安装替换实现。
- provider key 由插件定义（例如 `acts-plugin-participant` 或
  `acme.participant`），不在 `acts` 做插件名常量；内核只定义能力类型和
  `assign.owner → Participant` 的属性映射。
- 只安装插件即可获得包、解析器与 `assign.owner` 能力；没有插件的引擎发布
  含 `assign.owner` 的模型会失败并点名属性和提供者，活动启动路径也有同样的
  fail-closed 错误。

### 3.2 内核端口

```rust
/// Resolve the local atoms of an owner expression.
pub trait ParticipantResolver: Send + Sync {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>>;
}
```

- 内核不再提供默认解析器。`Context::resolve_local_owners` 是确定性本地求值
  原语（用户、上下文变量、用户变量、集合运算），业务原子仍报错并点名
  `acts.core.owner`；插件的 `LocalParticipantResolver` 是对它的薄适配器。
- `assign.owner` 通过 `Context::resolve_owners` 调用当前解析器；插件可以替换
  （例如接入自定义规则、审计、黑白名单），但业务数据仍走 IRQ 包。
- 端口是同步的：`assign` 在活动启动路径上，不做 I/O。
- `Engine::register_participant_resolver` 只在 `ActPlugin::on_init` 期间可用；
  启动 pass 在插件钩子之后封口解析器槽位，启动后替换返回 `ActError::Runtime`。
- 能力注册：`ParticipantPlugin::on_init` 广播
  `CapabilityKind::Participant`，并用自己的 provider key 注册解析器。
  `Workflow::required_capabilities` 返回能力类型；发布时按如下规则选提供者：
  - 模型声明 `provider` → 用该 key；
  - 未声明且该类型只有一个提供者 → 用唯一提供者；
  - 未声明且有多个 → 发布失败，要求显式选择。
  发布成功后把 `(kind, key, version)` 固化到 process spec；活动启动按固化 key
  取类型化端口。缺失 → `ActError::Model` / fail closed，不静默回退。能力表在
  启动 pass 封口。

### 3.3 表达式环境扩展

解析规则需要运行时配置或业务约定时，插件实现 `ParticipantResolver`，并通过
引擎环境读取插件注册的变量和方法：

- `ActUserVar`：注册一个命名变量（如 `secrets`），表达式按名字读取；
- `ActFunction`：注册一个表达式方法（如 `owner_of(...)`），由插件实现参数校验
  和结果转换。函数在表达式引用它时才注入；引擎内建方法优先。

插件在 `on_init` 中用 `Engine::register_env_var` / `Engine::register_env_function`
注册；嵌入方在运行时用 `executor.ext().register_var` / `register_function`，
走 ACL 检查。变量/方法只扩展“表达式能读到什么”，不改变
`__assignment`/`__delegation` 的授权语义。

### 3.4 服务端

`acts-server` 注册：

```rust
.add_plugin(&acts_plugin_participant::ParticipantPlugin::new())
```

`ParticipantPlugin::on_init` 注册 `acts.core.owner` 包和默认
`LocalParticipantResolver`（或构造插件时传入的替换实现）。

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
3. **P3（原型暂停，契约调整）**：`assign.owner` 作为模型原语保留在内核；
   `CapabilityKind::Participant` 定义能力类型，插件用自己的 provider key 注册
   `ParticipantResolver`；`Context::resolve_owners` 按固化 key 调类型化端口；
   无插件/无提供者时发布和启动明确失败。表达式环境的 `ActUserVar`/`ActFunction`
   扩展同时落地。实现需从“内核插件名常量 + 能力字符串”调整为“能力类型 +
   provider key + typed PortSet”。
4. **P4（待执行）**：`acts-server` 端到端验证、文档收尾与 changelog 完整说明。
5. **P5（待设计）**：参与人命令/审计端口。如果确实需要，命令端口只参与
   transfer/delegate 的 *payload 校验*，内核保留最终授权、revision 比较和计划。

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
- **包注册时机**：`ActPlugin::on_init` 同步，包定义发布异步；插件通过
  `Engine::add_package` 直接注册 runtime，引擎在 `start` 中发布定义后再返回，
  所以只安装插件即可，不会出现“包未发布就开始工作流”的窗口。
- **没有配置目录**：不引入 `[owner]`，避免目录副本漂移。
- **公开 API**：`ParticipantResolver`/`Context::resolve_owners` 一旦公开就是
  semver 契约；默认实现保证行为不变。
- **提供者选择**：同一能力类型有多个 provider 时必须显式或唯一；解析后的
  `(kind, key, version)` 固化到 process spec，恢复时缺失就失败，不静默换实现。
- **key 归属**：插件 provider key 不进内核常量；内核只定义能力类型、属性映射
  和内部默认 key。

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
