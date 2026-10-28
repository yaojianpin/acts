# 审批承办人、转办与代办实施计划

- 设计文档：[`doc/design/assignment.md`](../design/assignment.md)
- 分支：`feat/trans_delegate`
- 状态：待执行
- 执行方式：一次只执行一个阶段；上一阶段验收通过后，才生成并执行下一阶段提示词。

## 1. 阶段总览

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P1 | `assign.owner` + task 状态 + message 投影 + 决策授权 | 待执行 |
| P2 | 转办 `act:transfer` + 加签 `act:push` 的 assign | 待执行 |
| P3 | 代办 `act:delegate` / `act:undelegate` / 过期 | 待执行 |
| P4 | 客户端 API、用户文档、端到端验证 | 待执行 |

P1 合并了“数据模型”和“授权”两步，避免出现“assignment 已路由但未授权”的中间版本。

## 2. 执行规则

1. 每个阶段只执行该阶段范围，不提前实现下一阶段。
2. 每个阶段结束必须运行仓库要求的 gate，并记录实际执行的命令和结果。
3. 阶段完成条件：设计文档中的该阶段验收项全部有证据；未通过不得生成下一阶段提示词。
4. 生成下一阶段提示词时，只包含：
   - 目标阶段；
   - 上一阶段的实际变更和遗留；
   - 本阶段允许修改的文件/模块；
   - 必须复用的现有语义；
   - 禁止项；
   - 验收与验证命令。
5. 不允许通过新增 `candidates`、delegation collection、standing delegation、new task on transfer 来绕过问题。

## 3. P1：assignment 状态 + 决策授权

### 3.1 目标

让 human act 可以声明一个 `owner`，把它持久化为 task 状态，投影到 message/delivery，并在 kernel 的决策入口做 per-task 授权。

### 3.2 范围

- `Step.assign { owner: String }`，可选。
- 只允许人类决策叶子 act：`ActRunAs::Irq`。
- 禁止在 `acts.core.block` / `parallel` / `sequence` 容器自身声明 `assign`；叶子写在 `params.steps[]`。
- 新增 task 私有变量 `__assignment`：
  ```json
  {"owner":"u1","assignee":"u1","revision":0,
   "transferred_from":null,"updated_by":null,"updated_at":0}
  ```
- 在 act start 时用与 `params` 相同的表达式解析路径解析 `assign.owner`；空值 fail closed。
- 投影到 `Message` / `data::Message` / `data::Delivery` / `MessageInfo` / `acts-channel::model::Message`：
  `owner`、`assignee`、`delegator`、`delegatee`、`assignment_rev`、`ref_msg_id`。
- 写 `inputs.options.owner` / `inputs.options.assignee`，复用现有 channel glob。
- 决策授权：
  - 服务端从认证主体得到 operator，不信任客户端字段。
  - `owner` / `assignee` 可决策；未声明 `assign` 的任务保持旧 `@execute`。
  - 决策携带 `assignment_rev`；不匹配返回 `EASSIGNMENT_CHANGED`。
  - engine 规则触发的自动决策跳过人员校验。
  - ACL 关闭 / 无认证主体时保持旧行为，assignment 仅作路由提示。

### 3.3 主要落点

- `crates/acts/src/model/step.rs`
- `crates/acts/src/model/validation.rs`
- `crates/acts/src/scheduler/shell/facts.rs`
- `crates/acts/src/scheduler/shell/operation/task.rs`
- `crates/acts/src/scheduler/shell/mod.rs`
- `crates/acts/src/event/message.rs`
- `crates/acts/src/event/action.rs`（仅认证主体字段）
- `crates/acts/src/store/data/message.rs`
- `crates/acts/src/store/data/delivery.rs`
- `crates/acts/src/model/info.rs`
- `crates/acts/src/export/executor/act_executor.rs`
- `crates/acts/src/scheduler/runtime.rs`
- `crates/acts/src/scheduler/shell/operation/action.rs`
- `crates/acts/src/export/channel.rs`
- `crates/acts-channel/src/model/message.rs`
- 变更点附近已有测试文件；优先扩展而非新建文件。

### 3.4 验收

- `assign.owner` 能解析为字符串并持久化到 `__assignment`。
- `assign.owner` 为空/解析失败时 act 失败，不产生无人审批。
- 非人类 act（Func/Msg/容器）声明 `assign` 在 start 时失败，错误信息指出节点。
- 声明 `assign` 的任务：非 owner/assignee 的 `act:complete` 被拒。
- 未声明 `assign` 的任务：旧行为保持。
- `act:complete` 的 Completed 消息带 `owner`、`assignee`、`assignment_rev`。
- `inputs.options.owner` / `.assignee` 可被 channel glob 匹配。
- ACL 关闭时旧行为保持。
- 消息/投递 schema 升级带 `#[serde(default)]` 与 upcast；旧行可读。

### 3.5 验证

```sh
cargo fmt
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo test -p acts --lib
cargo test -p acts --doc
cargo test -p acts-channel --lib
```

如本地缺 NATS/Postgres，按 `AGENTS.md` 说明未跑的套件，不得声称全绿。

### 3.6 不做

- `act:transfer`
- `act:delegate` / `act:undelegate`
- `candidates`
- delegation collection
- `ChannelOptions.mine`
- 新 task / complete 原 task 的转办实现

## 4. P2：转办 + 加签

### 4.1 目标

实现 `act:transfer`，并让 `act:push` 的新 human act 可以声明 `assign.owner`。

### 4.2 范围

- 新增 action：
  ```text
  act:transfer {pid,tid,to,expect_rev,reason}
  ```
- 授权：当前 `owner` 或 `assign` 能力。
- 校验：task 非终态、目标 enabled、目标有决策权限、revision 匹配。
- 行为：
  - 写 `__assignment`：`owner=B`、`assignee=B`、`revision+1`、`transferred_from=A`。
  - 清空 `__delegation`。
  - 关闭 A 的旧 delivery，标记 `Superseded`。
  - 发送 `type=assignment` 消息给 B。
  - **不修改 task state，不 complete，不生成新 task。**
- `act:push` payload 增加 `assign`，`PushedAct` 持久化并传给新 child act。
- 决策授权在 transfer 后拒绝旧 owner。
- 并发：transfer 与 decision 只有一个赢家。

### 4.3 实现方式

优先复用现有 `ApplyAction` / `ActionKind` 机制，增加非终态的 `Transfer` 词汇：

- 在进入 terminal `ActionPlanner` 前分支处理 assignment plan。
- plan 只写变量 + outbox，不产生 state change / propagation。
- `__action` fact 持久化 transfer 参数和调用者，支持 replay。

如果现有 `TransitionPlan` 无法表达“无 state change”的 plan，则新增 `OperationKind::AssignTask`，但语义不变；不要把它做成 `act:complete` 的变体。

### 4.4 主要落点

- `crates/acts/src/event/action.rs`
- `crates/acts/src/scheduler/runtime.rs`
- `crates/acts/src/scheduler/core/commit/command/action.rs`
- `crates/acts/src/scheduler/shell/operation/action.rs`
- `crates/acts/src/export/executor/act_executor.rs`
- `crates/acts/src/export/actions.rs`
- `crates/acts/src/acl.rs`
- `crates/acts/src/store/data/delivery.rs`
- `crates/acts/src/store/delivery_rows.rs`
- `crates/acts/src/scheduler/shell/action.rs`（`PushedAct`）
- 相关测试。

### 4.5 验收

- transfer 后 task `tid/nid/generation/state` 不变；无新 task row。
- A 的旧 delivery `Superseded`；B 收到 `type=assignment`。
- A 后续 `act:complete` 被拒；B 可以完成。
- transfer 目标无权限/终态/revision 不匹配时拒绝。
- transfer 与 complete 并发时只有一个成功。
- `act:push` 新 act 的 `assign.owner` 生效，独立授权。
- 重放后 transfer 结果仍在。

### 4.6 验证

同 P1，另加转办/并发/重放相关测试。

## 5. P3：代办

### 5.1 目标

实现 per-task 代办，不引入 collection。

### 5.2 范围

- 新增 action：
  ```text
  act:delegate   {pid,tid,delegatee,expires_at,reason}
  act:undelegate {pid,tid}
  ```
- 授权：当前 `owner` 或 `assign` 能力。
- 写 `__delegation`：
  ```json
  {"id":"dg1","delegator":"u1","delegatee":"u2",
   "granted_by":"svc","expires_at":1761000000000,"reason":"休假"}
  ```
- `assignee=delegatee`，`revision+1`。
- owner 仍可决策；delegatee 在有效期内可决策。
- 决策记录 `owner`、`assignee`、`delegator`、`delegatee`。
- 撤销：`act:undelegate` 清空，`assignee=owner`，`revision+1`。
- 过期：决策时检查；可选清理变量，不作为授权依据。
- transfer 清空 delegation。
- 一人一 task 只保留一个有效 delegation。

### 5.3 主要落点

- `crates/acts/src/event/action.rs`
- `crates/acts/src/scheduler/core/commit/command/action.rs`
- `crates/acts/src/scheduler/shell/operation/action.rs`
- `crates/acts/src/export/executor/act_executor.rs`
- `crates/acts/src/export/actions.rs`
- `crates/acts/src/acl.rs`
- `crates/acts/src/scheduler/shell/facts.rs`
- message/delivery 投影字段补全。
- 相关测试。

### 5.4 验收

- delegate 后 owner A 和 delegatee B 都可决策。
- B 决策记录 `delegator=A`、`delegatee=B`、`assignee=B`。
- A 决策记录 `assignee=A`，不误记 delegation。
- `act:undelegate` 后 B 被拒。
- `expires_at` 后 B 被拒。
- transfer 清空 delegation，A 的 delegatee 不能操作 B 的任务。
- 无 collection、无 standing delegation。

### 5.5 验证

同 P1，另加 delegate/expiry/revoke/transfer 交叉测试。

## 6. P4：客户端、文档、端到端

### 6.1 目标

把 P1–P3 暴露给客户端，并完成用户文档与端到端验证。

### 6.2 范围

- `acts-channel` 增加：
  - `transfer(pid, tid, to, expect_rev, reason)`
  - `delegate(pid, tid, delegatee, expires_at, reason)`
  - `undelegate(pid, tid)`
- gRPC/NATS/web 共用 `actions::apply_as`，确认新 action 自动可用；补路由/SSE 测试。
- `doc/src` 用户文档：
  - `assign` 写法；
  - 转办/代办；
  - 或签/会签只使用 `join`；
  - 无 `candidates`。
- `examples/approve` 增加转办/代办演示（可选，但必须有端到端测试）。
- `ChannelOptions` 过滤示例：`options.owner` / `options.assignee`。

### 6.3 验收

- 客户端可调用 transfer/delegate/undelegate。
- HTTP/gRPC/NATS 至少各有一条 action 路径测试；不重复实现 dispatch。
- 文档示例可运行。
- 端到端：创建审批 → 转办 → B 完成；另一个流程：代办 → B 完成 → 审计字段正确。
- 全量 gate 通过。

## 7. 风险与回滚

| 风险 | 处理 |
| --- | --- |
| `__assignment` 进入 snapshot/codec，旧行不兼容 | 用 `#[serde(default)]` + upcast；task vars 是 JSON，不新增 TaskNode 必填字段 |
| `ActionKind` 扩展影响 replay | 新 variant 只由新代码写入；旧 action fact 不受影响 |
| 无 state change 的 plan 不受支持 | 回退新增 `OperationKind::AssignTask`，语义不变 |
| 旧 delivery 未关闭 | read-model 投影可异步；决策校验不依赖 delivery |
| ACL 关闭无法识别 operator | 保持旧行为，assignment 仅路由提示；文档明确 |
| 业务系统绕过 owner 权限 | `assign` 能力独立于 `@execute`；审计 `granted_by`/`updated_by` |

回滚策略：

- P1：移除 `assign` 解析与授权检查，保留消息字段（serde default）不影响旧数据。
- P2/P3：禁用新 action；已有 `__assignment`/`__delegation` 变量被忽略，task 仍可按 owner 或旧 `@execute` 处理。
- 不删除 message/delivery 字段，避免破坏历史行。

## 8. 提示词接力

### 8.1 模板

```text
# <阶段> 实现提示词

你是在 D:/private/github/workflow/acts 上的 coding agent。
只执行 <阶段>，不要提前实现下一阶段。

## 必读
- doc/design/assignment.md
- doc/plan/assignment.md
- AGENTS.md

## 前置
<上一阶段已完成的证据/遗留>

## 目标
<本阶段目标>

## 允许修改
<文件/模块>

## 禁止
<不在本阶段范围内的功能>

## 实现要求
<具体约束>

## 验收
<可观察结果>

## 验证
cargo fmt
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
<相关 cargo test>

## 交付
- 变更文件
- 实际执行的命令与结果
- 未跑套件及原因
- 遗留问题
完成后停止，不要继续下一阶段；下一阶段提示词由计划负责人生成。
```

### 8.2 当前提示词（P1）

```text
# P1 实现提示词：assignment 状态 + 决策授权

你是在 D:/private/github/workflow/acts 上的 coding agent。
只执行 P1，不要实现 transfer/delegate/undelegate。

## 必读
- doc/design/assignment.md
- doc/plan/assignment.md
- AGENTS.md

## 目标
1. human act 可以通过 assign.owner 声明承办人。
2. owner/assignee 持久化到 task 状态并投影到 message/delivery。
3. act 决策在 kernel 的 (pid,tid) 上做 per-task 授权。
4. 未声明 assign 的任务保持旧行为。

## 允许修改
- crates/acts/src/model/step.rs
- crates/acts/src/model/validation.rs
- crates/acts/src/scheduler/shell/facts.rs
- crates/acts/src/scheduler/shell/operation/task.rs
- crates/acts/src/scheduler/shell/mod.rs
- crates/acts/src/event/message.rs
- crates/acts/src/event/action.rs
- crates/acts/src/store/data/message.rs
- crates/acts/src/store/data/delivery.rs
- crates/acts/src/model/info.rs
- crates/acts/src/export/executor/act_executor.rs
- crates/acts/src/scheduler/runtime.rs
- crates/acts/src/scheduler/shell/operation/action.rs
- crates/acts/src/export/channel.rs
- crates/acts-channel/src/model/message.rs
- 上述变更点附近的现有测试文件

## 禁止
- 不实现 act:transfer / act:delegate / act:undelegate。
- 不引入 candidates、delegation collection、standing delegation。
- 不改变 join 的 all/any/partial 语义。
- 不通过 complete 原 task 或新建 task 实现转办。
- 不新增 ChannelOptions.mine。
- 不信任客户端提交的 owner/assignee/assignment_rev；服务端以认证主体和 task 状态为准。

## 实现要求
1. Step 增加可选 assign { owner: String }。
   - 只允许人类决策叶子 act（ActRunAs::Irq，例如 acts.core.irq / acts.core.decide / 自定义 Irq）。
   - 容器 acts.core.block / parallel / sequence 自身声明 assign 时，在 act start 报错。
   - Func / Msg 叶子声明 assign 时，在 act start 报错。
2. 新增私有 task 变量 __assignment：
   {"owner":"...","assignee":"...","revision":0,
    "transferred_from":null,"updated_by":null,"updated_at":0}
   - 在 act start 时用与 params 相同的表达式解析路径解析 assign.owner。
   - 解析为空或失败：act 失败，不产生无人审批。
   - 初始 assignee = owner，revision = 0。
3. 消息投影：
   - Message / data::Message / data::Delivery / MessageInfo / acts-channel::model::Message
     增加 owner、assignee、delegator、delegatee、assignment_rev、ref_msg_id。
   - act payload 的 inputs.options 写 owner、assignee，供现有 channel glob 使用。
   - schema 升级使用 serde default + upcast，旧行可读。
4. 决策授权：
   - Action 增加服务端填充的认证主体字段（例如 subject），由 ActExecutor 从 Principal 写入，
     绝不由客户端 options 提供。
   - kernel 在 apply/plan 阶段读取 __assignment：
     * 有 assign：只允许 owner 或 assignee 决策；否则返回 ENOTOWNER。
     * 决策携带 assignment_rev；不匹配返回 EASSIGNMENT_CHANGED。
     * 未声明 assign：保持现有 @execute 行为。
     * 规则自动决策（engine origin）跳过人员校验。
     * ACL 关闭 / 无认证主体：保持旧行为，assignment 仅路由提示。
   - Completed 消息带 owner、assignee、assignment_rev。
5. 不新增 act action；不实现转办/代办。

## 验收
- assign.owner 正常解析并持久化。
- assign.owner 为空时 act 失败。
- 非人类 act 声明 assign 时 start 失败，错误指明节点。
- 非 owner/assignee 的 act:complete 被拒；owner/assignee 成功。
- 未声明 assign 的任务旧行为不变。
- Completed 消息字段和 inputs.options 正确。
- 旧 message/delivery 行可读。
- ACL 关闭时旧行为不变。

## 验证
cargo fmt
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo test -p acts --lib
cargo test -p acts --doc
cargo test -p acts-channel --lib

如缺 NATS/Postgres 或资源不足，按 AGENTS.md 说明未跑套件，不得声称全绿。

## 交付
- 变更文件列表
- 实际执行的命令与结果
- 未跑套件及原因
- 设计偏差与遗留问题
完成后停止，不要继续 P2；下一步提示词由计划负责人生成。
```
