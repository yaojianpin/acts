# 审批承办人、候选人与转办代办设计

- 状态：定稿（2026-10-29 增补候选人与 caps 化；transfer/delegate 语义不变）
- 日期：2026-10-27（2026-10-29 更新）
- 范围：`crates/acts`、`crates/acts-channel`、`plugins/acts-plugin-web|grpc|nats`
- 关联：`design/caps.md`（能力模型）、`design/owner-expr.md`（CandidateExpr）

## 1. 目标

在不改变现有 channel 投递模型的前提下，为人类审批 act 增加一套可持久化、可授权、可审计的承办关系，并支持：

- 转办：承办人从 A 变为 B，任务实例不变。
- 代办：承办人仍是 A，代理人 B 可以代表 A 操作。
- 加签/减签：复用 `act:push` / `act:remove`，新 act 自带承办人。
- 或签/会签：完全复用现有 `join` 语义，不引入第二套多人审批标准。

### 非目标

- ~~不引入 `candidates` 候选池~~（2026-10-29 起引入候选池，见 §7/§8 与
  `design/caps.md`；多人审批的"人从哪来"由 candidates 回答，"怎么算完成"
  仍由 `join` 回答）。
- 不引入 standing delegation 或 `delegations` collection。
- 不允许转办通过 complete 原 task、再建新 task 实现。
- 不在引擎内实现用户目录、角色、用户组。
- 不改变 `join` 的 `all` / `any` / `partial` / delegated action 语义。

## 2. 术语

| 中文 | 字段 | 位置 | 含义 |
| --- | --- | --- | --- |
| 承办人 | `owner` | task 状态 | 任务责任人；转办时变更 |
| 操作人 | `assignee` | task / decision | 当前实际操作人；默认等于 owner |
| 代理人 | `delegatee` | delegation / decision | 受托代办的人 |
| 委托人 | `delegator` | delegation / decision | 授权代办的承办人 |
| 候选人 | `candidates` | act 变量（列表） | `assign.candidates` 声明、`uses` 包计算出的候选池，供容器展开多人审批 |

不再使用：

- `on_behalf_of`：改名为 `delegator`。
- `actor`：决策时 `assignee` 就是实际操作人；管理动作的调用者用 `by` / `granted_by` 记录。

## 3. 现状约束

- `data::Task` 没有承办人字段，task 的变量存在 `ProcessSnapshot.variables` / `TaskVars`。
- `Message` 只有 `tid/nid/pid/state/inputs/outputs`；`inputs` 携带 `params`、`options`、step vars。
- `ChannelOptions` 按 `type/state/uses/options` 做 glob 匹配；每个匹配 channel 得到一条 delivery。
- `act:complete` 当前只做全局 ACL 检查：`ActExecutor::do_action` 调用 `principal.check(name)`，没有 `(pid,tid)` 级校验。
- `ScopePolicy.subject` 是流程发起人，不是审批承办人。
- 决策按进程 aggregate 串行提交，task 变量随 snapshot 持久化、replay。

关键位置：

- `crates/acts/src/event/message.rs`
- `crates/acts/src/export/channel.rs`
- `crates/acts/src/export/executor/act_executor.rs`
- `crates/acts/src/scheduler/shell/facts.rs`
- `crates/acts/src/scheduler/shell/mod.rs`
- `crates/acts/src/acl.rs`
- `crates/acts/src/scheduler/core/model/operation.rs`

## 4. 核心不变量

1. **一个人类 act 只有一个 owner。** 多人审批通过 `parallel` / `block` 展开成多个 act。
2. **assignment 是 task 实例状态，不是 message 状态。** message 只携带投影。
3. **转办不改变 task 状态机。** 不 complete、不生成新 task、不推进工作流。
4. **代办是 per-task 的。** 通过 `act:delegate` 设置，`act:undelegate` 或 `expires_at` 撤销，不建 collection。
5. **`join` 是唯一的多人审批语义。** `any` 或签，`all` 会签，`partial` 阈值。
6. **决策授权在 kernel 的 `(pid,tid)` 上执行。** 不能只靠 channel 过滤或全局 `@execute`。

## 5. 数据模型

### 5.1 Task 状态

在 task scope 增加私有变量：

```json
"__assignment": {
  "owner": "u1",
  "assignee": "u1",
  "revision": 0,
  "transferred_from": null,
  "updated_by": null,
  "updated_at": 1760000000000
}
```

规则：

- `owner`：承办人；`assign` 声明或转办设置。
- `assignee`：当前默认操作人；初始等于 `owner`。
- `revision`：assignment 版本；transfer / delegate / undelegate 递增。
- `transferred_from`：上一次转办的 owner，便于审计。
- `updated_by`：执行 assignment 变更的调用者。
- 私有 `__` 前缀：不进入 `$data()` / outputs / message `inputs`，由消息投影显式读取。

### 5.2 Delegation

同一 task 只保留一个有效 delegation：

```json
"__delegation": {
  "id": "dg1",
  "delegator": "u1",
  "delegatee": "u2",
  "granted_by": "svc-approval",
  "expires_at": 1761000000000,
  "reason": "休假"
}
```

规则：

- `delegator` 等于设置时的 `owner`，由引擎推断，不从请求体信任。
- `granted_by` 是实际调用者（业务系统或 owner）。
- `expires_at` 可选；到期后 delegation 失效，决策被拒。
- 新 `act:delegate` 替换旧 delegation；`act:undelegate` 清除。
- 转办时清空 `__delegation`，避免 A 的代理人继承 B 的权限。

### 5.3 Message / Delivery 投影

`Message`、`data::Message`、`data::Delivery`、`MessageInfo`、`acts-channel::model::Message` 增加：

```rust
owner: Option<String>,
assignee: Option<String>,
delegator: Option<String>,
delegatee: Option<String>,
assignment_rev: u64,
ref_msg_id: Option<String>,
```

同时写：

```json
"inputs": {
  "options": {
    "owner": "u1",
    "assignee": "u1"
  }
}
```

`options` 只用于现有 channel glob 路由，**不是授权依据**。

### 5.4 模型声明

```yaml
- id: approve
  uses: acts.core.irq
  assign:
    owner: "${{ reviewer }}"        # 责任人：字符串（用户 id 或 ${{ }} 表达式），可选
    candidates:                     # 候选池：{ params, uses }，可选
      params: { kind: role, name: reviewers }
      uses: acts.core.users    # 默认值；可指向自己的计算包
```

规则：

- `assign.owner` 只允许出现在人类决策叶子 act 上：`ActRunAs::Irq`；它是一个
  字符串（用户 id 或 `${{ }}` 表达式），由内核本地解析；解析为空（或多人）时
  fail closed，任务失败，不退化成无人审批。
- `assign.candidates` 声明在需要多人展开的 act 上（容器
  `block`/`parallel`/`sequence` 或人类叶）；`params` 是 CandidateExpr。
  act start 时先检查表达式能否**本地直接计算**——全部为本地原子时内核直接算出
  候选池，不发起请求；否则以 pushed act 的形式请求 `uses` 包（模式同 join 的
  action 请求），应答写入 act 变量。两条路发给 `uses` 包的参数都先把本地变量
  参数（上下文原子、`${{ }}` 字符串）计算好。
- `assignee` 不在模型中声明；初始为 `owner`，由 transfer/delegate 改变。
- 代办不是模型配置：`act:delegate`/`act:undelegate` 运行时移动。
- 未声明 `assign` 的任务保持旧行为：`@execute` 可决策。

## 6. 语义

| 场景 | owner | assignee | delegation | 谁可决策 |
| --- | --- | --- | --- | --- |
| 正常 | A | A | 无 | A |
| 转办 | B | B | 清空 | B |
| 代办 | A | B | `{delegator:A, delegatee:B}` | A 或 B |

### 6.1 转办

`act:transfer {pid,tid,to,expect_rev,reason}`：

1. 校验调用者是当前 `owner`，或拥有 `assign` 能力。
2. 校验 task 非终态、`expect_rev` 匹配、目标用户 enabled 且有决策权限。
3. 写 `__assignment`：`owner=B`、`assignee=B`、`revision+1`、`transferred_from=A`。
4. 清空 `__delegation`。
5. 关闭 A 的旧 delivery，标记 `Superseded`。
6. 发送一条 `type=assignment` 消息给 B。
7. **不修改 task state，不 complete，不生成新 task。**

消息序列：

```text
act Created (owner=A)
assignment (owner=B)       <- 转办
act Completed (assignee=B) <- B 决策后
```

原 act Created message 保留为历史，不改成 Completed。

### 6.2 代办

`act:delegate {pid,tid,delegatee,expires_at,reason}`：

1. 校验调用者是当前 `owner`，或拥有 `assign` 能力。
2. `delegator` = 当前 `owner`；`granted_by` = 调用者。
3. 写 `__delegation`，`assignee=delegatee`，`revision+1`。
4. 发送 `type=assignment` 通知给 delegatee。
5. owner 仍可决策；delegatee 在有效期内可决策。
6. 决策记录 `owner`、`assignee`（本次实际操作人）、`delegator`、`delegatee`。

撤销：

- `act:undelegate {pid,tid}`：清空 `__delegation`，`assignee=owner`，`revision+1`。
- 到期：决策时检查 `now < expires_at`；可选清理变量，但不是授权依据。
- 转办：清空 `__delegation`。

### 6.3 决策记录

正常：

```json
{ "owner": "u1", "assignee": "u1" }
```

代办：

```json
{
  "owner": "u1",
  "assignee": "u2",
  "delegator": "u1",
  "delegatee": "u2"
}
```

owner 自己操作时：

```json
{ "owner": "u1", "assignee": "u1" }
```

## 7. 容器中的 assign

`assign.owner` 可以写在叶子人类 act 上（每个叶子自己的 start 时解析），也可以
写在容器上——**容器的声明是子步骤的模板**：容器自身不产生 `__assignment`（没有
单 owner），不受决策授权约束；它生成的子活动里没有单独声明 `assign.owner` 的，
在各自启动时继承最近的祖先容器的 owner 表达式，并在子活动自己的作用域里解析
（`$item.value`、`$initiator`、工作流变量都可用）；自己声明了 owner 的子活动
用自己的。自完成的活动（`Func`/`Msg`）不带承办关系，继承对它们跳过。容器自身
还可以声明 `assign.candidates`：容器 start 时先计算候选池（pushed act 请求
uses 包），应答变量就位后容器才展开 fan-out，`params` 的模板（如
`in: "${{ candidates }}"`）由此能读到候选人并生成子 step；命名池
（`assign.<key>`）同理，应答落在 `$assign.<key>.*` 下。池的声明也沿模型树
继承：声明了 assign 的子活动，缺失的池 key 从最近的祖先容器补齐并在自己的
作用域里计算；没有声明 assign 的子活动（容器 fan-out 生成的子活动）沿作用域
链直接读到祖先已算出的池，不重复计算。

### 7.1 `acts.core.block`

```yaml
- id: finance
  uses: acts.core.block
  params:
    mode: parallel
    steps:
      - uses: acts.core.irq
        assign:
          owner: "$root.finance_lead"
      - uses: acts.core.irq
        assign:
          owner: "$root.cfo"
```

`mode: sequence` / `parallel` 不改变解析规则；每个 inline act 在自己的 start 时按 scope 解析。

### 7.2 `acts.core.parallel`

```yaml
- id: reviewers
  uses: acts.core.parallel
  params:
    in: "${{ reviewers }}"
    join: any
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
```

每个 item 的 `$item.value` / `$item.index` 在 item scope 中；叶子 act 按 item 解析 `owner`。

### 7.3 `acts.core.sequence`

```yaml
- id: sequential_review
  uses: acts.core.sequence
  params:
    in: "${{ reviewers }}"
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
```

逐项执行，每项解析自己的 `owner`。

### 7.4 嵌套

`parallel` / `sequence` 的 `$item.value` / `$item.index` 在 item 的祖先 scope 中，嵌套在其中的 `block` 叶子也能读取。顶层 `block` 没有 item 绑定，只能读 `$root`、`$parent`、step vars。

### 7.5 加签/减签

加签与减签仍是 `act:push` / `act:remove`，由一段统一的授权和两种形态组成。

**授权（两种形态一致）**：加签/减签由"持有该活动的被指派执行"的 `owner`
决定——被推活动（普通 pushed act、顺签链的叶子）从不为自己签名，其授权在
最近的带 `__assignment` 的祖先上；非 pushed 的目标从自身起沿 parent 链找
最近的 assignment。`subject` 不是该 owner 时内核以 `ENOTOWNER` 拒绝。整条
链上都没有 assignment 的活动（模型声明的无主容器、未声明 `assign` 的旧
任务）保持旧行为（`@execute`）。委托代办人（delegatee）不获得加签/减签权：
决策可以代办，审批人结构不能。

**单 owner 节点（顺签加签）**：目标 human leaf 只有一个 owner 时，
`act:push` payload 携带：

```json
{ "owners": ["u2", "u3"], "mode": "before", "params": { } }
```

内核在目标下动态生成一条顺签链（行为等价 `acts.core.sequence`：逐个准入、
顺序固定）。运行中的模型不可变、pushed 容器无法参数展开，所以顺签由内核
排序的 pushed 活动链实现：每个 `<target>#push<n>` 是一个普通 pushed
human leaf，`assign.owner` 为链上对应用户；目标 scope 的 `__sign` 队列
持有剩余顺序，前一个叶子落定（完成或被减签）后内核准入下一个。`mode`
决定链序：

- 前加签 `before`：`[u2, u3, …, <当前 owner>]` —— 增加的 step 在前，
  当前 owner 的 step 在后。
- 后加签 `after`（默认）：`[<当前 owner>, u2, u3, …]` —— 当前 owner 的
  step 在前，增加的 step 在后。

push 命令同时吸收目标自身的待答（目标 park，其 pending 的 act 消息就地
了结）：目标不再单独等自己的 client，owner 在顺签链的 owner 叶子上答复。
对链上一个叶子的 `act:remove` 把它决定 `Removed`，内核准入下一个；队列
排空后由普通 join 完成目标。

**会签节点（旧形态）**：目标下已是 block/parallel/sequence 多人展开（或
已挂 pushed act）时，`act:push` 保持原形态——client 点名活动（`uses`/
`params`/`assign`），内核生成 `<target>#push<n>` 子活动，目标 park 等它
落定，join 按现有语义重算。授权不变：只有 owner 可加签或减签。

**减签**：`act:remove` 只能由上述 owner 发起；被移除的活动决定
`Removed`，join 按 `Vote::Withdrawn` 重算。直接减掉一个顶层等待活动同样
owner-only。

## 8. 或签/会签

"人从哪来"由 candidates 回答，"怎么算完成"由 `join` 回答：

```yaml
- id: approvals
  uses: acts.core.parallel
  assign:
    candidates:
      params: { kind: role, name: reviewers }
      uses: acts.core.users
  params:
    in: "${{ candidates }}"     # 计算后的候选池（或 ids）
    join: any                   # 或签；all 会签；partial(2) 阈值
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
```

- 每个审批人一个独立 act、独立 message/delivery、独立 owner；第一票完成时
  `join: any` 撤销其余票。
- `join` 的 delegated 规则（`{ params, uses }`，默认 `acts.core.decide`）
  适合会签的"主持人裁决"形态：客户端拿 `{ params, outcomes }` census 自行
  判定 complete/fail/skip。
- 加签/减签在展开后的 children 上用 `act:push`（payload 带 `assign`）与
  `act:remove` 表达；父容器的 join 按现有语义重算。
- 顺签：`acts.core.sequence` + `in: "${{ candidates }}"`；减签也可通过前序
  步骤改写 candidates 变量后重新展开。

## 9. 授权

| 操作 | 允许者 |
| --- | --- |
| `act:complete/submit/back/cancel/abort/skip/error` | `owner`；`assignee`；有效 `delegatee`；显式 override |
| `act:transfer` | 当前 `owner`；`assign` 能力 |
| `act:delegate` / `act:undelegate` | 当前 `owner`；`assign` 能力 |
| `act:push` / `act:remove` | 目标（或沿 parent 链最近祖先）assignment 的 `owner`（见 §7.5）；无 assignment 的活动保持 `@execute` |
| `act:remind` | 除当前 owner 外的认证调用者（`@execute` 组） |
| `msg:sub` / `msg:ack` | 现有 grant；可选 `mine` 过滤 |
| 未声明 `assign` 的任务 | 保持旧行为：`@execute` |

要点：

- `assign` 能力独立于 `@execute`，避免所有能执行流程的人都能转办别人的任务。
- 决策请求携带 `assignment_rev`；kernel 在 `(pid,tid)` 上校验，不匹配返回 `EASSIGNMENT_CHANGED`。
- 认证主体由服务端写入 `assignee`；客户端提交的同名字段必须被覆盖或拒绝。
- 规则自动审批（`ValidationRule` 的 `next`）走 engine origin，跳过人员校验。
- 转办/代办目标必须是 enabled 用户；目标无决策权限时在 transfer 阶段拒绝。

## 10. 并发与一致性

- assignment 在 aggregate snapshot 中，transfer / delegate / decision 都在同一 `(pid,tid)` 上串行提交。
- transfer 携带 `expect_rev`；task 终态或在途决策时拒绝。
- decision 携带 `assignment_rev`；transfer 先提交则旧 actor 被拒，decision 先提交则 transfer 被拒。
- delegation 撤销同样递增 revision；撤销先提交则旧决策被拒。
- replay/restart 从 snapshot 恢复 `__assignment` / `__delegation`，不依赖内存。
- 旧 delivery 的 `Superseded` 是 read-model 投影；决策校验不依赖 delivery 状态。

## 11. API / 消息

新增 action：

```text
act:transfer   {pid,tid,to,expect_rev,reason}
act:delegate   {pid,tid,delegatee,expires_at,reason}
act:undelegate {pid,tid}
act:remind     {pid,tid,message?}
```

`act:push` payload 增加：

```json
"assign": { "owner": "u2" }
```

顺签加签（单 owner 节点）另支持：

```json
{ "owners": ["u2"], "mode": "before|after", "params": { } }
```

消息：

- `type=assignment`：转办/代办通知，新的 `msg_id`，同 `tid/nid`，带 `owner/assignee/delegator/delegatee/assignment_rev/ref_msg_id`。
- `type=remind`：催办记录，同 `tid/nid`，`owner/assignee/rev` 是当前 assignment，`inputs.from` 是催办人；`ref_msg_id` 指向原 `Created`。已转办的任务再向转办人投递一个动态 `acts.core.msg` 子活动（`<tid>#remind<n>`）。
- 原 `type=step`（act payload，曾写作 `act`）的 Created message：保留；转办时旧 delivery `Superseded`；代办时 owner 的 delivery 保留。
- `proc:graph` 按 `tid` 把消息折叠到节点（`GraphNode.messages`），assignment/remind/act 消息都在内。

Channel：

- `options.owner` / `options.assignee` 可被现有 glob 使用。
- 服务端 `mine` 过滤是后续增强，不作为授权依据。

## 12. 验收

| 场景 | 期望 |
| --- | --- |
| 正常 assigned task | 只有 owner/assignee 可决策 |
| 非 owner 决策 | 拒绝 `ENOTOWNER` |
| 未声明 assign | 旧 `@execute` 行为 |
| 转办 | task `tid` 不变、状态不变、无新 task；A 被拒，B 可决策 |
| 转办消息 | 旧 delivery `Superseded`；B 收到 assignment 消息 |
| 代办 | owner A 仍可决策；delegatee B 可决策；记录 delegator/delegatee |
| 代办撤销 | `act:undelegate` 后 B 被拒 |
| 代办过期 | `expires_at` 后 B 被拒 |
| 转办清空代办 | A 的 delegatee 不能操作 B 的任务 |
| 或签 | `parallel` + `join:any`，第一票完成，其余撤销 |
| 会签 | `join:all`，所有 act 完成才继续 |
| 候选池计算 | `assign.candidates` 的 uses 包应答写入变量；失败则 act 失败 |
| 业务原子经候选池 | `role`/`group` 等原子由 `acts.core.users` 发给业务系统解析 |
| 加签（会签形态） | `act:push` + assign，新 act 独立 owner |
| 加签（单 owner 前加签） | `act:push{owners,mode:"before"}` 生成 `acts.core.sequence`，先新增审批人后 owner |
| 加签（单 owner 后加签） | `mode:"after"`（默认）先 owner 后新增审批人 |
| 加签/减签授权 | 非 owner `ENOTOWNER`；无 assignment 的活动保持旧行为 |
| 减签 | `act:remove` 删除 child，join 重算，仅 owner 可发起 |
| 催办 | 非 owner `act:remind` 写 `type=remind` 记录；owner 自催被拒 |
| 催办（已转办） | 另生成 `acts.core.msg` 子活动投递转办人 |
| graph 折叠 | `proc:graph` 每节点携带自己的消息（含 msg 与 remind 记录） |
| 并发 | transfer 与 decision 只有一个赢家 |
| 重放 | restart 后 assignment/delegation 仍生效 |
