# 承办人、转办与代办

人类审批活动可以声明承办人（`owner`），并在运行期把承办关系转办给另一个业务用户，或让另一个业务用户代办。转办和代办都只改变“谁可以决策这个活动”，不改变活动实例、状态机或流程走向。

## 声明承办人

`assign.owner` 是模型层的参与人原语，**解析由参与人插件提供**：安装
`acts_plugin_participant::ParticipantPlugin` 后使用默认
`LocalParticipantResolver`，或用 `ParticipantPlugin::with_resolver` 替换。
没有安装插件（且未通过 `Engine::register_capability_provider` 广播能力、再用
`Engine::register_participant_resolver(key, ...)` 注册解析器）时，
发布含 `assign.owner` 的模型会直接失败并点名插件；活动启动路径也会 fail
closed。`__assignment`/`__delegation` 和决策授权仍在引擎内核。

`assign` 只允许写在人类决策叶子上（`ActRunAs::Irq` 的包，如 `acts.core.irq`、`acts.core.decide`，以及自定义 IRQ 包）。

```yml
- id: approve
  name: approve
  uses: acts.core.irq
  params:
    key: approve
  assign:
    owner: "${{ reviewer }}"
    provider: "acme.participant" # optional
```

`assign.provider` 选择解析该活动的参与人提供者：

- 省略时使用该能力类型唯一的已安装提供者；
- 注册了多个提供者时必须显式声明，否则发布失败；
- 发布时把 key 固化到存储的模型，活动启动和重启都使用该 key；
- 同一个 provider key 在任一时刻只有一个活动实现；`version` 只用于能力表诊断和
  版本校验，不支持同 key 多版本共驻。

规则：

- 表达式在活动启动时通过和 `params` 相同的表达式路径解析。
- 解析结果为空（或不是字符串）时活动失败，不会退化成“无人审批”。
- `assign` 不允许写在 `acts.core.block` / `acts.core.parallel` / `acts.core.sequence` 容器本身，也不允许写在 `Func`/`Msg` 活动上。
- 未声明 `assign` 的活动保持旧行为：拥有 `@execute` 的主体可以决策。

容器中的写法：每个叶子的 `assign` 写在 `params.steps[]` 的元素上，按该元素自己的作用域解析。

```yml
# block：每个叶子独立解析
- uses: acts.core.block
  params:
    mode: parallel
    steps:
      - uses: acts.core.irq
        assign:
          owner: "$root.finance_lead"
      - uses: acts.core.irq
        assign:
          owner: "$root.cfo"

# parallel：按 item 的 value 解析
- uses: acts.core.parallel
  params:
    in: "${{ reviewers }}"
    join: any
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ value }}"

# sequence：逐项解析
- uses: acts.core.sequence
  params:
    in: "${{ reviewers }}"
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ value }}"
```

嵌套容器中，`value` / `index` 位于 item 的祖先作用域，内层 `block` 的叶子也能读取；顶层 `block` 只能读 `$root`、`$parent` 和步骤变量。

## OwnerExpr（参与人表达式）

`assign.owner` 除了字符串，还接受结构化的 `OwnerExpr`。字符串形式保持原语义
（用户 id，或 `${{ }}` 表达式）；对象形式用 `kind` 区分：

```yml
assign:
  owner:
    kind: owners_of
    resource: { resource: department, name: 研发部 }
    duty: principal
```

| `kind` | 含义 |
| --- | --- |
| `user` / `role` / `group` | 静态用户、角色、组 |
| `owners_of` | 资源的负责人；`resource` 为 `project`/`department`/`unit`，`duty` 为 `all`（默认）/`principal`/`deputy`/`member` |
| `context` | 运行时上下文变量 |
| `relate` | 关系变换，如 `department_of`、`manager_of`；关系可写成字符串或 `{ rel, value }` |
| `any_of` / `all_of` / `except` | 并集 / 交集 / 差集 |

上下文变量：

```json
{ "kind": "context", "var": { "type": "initiator" } }
{ "kind": "context", "var": { "type": "current_actor" } }
{ "kind": "context", "var": { "type": "previous_approver" } }
{ "kind": "context", "var": { "type": "previous_approver_nth", "back": 1 } }
{ "kind": "context", "var": { "type": "var", "name": "owner_unit", "ty": "unit", "duty": "principal" } }
```

- `initiator`：流程发起人。
- `current_actor`：最近一次决策的操作人；还没有决策时等于发起人。
- `previous_approver`：当前操作人之前的那次决策；按决策次数计数，不按人去重。
- `var`：工作流变量。`ty` 为 `user`/`users`/`role`/`group`/`project`/
  `department`/`unit`；资源类型按 `duty` 查负责人。

表达式也可以写成 `$initiator`、`$actor`、`$previous_approver(n)`。

**一个活动只有一个 owner。** 表达式解析为空或多于一个参与人时活动失败：

```text
assign.owner resolved to 2 participants (u1, u2); a human act has exactly one
owner — use the acts.core.owner package with block/parallel/sequence for a
multi-approver step
```

多人审批用 `acts.core.owner` 先算参与人，再交给 `block`/`sequence`/
`parallel`：

```yml
- id: owners
  uses: acts.core.owner
  params:
    kind: any_of
    operands:
      - { kind: context, var: { type: current_actor } }
      - { kind: role, name: reviewer }

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

`acts.core.owner` 是 **IRQ 活动**：引擎把表达式作为请求发给业务系统，业务系统用
自己的用户目录（角色、组、资源负责人、关系）解析，完成活动并返回。请求发出前，
引擎会把表达式里的本地上下文原子（`initiator`、`current_actor`、
`previous_approver`、`user`/`users` 变量）替换成解析出的 `user` 节点，业务系统
只需要解析自己的目录数据：

```json
{
  "participants": [{ "id": "u1", "code": "alice", "name": "Alice" }],
  "ids": ["u1"],
  "count": 1
}
```

只需要 fan-out 时也可以只返回 `ids`，工作流用 `in: "${{ owners.ids }}"` 和
`assign.owner: "${{ value }}"`。引擎不维护业务目录，也没有 `[owner]` 配置；
业务系统是唯一的解析方。

`assign.owner` 只解析本地原子（用户、上下文变量、普通用户变量、集合运算）。
出现 `role`/`group`/`owners_of`/`relate` 时活动失败并提示先走
`acts.core.owner`：

```text
owner atom 'role' ('reviewer') names business data; resolve it through the
acts.core.owner IRQ package — pass this expression as the package's params,
let the business system return the participants, and assign from the returned ids
```

这样做是因为 `assign` 在活动启动时必须立即得到唯一 owner，不应在启动路径上做
业务 I/O。需要业务数据的单个 owner，先走 `acts.core.owner`，再取唯一结果：

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

## 承办关系

活动启动后，承办关系持久化在任务的私有变量 `__assignment` 中：

```json
{
  "owner": "u1",
  "assignee": "u1",
  "rev": 0,
  "transferred_from": null,
  "updated_by": null,
  "updated_at": 0
}
```

- `owner`：承办人。
- `assignee`：当前默认操作人；初始等于 `owner`，转办或代办后变化。
- `rev`：承办关系版本；转办、代办、撤销代办都会递增。
- `transferred_from` / `updated_by` / `updated_at`：审计信息。

决策记录投影到活动消息：

- `owner`、`assignee`、`delegator`、`delegatee`、`rev`、`ref_msg_id`。
- 同时写入 `inputs.options.owner` / `inputs.options.assignee`，供 `ChannelOptions` 的 glob 过滤使用。

`inputs.options` 只用于路由，**不是授权依据**；授权以任务的 `__assignment` / `__delegation` 状态为准。

## 转办

```text
act:transfer {pid,tid,to,expect_rev,reason}
```

- 允许者：当前 `owner`，或拥有 `assign` 能力的主体（`@assign` 或显式的 `act:transfer` 授权）。
- `to` 是目标**业务用户**，不是 ACL 用户；引擎只校验它非空。
- `expect_rev` 可选；与当前 `rev` 不匹配时返回 `EASSIGNMENT_CHANGED`。
- 成功后 `owner` 和 `assignee` 都变为目标用户，`rev + 1`，旧承办人的待办投递标记为 `Superseded`。
- 不修改活动状态、不 complete、不生成新活动。
- 转办会清空当前活动的代办：旧承办人的代理人不能继承新承办人的权限。

消息序列：

```text
act Created       (owner=A)
assignment        (owner=B)      <- 转办通知
act Completed     (assignee=B)   <- B 决策后
```

原来的 `Created` 消息保留为历史；`assignment` 消息通过 `ref_msg_id` 指向它。

## 代办

```text
act:delegate   {pid,tid,delegatee,expires_at,reason}
act:undelegate {pid,tid}
```

- 允许者：当前 `owner`，或拥有 `assign` 能力的主体。
- `delegatee` 必填且非空，是业务用户。
- `expires_at` 可选，Unix 毫秒时间戳；到期后代理人立即失效，不依赖后台清理任务。
- `reason` 可选，仅用于审计和通知。

`act:delegate` 写入任务的 `__delegation`：

```json
{
  "id": "dg1",
  "delegator": "u1",
  "delegatee": "u2",
  "granted_by": "svc-approval",
  "expires_at": 1761000000000,
  "reason": "休假"
}
```

- `delegator` 是设置时的 `owner`，由引擎推断，不从请求体读取。
- `granted_by` 是服务端认证后的调用主体。
- `owner` 不变，`assignee` 变为 `delegatee`，`rev + 1`。
- 一个活动同时只有一个有效代办；新的 `act:delegate` 替换旧记录。

`act:undelegate` 清空 `__delegation`，`assignee` 恢复为 `owner`，`rev + 1`。

## 决策授权

| 场景 | owner | assignee | delegation | 谁可以决策 |
| --- | --- | --- | --- | --- |
| 正常 | A | A | 无 | A |
| 转办 | B | B | 清空 | B |
| 代办 | A | B | `{delegator:A, delegatee:B}` | A 或 B（有效期内） |

- `owner` 始终可以决策。
- `delegatee` 只在有效期内可以决策；过期、撤销或转办后拒绝，错误信息会点名活动和过期事实。
- 决策可携带 `rev`；与当前承办版本不一致时返回 `EASSIGNMENT_CHANGED`。
- 引擎规则触发的自动决策没有认证主体，跳过人员校验。

决策记录字段：

```json
// owner 决策
{ "owner": "u1", "assignee": "u1" }

// delegatee 决策
{
  "owner": "u1",
  "assignee": "u2",
  "delegator": "u1",
  "delegatee": "u2"
}
```

`rev` 是本次决策对应的承办版本。

## 或签 / 会签

多人审批只使用 `join` 语义，不引入 `candidates` 候选池：

```yml
- uses: acts.core.parallel
  params:
    in: "${{ approvers }}"
    join: any          # 或签：第一票通过即撤销其余票
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ value }}"
```

- `join: any`：或签，任一活动完成即可。
- `join: all`：会签，全部活动完成才继续。
- `join: partial(n)`：达到阈值即通过。

每个审批人是一个独立活动，拥有独立的 `owner`、消息和投递；不要用候选池表达多人审批。

加签/减签仍使用 `act:push` / `act:remove`；新加的 IRQ 叶子可以在 payload 中携带自己的 `assign`。

## 业务用户与 ACL 用户

这是两套不同的概念：

- **ACL 用户**：管理用户，决定谁可以调用哪些命令（`@read`、`@execute`、`@assign` 等）。
- **业务用户**：审批人、承办人、代理人（`owner` / `assignee` / `delegator` / `delegatee`）。引擎不实现业务用户目录。

因此：

- `assign` 能力独立于 `@execute`：能执行流程不代表能转办别人的审批。
- 目标业务用户的 `enabled`、岗位权限、业务规则由业务系统校验；引擎只校验目标非空、活动存在、活动非终态和 `rev`。
- 客户端提交的 `owner` / `assignee` / `delegator` / `delegatee` / `granted_by` / `subject` / `rev` 不被信任；主体由服务端认证后写入，人员字段以任务状态为准。
- `snapshot` / `profile` 是业务数据，不是授权源。

## 客户端

`acts-channel` 提供：

```rust
client.transfer(&pid, &tid, "u2", Some(rev), Some("出差")).await?;
client.delegate(&pid, &tid, "u2", Some(deadline_ms), Some("休假")).await?;
client.undelegate(&pid, &tid).await?;
```

原始 action 名称：

```text
act:transfer   {pid,tid,to,expect_rev,reason}
act:delegate   {pid,tid,delegatee,expires_at,reason}
act:undelegate {pid,tid}
```

## Channel 过滤

消息投影可直接用于 `ChannelOptions.options` 的 glob：

```rust
// 只看 alice 的待办
let options = Vars::new().with("owner", "alice");
// 只看当前操作人是 bob 的消息
let options = Vars::new().with("assignee", "bob");
```

转办后旧投递被标记 `Superseded`；代办的 `assignment` 通知会同时带 `delegator` / `delegatee`，便于订阅方区分“本人操作”和“代办操作”。
