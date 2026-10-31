# 承办人、候选人与转办代办

人类审批活动可以声明承办人（`owner`），在运行期把承办关系转办给另一个业务用户，
或让另一个业务用户代办；多人审批的候选池由 `assign.candidates` 声明。转办和代办
都只改变“谁可以决策这个活动”，不改变活动实例、状态机或流程走向。

## 声明承办人

`assign.owner` 是模型层的责任人原语，**由内核本地解析**：一个字符串——用户 id，
或 `${{ }}` 表达式（`$initiator` / `$actor` / `$previous_approver(n)`、工作流
变量等，与 `params` 同一条表达式路径）。`__assignment`/`__delegation` 和
决策授权都在引擎内核；没有插件、没有 provider 配置。

`assign.owner` 只允许写在人类决策叶子上（`ActRunAs::Irq` 的包，如
`acts.core.irq`、`acts.core.decide`，以及自定义 IRQ 包）。

```yml
- id: approve
  name: approve
  uses: acts.core.irq
  params:
    key: approve
  assign:
    owner: "${{ reviewer }}"
```


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

# parallel：按 item 的 $item.value 解析
- uses: acts.core.parallel
  params:
    in: "${{ reviewers }}"
    join: any
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"

# sequence：逐项解析
- uses: acts.core.sequence
  params:
    in: "${{ reviewers }}"
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
```

嵌套容器中，`$item.value` / `$item.index` 位于 item 的祖先作用域，内层 `block` 的叶子也能读取；顶层 `block` 没有 item 绑定，只能读 `$root`、`$parent` 和步骤变量。

## CandidateExpr（候选人表达式）

`assign.candidates.params` 是一个结构化的 `CandidateExpr` 文档（也接受字符串
形式——用户 id 或 `${{ }}` 表达式），对象形式用 `kind` 区分：

```yml
assign:
  candidates:
    params:
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

- `initiator`：流程发起人：启动参数里的业务变量 `actor`；未传时为空。不是 ACL 用户。
- `current_actor`：最近一次决策的业务 actor（`act:*` 请求的 `actor` option）；还没有决策时等于发起人。
- `previous_approver`：当前操作人之前的那次决策；按决策次数计数，不按人去重。
- `var`：工作流变量。`ty` 为 `user`/`users`/`role`/`group`/`project`/
  `department`/`unit`；资源类型按 `duty` 查负责人。

表达式也可以写成 `$initiator`、`$actor`、`$previous_approver(n)`。它们的值来自
**业务变量 `actor`**（启动参数或 `act:*` 的 options），与 ACL 用户无关。

**一个活动只有一个 owner。** 表达式解析为空或多于一个参与人时活动失败：

```text
assign.owner resolved to 2 participants (u1, u2); a human act has exactly one
owner — declare assign.candidates and fan out with block/parallel/sequence for
a multi-approver step
```

多人审批用 `assign.candidates` 声明候选池，`uses` 包（默认
`acts.core.candidates`）计算，容器再按结果展开：

```yml
- id: approve
  uses: acts.core.parallel
  assign:
    candidates:
      params:
        kind: any_of
        operands:
          - { kind: context, var: { type: current_actor } }
          - { kind: role, name: reviewer }
      uses: acts.core.candidates   # 默认值，可省略；可指向自己的包
  params:
    in: "${{ candidates }}"
    join: any
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ value.id }}"
```

`acts.core.candidates` 是 **IRQ 活动**：引擎把表达式作为请求发给业务系统，业务
系统用自己的用户目录（角色、组、资源负责人、关系）解析，完成活动并返回。请求
发出前，表达式里引擎能算的部分先在本地算好——上下文原子（`initiator`、
`current_actor`、`previous_approver`、`user`/`users` 变量）替换成解析出的
`user` 节点，`${{ }}` 字符串替换成解析出的用户——业务系统只需要解析自己的目录
数据：

```json
{
  "participants": [{ "id": "u1", "code": "alice", "name": "Alice" }],
  "ids": ["u1"],
  "count": 1
}
```

只需要 fan-out 时也可以只返回 `ids`，工作流用 `in: "${{ candidates }}"` 和
`assign.owner: "${{ $item.value }}"`。引擎不维护业务目录，也没有 `[owner]`
配置；业务系统是唯一的解析方。

**候选池的计算顺序：先本地，后 uses。** 活动启动时先检查 `params` 是否可以
直接计算——表达式全部为本地原子（用户、上下文变量、用户变量、集合运算、
`${{ }}` 字符串）时，内核直接算出候选池，不发起任何请求；含业务原子时通过
`uses` 包计算。全本地但解析失败（未知变量等）时活动启动失败，不会伪造空池，
也不会把引擎已知不存在的值发给业务系统。无论哪条路，发送给 `uses` 包的参数
里，`Candidates` 的变量参数都已在本地计算完成。

`owner` 是字符串，表达不了业务原子（`role`/`group`/`owners_of`/`relate`）。
需要业务数据的单个 owner，先在候选池里算出参与人，再取唯一结果：

```yml
- id: owner
  uses: acts.core.irq
  assign:
    candidates:
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

不走 `assign.candidates` 也可以直接把池计算做成一个普通步骤：
`uses: acts.core.candidates` + `params`（CandidateExpr 文档）。这是一个普通
IRQ 活动——参数在本地准备好（`${{ }}` 与上下文原子已解析）后发给业务系统，
应答作为活动自己的输出落地，引擎不做本地折叠、也没有 `__candidates` 标记：

```yml
- id: pool
  uses: acts.core.candidates
  params:
    kind: role
    name: reviewers
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

## 读取承办关系（`$assign`）

表达式可以只读地读取当前执行及其祖先的承办事实：

| 字段 | 来源 |
| --- | --- |
| `$assign.owner` / `$assign.assignee` | 最近一个持 `__assignment` 的执行 |
| `$assign.delegator` / `$assign.delegatee` | 该执行自己的 `__delegation`；无有效代办为 `null` |
| `$assign.candidates` / `$assign.ids` | 最近一个已算出候选池（`__candidates`）的执行的 ids 列表 |
| `$assign.participants` / `$assign.count` | 同一池的参与人对象与数量 |
| `$assign.at("<step-id>")` | 显式指定执行：node id 取该节点最新 generation，也接受 wire `tid`；只读该执行自身的事实 |

身份字段与候选池**各自**沿 scope 链就近解析：容器生成的子活动
（`<id>#item` / `<id>#act{i}`）通常有自己的 `owner`，`$assign.candidates`
仍命中生成它的容器的池。

```yml
- id: approve
  uses: acts.core.parallel
  assign:
    candidates:
      params: { kind: role, name: reviewers }
  params:
    in: "${{ $assign.candidates }}"     # 本容器算出的池
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
        params:
          peers: "${{ $assign.candidates }}"            # 父容器的池
          peers_explicit: "${{ $assign.at('approve').ids }}"
```

没有 assign 事实时读取不会报错：身份字段为 `null`，`candidates`/`ids`/
`participants` 为空列表，`count` 为 `null`。表达式语言的对象不可调用，
显式形式因此写作 `$assign.at("<step-id>")`。

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

"人从哪来"由 `assign.candidates` 回答，"怎么算完成"由 `join` 回答：

```yml
- uses: acts.core.parallel
  assign:
    candidates:
      params: { kind: role, name: reviewers }
  params:
    in: "${{ candidates }}"
    join: any          # 或签：第一票通过即撤销其余票
    steps:
      - uses: acts.core.irq
        assign:
          owner: "${{ $item.value }}"
```

- `join: any`：或签，任一活动完成即可。
- `join: all`：会签，全部活动完成才继续。
- `join: partial(n)`：达到阈值即通过。
- `join: { params, uses }`：委托给 `uses` 包裁决（默认 `acts.core.decide`），
  适合主持人裁定的会签形态。

每个审批人是一个独立活动，拥有独立的 `owner`、消息和投递。顺签用
`acts.core.sequence` + `in: "${{ candidates }}"`；加签/减签仍使用
`act:push` / `act:remove`，新加的 IRQ 叶子在 payload 中携带自己的 `assign`。

## 业务用户与 ACL 用户

这是两套不同的概念：

- **ACL 用户**：管理用户，决定谁可以调用哪些命令（`@read`、`@execute`、`@assign` 等）。
- **业务用户**：审批人、承办人、代理人（`owner` / `assignee` / `delegator` / `delegatee`）。引擎不实现业务用户目录。

因此：

- `assign` 能力独立于 `@execute`：能执行流程不代表能转办别人的审批。
- 目标业务用户的 `enabled`、岗位权限、业务规则由业务系统校验；引擎只校验目标非空、活动存在、活动非终态和 `rev`。
- 客户端提交的 `owner` / `assignee` / `delegator` / `delegatee` / `granted_by` / `subject` / `rev` 不被信任；主体由服务端认证后写入，人员字段以任务状态为准。
- 业务 `actor`（启动参数里的 `actor` 与 `act:*` 的 `actor` option）是业务归属，驱动 `$initiator`/`$actor` 与 owner 表达式；它不是 ACL 主体，也不参与命令/转办授权。
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
