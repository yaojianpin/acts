# 能力模型（caps）设计

- 状态：定稿（A–E 已落地 2026-10-30；内核/能力边界见 §3）
- 日期：2026-10-29（边界切分 2026-10-30）
- 范围：`crates/acts/src/scheduler/shell/caps`、`crates/acts/src/scheduler`、`crates/acts/src/export/status.rs`、`crates/acts-channel/src/model/status.rs`
- 取代：`design/shell-extensibility.md` §6 的 PortSet/能力注册方案、`design/participant-plugin.md`（已删除）

## 1. 决策

能力扩展统一走 **package（`uses`）**，不再走插件注册：

- 删除 `PortSet`（keyed 注册表 + default 选择）、`CapabilityKind` /
  `CapabilityProvider` / `CapabilityRequirement`、Engine 的
  `register_capability_provider` / `register_participant_resolver` /
  `register_effect_runner`，以及发布期的 provider 固化（`assign.provider`、
  `freeze_step_capabilities`、`resolve_model_capabilities`）和封口逻辑。
- 参与人插件 `acts-plugin-participant`（`ParticipantResolver` 端口 +
  `acts.core.owner` 包）整体删除；业务参与人解析由
  `acts.core.candidates` IRQ 包承担。
- 内核能力 = **trait + 一个实现 + key**；scheduler 是能力的调度与组装：
  `Workers`（原 KernelHost）装配 `CapServices`（原 KernelPorts），每个槽位
  一个 `Arc<dyn Trait>`。

替换实现的方式只有两种：装配期换槽位实现（测试、嵌入方在构造宿主时），
或模型层把 `uses` 指向另一个包（业务扩展）。

**边界切分（2026-10-30，用户决策）**：每个能力一个目录 + 一个能力类型——
类型实现 `Capability`（自带 key），该能力的纯语义与编排都是这个类型的
方法，不做散落自由函数，更不堆到 `Workers`；目录不必迁出 scheduler
（如 `shell/caps/assign`）。`Workers` 只做槽位管理（`attach_*`）与分发；
能力不回调内核，副作用只经返回值/计划意图，由内核 commit。

## 2. 核心概念

```rust
// caps/mod.rs
pub trait Capability: Send + Sync {
    /// 能力 key：`sys:status` 的 caps 列表与诊断用它点名实现。
    fn key(&self) -> &str;
}
```

每个能力 trait 以 `Capability` 为 supertrait，实现自带 key。

### 2.1 端口（shell 的可替换缝）

| 端口 | key（内置实现） | 槽位 | 说明 |
| --- | --- | --- | --- |
| `EffectRunner` | `kernel.effect`（`PackageEffectRunner`） | `Option<Arc<_>>` | 包效果执行；未装配时需要效果的操作失败 |
| `PredicateRunner` | `kernel.predicate`（`EnginePredicateRunner`） | `Option<Arc<_>>` | 分支谓词；未装配时回退内置子集求值 |
| `OutboxTransport` | `kernel.delivery`（`MessageForwarder`）/ `shadow.delivery`（`ShadowTransport`，默认） | `Arc<_>` | outbox 投递 |
| `LifecycleSink` | `kernel.lifecycle`（`FnLifecycleSink` 适配闭包） | `Option<Arc<_>>` | 运行生命周期上报 |
| `AssignmentResolver` | `kernel.assign`（`RuntimeAssignmentResolver`） | `Option<Arc<_>>` | `assign.owner` 字符串 → 单 owner；并本地计算 `assign.candidates` 的全本地表达式（算不了转 `uses` 包）。未装配时声明的 assign 失败关闭（act 绝不无主运行） |

### 2.2 CapServices 与 Workers

```rust
// caps/services.rs（原 kernel_ports.rs）
pub struct CapServices {
    pub effects: Option<Arc<dyn EffectRunner>>,
    pub(crate) predicates: Option<Arc<dyn PredicateRunner>>,
    pub(crate) delivery: Arc<dyn OutboxTransport>,
    pub(crate) lifecycle: Option<Arc<dyn LifecycleSink>>,
    pub(crate) join: crate::caps::join::Join,
    pub(crate) assignment: Option<Arc<dyn AssignmentResolver>>,
}
```

- `Workers::new_with_admissions` 构造时给默认值（shadow transport）；
  `attach_effect_runner` / `attach_outbox_transport` / `attach_lifecycle_sink`
  / `attach_predicate_runner` / `attach_assignment_resolver` 在装配期安装
  实现（last wins，serve 前完成）。
- 没有 registry、没有 default key 选择、没有封口：一个槽位一个实现。

### 2.3 状态输出

`Runtime::caps()` → `Workers::caps()` → `CapServices::keys()`：
已安装能力的 key 列表，按槽位顺序。`sys:status`（`export/status.rs` 的
`SystemStatus`）增加 `caps: Vec<String>`；`acts-channel` 的 wire 镜像同步。

## 3. 调度内核与能力的边界（2026-10-30）

### 3.1 切分原则

- **每个能力一个目录 + 一个能力类型**：类型实现 `Capability`（自带 key），
  该能力的纯语义与编排都是这个类型的方法（`impl Assign`/`impl Join`），
  不做散落自由函数，更不堆到 `Workers`；目录不必迁出 scheduler。
- **`Workers` 只做两件事**：槽位管理（`attach_*`）与分发（把操作路由到
  能力方法，如 `resolve_assignment_owner` 读槽位后转交 resolver）。
- **能力不回调内核**：副作用只经返回值/计划意图，由内核 commit。
- **桥**：
  - 对外接缝（装配期可替换）：**Context 是标准桥**——内核构建作用域
    （`ScopeSpec` → headless `Context`：initiator、vars、决策事实、表达式
    环境），能力拿 Context + 请求求值，返回值即可（effect / predicate /
    assign.resolver）；delivery / lifecycle 的桥是 outbox 记录与生命周期
    事件。
  - 内核能力（进程内调用，语义扩展走 `uses` 包）：**snapshot/事实入、
    计划意图出**（`TransitionPlan` 的状态变更/变量写/操作/outbox）。

### 3.2 功能盘点

核心功能（调度内核，不成能力、不外迁）：

| 功能 | 位置 |
| --- | --- |
| 调度（驱动循环） | serve loop、claim、wake/deliver、liveness、admission、recovery |
| 执行 | `operation/task.rs`（ExecuteTask）：跑包、IRQ 等待、子流 |
| 编排 | `operation/{propagate,cascade,reentry,return_to_parent,timeout,action}.rs`、graph |
| 计划 | `core/plan/*`（branch/join/catch/fanout）、`TransitionPlan`、commit planner |
| 持久 | snapshot、recovery、timers、read_model 行 |
| 分发设施 | `Workers` + `CapServices`（槽位管理、操作路由） |

能力清单：

| 能力 | 目录 | 形态 | 桥 | 内核实现 |
| --- | --- | --- | --- | --- |
| effect | `shell/caps/effect` | 接缝 | Context | 同模块 `PackageEffectRunner` |
| predicate | `shell/caps/predicate` | 接缝 | Context | 同模块 `EnginePredicateRunner` |
| assign（owner 解析） | `shell/caps/assign/resolver.rs` | 接缝 | Context(ScopeSpec) | 同模块 `RuntimeAssignmentResolver` |
| delivery | `shell/caps/delivery` | 接缝 | outbox 记录 | `caps::message::MessageForwarder` / 同模块 `ShadowTransport` |
| lifecycle | `shell/caps/lifecycle` | 接缝 | 生命周期事件 | `FnLifecycleSink` |
| assign（授权/候选池） | `shell/caps/assign` | 内核能力 | snapshot/事实 → 计划意图 | `Assign` 的方法 |
| read（读投影） | `shell/caps/read/` | 槽位（`kernel.read`） | snapshot → legacy 行 | 同模块 `RowProjector` + `read_model` 的 `ProcRead` |
| join（会合） | `shell/caps/join/` + core `join_rules.rs` | 内核能力 | census → 计划意图 | 纯判定 `join_rules::decide`/`verdict`；编排为 `Join` 方法（`settlement`/`ask`/`apply_verdict`） |

### 3.3 调用关系（单向）

```
操作循环: 领取 → 构建 Context → 分发
  ① 对外接缝: cap(Context + 请求) → 返回值
  ② 内核能力: cap(snapshot/事实 + 请求) → 计划意图
  ③ commit 计划 → outbox → delivery 投递
```

### 3.4 既有 `impl Workers` 的收敛顺序（顺序即依赖序）

已完成：assign 的授权/候选池/声明检查/ask/settle 全部是 `Assign` 的方法
（`Workers` 仅剩 `attach_assignment_resolver` 与薄分发
`resolve_assignment_owner`）；join 的纯语义是 `core/plan/container/
join_rules.rs` 的 `decide`/`verdict`（caps/join 再导出）；**消息投影能力**
（2026-10-30）——`shell/caps/message/` 拆为 transport（`mod.rs` 的
`MessageForwarder`）与 payload（`payload.rs` 的 `step_uses`/
`act_message_payload`/`task_message_payload`/`operation_message_payload`/
`attach_join_message`，全部为 snapshot 上的自由函数）；join 编排已迁为
`Join` 方法（`settlement`/`ask`/`apply_verdict`，`propagate` 直呼）；**读投影**
（2026-10-30）——`read_model.rs`/`projection.rs` 归组为 `shell/caps/read/`，
`RowProjector` 槽位并入 `CapServices.read`（key `kernel.read`），
`Workers.projection` 字段删除（存取走 `Workers::projector()`）；**operation
规划方法收敛**（2026-10-30）——`ExecuteTask` 的守卫规划
（`guard_verdict`/`guard_skip_plan`）、`DispatchTimeout` 的超时规划
（`timeout_branches`/`timeout_guard`/`timeout_round`/`timeout_plan`/
`empty_remainder`）、`PropagateTask::flow_of` 已从 `impl Workers` 迁为各驱动
类型的关联方法（需要宿主设施的传 `host: &Workers`）；**`ApplyAction` 规划
方法组收敛**（2026-10-31）——`action.rs` 的 18 个规划方法（completion/
transfer/delegate/undelegate/push/catch 等 plan 与 fanout）全部迁为
`impl ApplyAction` 的关联方法，宿主设施（`task_operation_plan`、
`evaluate_branch_predicate`、`caps.effects`）经 `host: &Workers` 参数串入，
纯规划不带宿主；`ExecuteTask` 的失败路径直呼
`ApplyAction::failure_plan(host, …)`。

保留在内核：

1. 其余 `impl Workers`（`dispatch.rs` 的 `dispatch`/`execute_decision`/
   `mark_effect_unknown`、`return_to_parent.rs` 的 `ensure_parent_return`、
   `mod.rs` 的 `execute_one`）是调度/路由本体，按设计留在内核。

## 4. 能力目录

`caps/` 按"一种能力一个目录"组织，调度器只调用能力，不认识具体实现：

| 目录 | 能力 | 内容 |
| --- | --- | --- |
| `shell/caps/assign/owner.rs` | 责任人 | `Assignment` 状态、决策/改派授权（owner 或有效 delegatee）、assignment payload 投影（`impl Assign`） |
| `shell/caps/assign/delegate.rs` | 代办 | `Delegation` 状态、有效期语义、`expires_at`/`expect_rev` 传输形状解析、delegation payload 投影（`impl Assign`） |
| `shell/caps/assign/candidates.rs` | 候选人 | 候选池契约：`__candidates_wait`/`__candidates_ask`/`__candidates` 标记与 spec（`{ params, uses }`） |
| `shell/caps/assign/resolver.rs` | owner 解析缝 | `AssignmentResolver` trait（Context 桥，§2.1） |
| `shell/caps/join/` | 会合 | 身份 + 再导出 core `join_rules`（`decide`/`verdict`/`Decision`/`Verdict`）+ 编排（`Join` 的方法：`settlement`/`ask`/`apply_verdict`） |

切分规则（§3）：一个能力一个目录，契约（trait）与内核实现在同一目录
（多为同一文件——trait 段在上、内核实现在下）；assign 的纯语义与编排同居
`shell/caps/assign`（声明检查、transfer/delegate/undelegate 调用方检查、
候选人请求的 push（`Assign::ask`）与应答折叠（`Assign::settle`）、全本地候选
池的内核计算与 `assign.owner` 字符串的本地求值（`evaluate.rs`），都是
`impl Assign` 或其 resolver 缝的内核实现）。跨目录的只有能力模型本体：
`shell/caps/mod.rs`（`Capability`）与 `shell/caps/services.rs`（`CapServices`
槽位）。

## 5. 通过 package 扩展

内核只定义**信封 + 契约**，业务语义在客户端；模型用 `uses` 显式声明处理包。

### 5.1 候选人（assign.candidates）

```yaml
assign:
  candidates:
    params:            # CandidateExpr 文档（核心包的输入契约）
      kind: role
      name: reviewers
    uses: acts.core.candidates   # 默认值，可指向自己的包
```

- act start 先对 `params` 做**结构检查**：全部原子都是本地（用户、context
  变量、`user`/`users` 变量、集合运算、`${{ }}` 字符串）时直接在内核算出候选
  池，不发起请求（本地解析失败则活动启动失败，不伪造空池）；含业务原子时才
  请求 `uses` 包。
- `acts.core.candidates` 是 `package/core` 的 IRQ 包：`prepare_params` 把
  `params`（CandidateExpr）里的本地 context 原子与 `${{ }}` 字符串替换成解析
  结果后发给业务系统，应答 `{ participants, ids, count }`。
- 应答折叠进 act 变量：`candidates` = ids 列表（fan-out 与 owner 引用的
  主形态），`participants`/`count` 随行；声明 candidates 的 act 以
  `Waiting` 停靠，应答就绪后重入启动，容器 fan-out 由此读到候选人。
- 不经 `assign.candidates`、直接 `uses: acts.core.candidates` + `params` 的
  步骤是普通 IRQ 活动：没有本地求值与折叠，应答落在活动自己的输出上。

### 5.2 会合（join 的 delegated 规则）

```yaml
join:
  params:              # 自由格式，按容器 scope 解析
    action: countersign
  uses: acts.core.decide   # 默认值，可指向自己的包
```

- 内核信封固定为 `{ params, outcomes }`；`acts.core.decide` 约定
  `params.action` 为客户端业务处理器 key。
- `all`/`any`/`partial` 仍是本地判定（`caps/join`），不经包。

### 5.3 与 step 级 uses/params 的一致性

step 自身的 `uses`/`params`、`assign.candidates` 的 `params`/`uses`、join
的 `params`/`uses` 是同一模式：**params 是数据，uses 是能力的包名**。

## 6. 授权

有 assign 的 act，操作人（actor）必须与 `assignee` 一致（或与有效
delegation 的 `delegatee` 一致）方可执行决策；转办/代办的管理授权
（owner 或 `@assign` 能力）不变。仅有 candidates、没有 owner 的容器 act
不产生 `__assignment`，保持"无 assign 即 `@execute`"的旧行为。

## 7. 删除面（对照）

| 删除 | 原用途 |
| --- | --- |
| `acts-plugin-participant` crate | 参与人插件（resolver + `acts.core.owner` 包） |
| `caps::ParticipantResolver` 端口 | 参与人解析端口 |
| `PortSet` | keyed 注册表 |
| `CapabilityKind/Provider/Requirement` | 能力广告与发布检查 |
| `Engine::register_*`（capability/participant/effect） | 外部注册入口 |
| `Assign.provider` + `with_assign_provider` | 模型内固化 provider key |
| `freeze_step_capabilities` / `resolve_model_capabilities` / `seal_*` | 发布/启动固化与封口 |

## 8. `$assign` 读投影（2026-10-31 已实现）

`$assign` 是 assign 能力的只读表达式投影
（`shell/caps/assign/projection.rs` 的 `impl Assign`，经
`Scope::assign_projection` 桥接，`env/functions.rs` 的 `$assign` builtin 暴露）：

- 身份字段 `owner`/`assignee`/`delegator`/`delegatee`：取最近一个持
  `__assignment` 的执行；delegator/delegatee 取该执行自己的 `__delegation`
  （不从无关祖先继承）。
- 池字段 `candidates`/`ids`/`participants`/`count`：取最近一个已算出候选池
  （`__candidates`）的执行。与身份字段**各自**就近解析：容器生成的子 act
  带着自己的 owner，仍读到父容器的池。
- `$assign.at("<step-id>")`：显式指定执行（node id 取最新 generation，wire
  `tid` 取确切实例），只读该执行自身事实。
- 无 assign 事实时回答 `null` 身份与空列表，不报错；无共享聚合的作用域
  （谓词回退）回答空投影。

表达式语言的对象不可调用，因此显式形式是 `.at(...)` 方法，而不是
`$assign("...")`：这是对 `doc/plan/caps-refactor.md` §5 建议语法的唯一偏离，
语义（最近祖先 + 显式执行）不变。
