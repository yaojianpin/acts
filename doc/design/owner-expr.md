# 参与人表达式（OwnerExpr）与 IRQ 扩展设计

- 状态：定稿
- 日期：2026-10-28
- 范围：`crates/acts`、`crates/acts-server`、`doc/`

## 1. 背景

现有实现把承办人（owner）当作一个人：

```rust
pub struct Assign {
    pub owner: String, // 用户 id 或 `${{ expression }}`
}
```

`Assign.owner` 在活动启动时通过 `fill_params` 解析成一个字符串，写入任务的私有变量
`__assignment`，再投影到活动消息。它解决了“谁可以决策这个人类活动”，但没有解决
“这个活动应该交给谁”：部门正职、项目负责人、当前操作人所在单位的负责人、上级
等业务关系只能由调用方在启动前算好，写成一个用户 id。

本设计把 owner 扩展为表达式语言，并把业务数据（角色、组、资源负责人、关系）的
解析交给业务系统：`acts.core.owner` 是一个 IRQ 活动，引擎把表达式作为请求发给
业务系统，业务系统返回参与人（`id`/`code`/`name`）。核心不内置用户目录，也没有
`[owner]` 配置。

### 1.1 目标

1. `assign.owner` 接受 `OwnerExpr`（保留字符串旧形式）。
2. 表达式可以求值为一个参与人集合，为后续 `block`/`sequence`/`parallel`
   审批提供预定义参与人。
3. 新增 `acts.core.owner` IRQ 包：把 owner 表达式发给业务系统，返回
   `{ participants: [{id, code, name}], ids, count }`。
4. 支持运行时上下文变量：`initiator`、`current_actor`、
   `previous_approver` / `previous_approver_nth`，以及持有用户的普通变量。
5. 业务数据不进入核心，也不进入插件配置：由业务系统在 IRQ 应答里给出。
6. 兼容现有的字符串 owner 和 `__assignment` 状态。

### 1.2 非目标

- 核心内置用户目录、角色表或组织架构。
- 用插件配置（`[owner]`）维护一份业务目录副本。
- 单个活动拥有多个 owner（候选池）。一个活动仍然只有一个 owner；多人审批由
  多个活动 + `join` 表达。
- 在调度器的纯计划层做 I/O。

## 2. 现状与约束

- `assign` 只允许写在人类决策叶子（`ActRunAs::Irq`）上；容器、`Func`/`Msg`
  叶子在活动启动时被拒绝。
- 启动解析发生在 `ExecuteTask` 的 shell 阶段：构造 headless scope、执行
  `prepare()`，再走和包参数相同的表达式路径。解析结果随启动提交写入
  `__assignment`，重试会重新解析，提交后以持久状态为准。
- `acts.core.parallel{ in, steps }` / `sequence` 已经按 `in` 数组在运行期展开
  出多个子活动；`in` 在容器启动时解析并固定到 `act_facts.params`。因此
  “先算参与人，再 fan-out”不需要新的调度语义。
- 核心没有用户目录，`ScopePolicy.subject` 是流程发起人，不是审批人；决策主体
  记录在每个任务的私有 `__action@<sequence>` 事实里。
- IRQ 包是引擎既有的扩展方式：包定义 + 客户端决策，不需要核心理解业务。

## 3. 架构决策

### 3.1 业务数据由业务系统解析

`acts.core.owner` 是 IRQ 活动，不是求值器：

1. 工作流声明 `uses: acts.core.owner`，params 就是 owner 表达式。
2. 引擎像任何 IRQ 活动一样把 params 发给业务系统（`Created` 消息）。
3. 业务系统用自己的用户目录、组织架构、项目库解析表达式，完成活动并返回：
   ```json
   {
     "participants": [{ "id": "u1", "code": "alice", "name": "Alice" }],
     "ids": ["u1"],
     "count": 1
   }
   ```
4. 后续 `parallel`/`sequence`/`block` 用 `participants` 或 `ids` 展开审批活动。

### 3.2 本地原子与业务原子

核心只解析不需要业务数据的部分：

| 原子 | 解析方 |
| --- | --- |
| `value`（用户 id / `${{ }}` 表达式） | 核心 |
| `user` | 核心 |
| `context`（`initiator`/`current_actor`/`previous_approver`） | 核心 |
| `context.var` 的 `user`/`users` 类型 | 核心（变量值就是用户） |
| `any_of`/`all_of`/`except` | 核心（对已解析集合做代数） |
| `role`/`group` | 业务系统 |
| `owners_of` | 业务系统 |
| `relate` | 业务系统 |
| `context.var` 的 `role`/`group`/`project`/`department`/`unit` 类型 | 业务系统 |

`assign.owner` 在活动启动时必须立刻得到一个 owner，所以它只接受本地原子；
出现业务原子时活动失败，错误信息点名 `acts.core.owner`：

```text
owner atom 'role' ('reviewer') names business data; resolve it through the
acts.core.owner IRQ package — pass this expression as the package's params,
let the business system return the participants, and assign from the returned ids
```

`acts.core.owner` 的 params 可以包含全部原子，因为业务系统会解析它们。

`assign.owner` 的解析通过 `ParticipantResolver` 端口完成。内核只定义能力类型
和属性映射（`assign.owner` → `CapabilityKind::Participant`）以及确定性本地
求值原语；参与人插件用自己的 provider key 注册默认 `LocalParticipantResolver`。
默认实现仍只解析本地原子，业务原子继续要求显式 `acts.core.owner` 步骤；替换
解析器可以改变“本地表达式解析成谁”，但不能改变单 owner、`__assignment` 和
决策授权。没有 provider 时发布模型或启动活动会明确失败，不会产生无人审批。

### 3.3 为什么不做插件目录

方案对比：

- **插件配置目录**（`[owner]` + `StaticOwnerDirectory`）：核心/插件维护一份
  业务目录副本，部署时必须同步组织架构；目录会漂移，且每个部署都要重复配置。
  它还把“业务数据”变成了引擎配置，违背“核心不实现用户目录”的非目标。
- **插件实现 `OwnerDirectory` 接口**：比静态配置灵活，但仍要求引擎在启动活动时
  同步调用业务系统。IRQ 本身已经是持久、可重试、可审计的调用方式；再造一套
  同步目录接口会让 `assign` 的启动路径依赖外部 I/O，并出现两套业务数据访问
  方式。
- **IRQ 包（采用）**：复用既有的客户端决策通道。业务系统可用任何目录、任何
  语言解析；引擎只定义表达式文档和应答字段。没有插件配置，没有目录副本，
  没有新的 shell 能力。

因此 `acts.core.owner` 是一个 IRQ 包（P2 起由参与人插件提供，与
`acts.core.irq` 同一运行方式），只定义请求契约；业务系统是唯一的解析方。

### 3.4 调度器影响

- `acts.core.owner` 是普通 `ActRunAs::Irq` 包：`ExecuteTask` 启动、等待客户端、
  `Complete` 后继续。没有新的操作类型、没有新的 plan 分支。
- 参与人 fan-out 复用 `acts.core.parallel.in`/`sequence`；`in` 在容器启动时
  解析并固定，子活动各自解析 `assign.owner`。
- 核心新增的代码只有三块：表达式模型、`assign` 的本地原子解析、上下文变量。
  业务解析、目录、配置都不在 shell。

## 4. 语言

### 4.1 类型

```rust
pub enum OwnerExpr {
    Value(String),                       // 旧形式：用户 id 或 `${{ … }}`
    User { name: String },
    Role { name: String },
    Group { name: String },
    OwnersOf { resource: ResourceRef, duty: Duty },
    Context { var: ContextVar },
    Relate { from: Box<OwnerExpr>, rel: Relation, duty: Option<Duty> },
    AnyOf { operands: Vec<OwnerExpr> },
    AllOf { operands: Vec<OwnerExpr> },
    Except { base: Box<OwnerExpr>, minus: Box<OwnerExpr> },
}
```

- `Value` 保持旧 JSON：`"alice"`、`"${{ reviewer }}"`。
- 对象形式用 `kind` 标签；`ResourceRef` 用 `resource` 标签；`ContextVar` 用
  `type` 标签。
- `Duty`：`all`（默认）、`principal`、`deputy`、`member`、`{ custom = "…" }`。
- `Relation`：命名字符串（`department_of`、`manager_of`、…）或
  `{ "rel": "resource", "value": "department_of" }` 的部署自定义形式。
- `VarType`：`user`（默认）、`users`、`role`、`group`、`project`、
  `department`、`unit`。

文档中的三个例子：

```json
{ "kind": "any_of", "operands": [
    { "kind": "context", "var": { "type": "initiator" } },
    { "kind": "all_of", "operands": [
        { "kind": "owners_of",
          "resource": { "resource": "department", "name": "研发部" },
          "duty": "principal" },
        { "kind": "context",
          "var": { "type": "var", "name": "owner_unit", "ty": "unit", "duty": "principal" } }
    ]},
    { "kind": "except",
      "base": { "kind": "owners_of",
                "resource": { "resource": "project", "name": "P1" },
                "duty": "deputy" },
      "minus": { "kind": "context", "var": { "type": "current_actor" } } }
] }
```

```json
{ "kind": "relate",
  "from": { "kind": "context", "var": { "type": "current_actor" } },
  "rel": { "rel": "resource", "value": "department_of" },
  "duty": "principal" }
```

```json
{ "kind": "relate",
  "from": { "kind": "relate",
            "from": { "kind": "owners_of",
                      "resource": { "resource": "project", "name": "P1" },
                      "duty": "principal" },
            "rel": { "rel": "subject", "value": "manager_of" } },
  "rel": { "rel": "resource", "value": "department_of" },
  "duty": "principal" }
```

这些例子作为 `acts.core.owner` 的 params 原样发给业务系统。

### 4.2 求值语义

本地求值：

- 每个原子求值为 `Vec<Participant>`；`Participant` 至少包含 `id`，
  `code`/`name` 缺省时等于 `id`。
- `any_of`：并集，按 `id` 去重，保留首次出现顺序。
- `all_of`：交集（任意一个为空则结果为空）。
- `except`：差集（`base - minus`）。
- 空结果不是错误：`assign` 会把空结果翻译成“解析为空，拒绝无人审批”。

业务系统求值（`acts.core.owner` 的客户端）应遵循同样的集合语义；引擎不校验
应答，除了 `assign` 的单 owner 检查。

## 5. 上下文变量

| 变量 | 含义 |
| --- | --- |
| `initiator` | 流程发起人：启动时密封进 `ScopePolicy.subject` 的主体；引擎内部启动/定时触发时为空 |
| `current_actor` | 最近一次决策的主体：从当前执行沿**前驱图**（顶层步骤的父节点是流程根，不是上一步）找到的最新 `__action@N.subject`；尚无决策时回落到 `initiator` |
| `previous_approver` | `current_actor` 之前的那次决策；按决策计数而不是按人去重 |
| `previous_approver_nth { back }` | 再往前 `back` 次决策 |
| `var { name, ty, duty }` | 工作流变量；`user`/`users` 由核心读，角色/组/资源类型由业务系统解析 |

决策链的走法：先读当前执行自己的 `__action@N`（新的在前），再按顺序走
`predecessors`；没有前驱时走 `parent`（嵌套流程的根）。并行 join 有多个前驱时
按快照中的前驱顺序取，结果确定但“上一审批人”本身是业务上的近似——需要精确
指定时用 `previous_approver_nth` 或显式变量。

表达式里也可以直接读：`$initiator`、`$actor`、`$previous_approver(n)`。

`acts.core.owner` 在请求发出前调用 `ActPackage::prepare_params`，把表达式里的
**本地** context 原子（`initiator`、`current_actor`、`previous_approver`、
`user`/`users` 变量）替换成它们解析出的 `user` 节点；业务原子（角色、组、资源、
关系）保持原样。业务系统拿到的是一份自包含的表达式：只需要解析自己的目录数据，
不需要知道这次运行的发起人/操作人。没有解析结果的本地原子变成空集合
（`any_of: []`），未知的用户变量让请求失败——引擎已经知道它不存在，不把错误
推给业务系统。

## 6. 工作流写法

### 6.1 单个 owner（本地原子）

```yml
- id: approve
  uses: acts.core.irq
  assign:
    owner:
      kind: context
      var: { type: previous_approver }
```

### 6.2 多人审批（业务系统解析）

```yml
- id: owners
  uses: acts.core.owner
  params:
    kind: any_of
    operands:
      - kind: owners_of
        resource: { resource: department, name: 研发部 }
        duty: principal
      - kind: context
        var: { type: current_actor }

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

`owners.participants` 在 `parallel` 启动时解析成数组并固定；每个 item 的
`value.id` 是参与人 id。`join` 决定或签/会签/阈值。

客户端应答：

```json
{
  "participants": [
    { "id": "u1", "code": "alice", "name": "Alice" },
    { "id": "u2", "code": "bob", "name": "Bob" }
  ],
  "ids": ["u1", "u2"],
  "count": 2
}
```

如果只需要 fan-out，客户端也可以只返回 `ids`（字符串数组），工作流用
`in: "${{ owners.ids }}"`、`assign.owner: "${{ value }}"`；`participants`
用于需要显示名的场景。两者都提供时以 `participants` 为准，`ids` 是便捷投影。

### 6.3 单个 owner 但需要业务数据

先解析，再取唯一结果：

```yml
- id: owner
  uses: acts.core.owner
  params:
    kind: relate
    from: { kind: context, var: { type: current_actor } }
    rel: { rel: resource, value: department_of }
    duty: principal

- id: approve
  uses: acts.core.irq
  assign:
    owner: "${{ owner.participants[0].id }}"
```

如果业务系统返回多个参与人，由工作流决定取第一个还是改用 `parallel`；
`assign.owner` 自身拒绝多结果，避免静默选择。

## 7. 复杂度与风险

| 组件 | 位置 | 规模 | 风险 |
| --- | --- | --- | --- |
| 表达式类型 + serde | `crates/acts/src/model/owner.rs` | ~450 行（含测试） | 低：纯数据 |
| 本地求值器 + 集合运算 | `crates/acts/src/scheduler/shell/owner.rs` | ~200 行 | 低：无 I/O |
| 上下文变量 | `Context` + `Scope` | ~120 行 | 中：决策链是前驱图，不是父子链 |
| `assign` 集成 | `assign/mod.rs`、`operation/task.rs` | ~60 行改动 | 低：沿用现有解析阶段 |
| `acts.core.owner` 包 | `plugins/acts-plugin-participant/src/package.rs` | ~100 行 | 低：纯 IRQ 定义 |
| 文档 + 测试 | 本目录 + 各 crate | ~400 行 | 中：端到端审批链 |

总计约 1,300 行（含测试和文档），且没有插件目录、没有 `[owner]` 配置、没有
新的 shell 能力。

风险：

- **业务系统可用性**：`acts.core.owner` 是一个 IRQ 活动，业务系统不在线时活动
  会等待（与任何审批一致），不会让引擎启动失败。
- **应答契约**：引擎不校验业务系统的应答字段；工作流应使用 `participants`/
  `ids`，客户端负责返回可解析的参与人。`assign` 仍会校验单 owner 和非空 id。
- **`assign` 不能用业务原子**：这是有意的。启动路径不做业务 I/O；需要业务数据
  的单 owner 先走 `acts.core.owner`，再从结果里取。
- **决策链的并行 join**：多个前驱时“上一审批人”按快照顺序取；文档已说明。
- **`owner` 用户变量名**：不引入；上下文变量是内建名称，避免与业务变量冲突。

## 8. 兼容性

- 旧工作流：`assign: { owner: "u1" }` / `assign: { owner: "${{ reviewer }}" }`
  反序列化为 `OwnerExpr::Value`，行为和消息投影不变；`__assignment` 结构不变，
  不需要数据迁移。
- `with_assign` 仍接受字符串；新增 `with_assign_expr(OwnerExpr)`。
- 解析时机不变：`ExecuteTask` 的 shell 阶段、`prepare()` 之后；业务数据不在
  这条路径上。
- 部署不需要新配置节。

## 9. 与 shell 扩展性的关系

`acts.core.owner` 是包，不是 shell 模块：业务解析在客户端，引擎侧只有一个
包定义和 IRQ 生命周期。新增参与人能力（新的原子、新的应答字段）优先扩展
表达式文档或包契约，而不是往 `scheduler/shell` 加代码。shell 的拆分计划见
[`shell-extensibility.md`](./shell-extensibility.md)。

## 10. 结论

采用 IRQ 方案：`OwnerExpr` 是核心工作流语言的一部分，`assign.owner` 解析
本地原子；业务数据由 `acts.core.owner` 以 IRQ 请求发给业务系统，业务系统返回
参与人，工作流用现有的 `block`/`sequence`/`parallel` 和 `join` 展开审批。
核心没有用户目录，没有插件目录配置，调度器没有新的审批语义。
