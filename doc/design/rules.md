# 步骤输入规则（Step rules）设计

- 状态：已实现
- 日期：2026-10-28
- 范围：`crates/acts/src/model/validation.rs`、`Step.rules`、
  `scheduler/shell/facts.rs`、`scheduler/shell/operation/task.rs`

## 1. 背景

人类审批活动在启动前经常需要一组业务校验或短路规则：

- 参数不合法时直接失败，并给出业务消息。
- 同一人兼任两个岗位时，跳过重复的审批活动（连岗审批）。
- 金额低于阈值时自动通过，不再打扰审批人。

这些规则如果由客户端在收到 `Created` 后再决定，会产生一次无意义的请求/投递，
而且“自动通过/跳过”需要额外的客户端逻辑。`Step.rules` 把这些规则放进模型：
步骤启动时先评估条件，命中后执行声明的动作。

### 1.1 目标

1. 规则是模型的一部分：`Step::rules: Vec<ValidationRule>`，随模型部署/持久化。
2. 条件在活动启动时、公告 `Created` 之前评估；命中规则后不再打扰客户端。
3. 默认动作 `error` 失败，支持 `skip` 跳过、`next`/`submit`/`abort`/`remove`
   自动决策。
4. 条件与表达式语言、分支守卫一致：读作用域、已解析参数、`$parent`/`$root`。
5. 结果可持久化、可重放；不引入新的操作类型或第二套状态机。

### 1.2 非目标

- 不是通用的状态转移/编排语言；规则只在**活动启动**这一个点执行。
- 不替代 `if`（步骤守卫）、`catches`（错误处理）或 `join`（多人审批）。
- 不允许规则执行 `back`/`cancel`/`push`/`transfer`/`delegate` 等需要目标或
  额外上下文的动作。
- 不为规则增加优先级字段；顺序由声明顺序决定。

## 2. 模型

```rust
pub struct ValidationRule {
    /// 条件；对步骤作用域求值，已解析参数按名字覆盖同名变量。
    pub r#if: String,
    /// `error` 动作的失败消息；缺省为 `validation failed: <if>`。
    pub message: Option<String>,
    /// 条件成立时执行的动作，默认 `EventAction::Error`。
    pub action: EventAction,
    /// 决策动作（`next`/`submit`/`abort`/`remove`）的输入；缺省 `None`。
    /// 与客户端自己的 inputs 一样落到活动 scope 上。
    pub params: Option<Vars>,
}
```

`Step` 新增可选字段：

```yaml
- id: review
  uses: acts.core.irq
  vars:
    - { name: amount, value: 0 }
  params:
    key: review
  rules:
    - if: "amount <= 0"                  # 命中：失败
      message: amount must be positive
    - if: "$parent.approver == $root.director"
      action: skip                       # 命中：跳过这个审批
    - if: "auto_submit"
      action: submit                     # 命中：自动提交，
      params:                            # 规则的 params 是决策的 inputs
        auto_submit: false
```

### 2.1 序列化

`ValidationRule` 有两种写法：

- 裸字符串：`"amount > 0"`，等价于 `{ if: "amount > 0", action: error }`。
- 对象：`{ if, message?, action?, params? }`；未知字段在反序列化时拒绝。

序列化保持紧凑：

- 默认 `error` 且没有 `message`、没有 `params` → 裸字符串。
- `error` + `message` → `{ if, message }`。
- `error` + `params` → `{ if, params }`。
- 其他动作 → `{ if, message?, action, params? }`。

`if` 为空或只有空白时拒绝；`message` 只在 `error` 动作下参与失败文本；
`params` 是决策动作的 inputs，`error` 动作只从中读 `code`/`message`。

## 3. 求值时机与顺序

规则在 `ExecuteTask` 的 phase 2 评估，顺序是：

1. 步骤守卫 `if`/`while` 与运行次数上限。
2. `assign.owner` 解析（如果声明）。
3. 包参数解析/校验（`resolve_step`；`rules` 看到的是解析后的参数）。
4. `input_rule_verdict`：按声明顺序逐条评估规则。
5. 规则没有命中 → 正常启动，公告 `Created` 并等待/执行包。
6. 规则命中 → 按动作进入失败、跳过或自动决策，**不公告 `Created`**。

规则条件通过和分支守卫相同的表达式 seam 求值：

- 作用域是活动的作用域链（步骤变量、祖先变量、`$parent`、`$root`、内置函数）。
- 已解析的参数以“参数覆盖”的形式叠加，同名参数优先于同名作用域变量。
- 条件为 `true` 时命中；`false`/`null` 表示不命中，继续下一条。
- 条件**无法求值**（编译失败、未知变量、非布尔等）直接按
  `validation_failed` 失败，无论声明的动作是什么——配置错误不能被动作吞掉。

顺序即优先级：第一条命中的规则生效，后面的规则不再评估；没有 `else`/权重。
规则在每个启动 generation 上评估一次；`back` 重入会重新评估。

## 4. 动作语义

| 动作 | 结果 | 客户端可见性 |
| --- | --- | --- |
| `error`（默认） | 活动失败，`ecode=validation_failed`，消息为 `message` 或 `validation failed: <if>`；标准错误传播，`catches` 可接管 | 无 `Created`；只看到失败/错误 |
| `skip` | 活动 `Skipped`，流程继续到下一步 | 无 `Created` |
| `next` | 启动活动，并把 `Complete` 作为它的第一个持久决策 | 无 `Created`；只看到完成/终态 |
| `submit` | 启动活动，并把 `Submit` 作为第一个决策 | 无 `Created`；只看到提交/终态 |
| `abort` | 启动活动，并把 `Abort` 作为第一个决策 | 无 `Created`；只看到终止 |
| `remove` | 启动活动，并把 `Remove` 作为第一个决策 | 无 `Created`；只看到移除 |
| 其他动作 | 不执行；以 `validation_failed` 失败，文本点名“cannot be applied when the act starts” | 无 `Created` |

决策动作按规则的 `params` 执行：`params` 是决策的 inputs，走和客户端命令
完全相同的路径——`__action` 事实记录它们，`ApplyAction` 重放时由
`variable_writes` 写到活动自己的 scope 上（同名覆盖步骤变量）。`params` 是
字面量，不做 `${{ }}` 解析：它们扮演的是“客户端这次决策带的输入”，而规则的
条件表达式才是读取/计算上下文的地方。`skip` 不消费 `params`。

`error` 动作消费 `params` 里的两个字符串键：`ecode` 替换默认的
`validation_failed`，`message` 替换失败文本（优先于规则自身的 `message`
字段）。`skip` 消费 `params.message`（字符串）作为跳过原因，优先于规则自身的
`message`。其他键和非字符串值被忽略；`error`/`skip` 不会把 `params` 写进
scope——它们不是一次决策输入。

规则命中后没有 `Created`，但一定产生**终态记录**，且记录带着规则的原因：

- `error`：`task.failed` 记录的 `outputs` 含失败代码（默认
  `validation_failed`，`params.ecode` 可覆盖）和失败消息（`params.message`
  > 规则 `message` > `validation failed: <if>`）。
- `skip`：`task.skipped` 记录的 `outputs.message` 是规则的原因
  （`params.message` > 规则 `message`；都为缺省时不写）；同一值也写在活动
  scope 的 `message` 上。
- 决策动作：规则的 `message`（若有）作为决策 inputs 里的 `message` 落盘，
  因此 `Completed`/`Submitted`/`Aborted`/`Removed` 记录的 `outputs.message`
  就是规则的原因；`params` 也一并出现在记录 outputs 中。显式的
  `params.message` 优先于规则的 `message` 字段。

客户端因此可以直接从终态记录的 `outputs` 判断“为什么自动完成/跳过/失败”。

`error` 的失败走和包错误相同的错误传播：有匹配的 `catches` 时由它处理，
否则活动/流程失败。`skip` 走和步骤守卫跳过相同的传播。

### 4.1 自动决策的持久化

`next`/`submit`/`abort`/`remove` 不是“在内存里直接改状态”：

- 启动计划把动作映射为 `ActionKind`（`Next→Complete`、`Submit`、`Abort`、
  `Remove`），在活动自己的 scope 上写 `__action@<sequence>` 事实（规则的
  `params` 就是事实的 inputs），并 seed 一个 `ApplyAction` 操作。
- 重放/重启时 `action_exists` 为真，不会重复追加决策；动作按普通客户端决策的
  路径应用。
- 动作是**引擎发起**的：没有认证主体（`subject = None`），因此决策记录的
  owner/assignee 保持 assignment 的投影，不会被误记为某个用户的操作。
- 规则命中的活动不公告 `Created`：动作本身就是第一个裁决，避免客户端收到一个
  它不需要回答的请求。

## 5. 与 assign / 参与人的关系

- `assign.owner`（用户 id 或 `${{ }}` 表达式）在规则之前解析。因此一个声明了
  assign 的活动即使随后命中 `skip`/`error`，owner 也必须先解析成功；解析失败会
  以 `assignment_failed` 失败，而不是被规则吸收。这是当前实现的有意顺序：
  assignment 是活动启动的一部分，规则不改变承办人解析的失败语义。
- 规则不改变授权：客户端决策仍按 `__assignment`/`__delegation` 检查；规则触发
  的 `next`/`submit` 等是引擎决策，不冒充 owner 的客户端操作。
- 连岗审批、额度自动通过等参与人相关规则可以读取 `$parent`/`$root` 和
  assignment 投影到消息的字段；规则自身不写 `__assignment`。
- pushed act 不在模型里，`step_rules` 对它返回空；pushed act 仍可携带
  `assign`，但不能声明 `rules`（后续如需支持，应把规则放进 `PushedAct` 事实）。

## 6. 表达式与作用域

- 条件使用和分支守卫相同的 `acts-expr` 语言：算术、比较、布尔、成员访问、
  内置函数；不支持语句/赋值/集合字面量。
- 可读：
  - 步骤变量（`Step::vars`）与祖先作用域。
  - 已解析的包参数（按名字，覆盖同名变量）。
  - `$parent`：上一个流程执行的数据（step1 在评估 step2 时）。
  - `$root`：流程根数据。
  - 内置：`$get`/`$data`/`$inputs`/`$step_*`/`$env` 等。
- 条件在启动时求值，此时包尚未执行；`$data`/`$inputs` 只包含启动前已解析的
  内容。不要依赖包运行后的 outputs。
- 规则按声明顺序求值，支持短路；后面的规则在第一条命中后不再执行。

## 7. 错误与边界

- 空 `if`：反序列化拒绝。
- 未知字段：反序列化拒绝（`if`/`message`/`action`/`params` 之外）。
- 无法求值的条件：`validation_failed`，文本包含表达式和求值错误。
- 不支持的动作：`validation_failed`，文本点名动作；不会静默降级为 `error`。
- `params` 只在命中时被消费：决策动作把它作为 inputs 写进 scope；`error` 从
  中读字符串 `ecode`/`message` 作为失败事实；`skip` 读字符串 `message` 作为
  跳过原因。`params` 是字面量对象，模板不解析。
- 规则命中后不产生 `Created`；依赖“先创建再决策”的客户端逻辑需要改为监听
  终态/错误消息。终态记录的 `outputs` 带规则的原因（`error` 的
  `ecode`/`message`、`skip` 的 `message`、决策的 `message` 与 `params`）。
- 规则结果不单独持久化；持久化的是失败/跳过/决策事实。启动重试会重新评估，
  同一快照/参数下结果确定；提交冲突后按新快照重规划。

## 8. 复杂度与风险

| 项 | 规模 | 风险 |
| --- | --- | --- |
| `ValidationRule` 模型 + serde | ~280 行（含测试） | 低：纯数据 |
| 规则求值 + 动作映射 | ~120 行（`facts.rs`/`task.rs`） | 中：顺序、Created 抑制 |
| 自动决策的持久化 | 复用 `ApplyAction`/`__action` | 低：无新操作类型 |
| 测试 | 4 个 flow cell + serde 单测 | 中：消息可见性断言 |

风险与处理：

- **顺序敏感**：规则在 assign 之后、公告之前；调整顺序会改变失败归属。文档与
  测试明确当前顺序。
- **消息抑制**：自动决策不公告 `Created`，客户端只看到终态；这是设计目标，
  但需要文档提醒客户端不要依赖“每个 act 都有 Created”。
- **动作能力**：只开放启动时可安全应用的四类决策；`back`/`cancel`/`push` 等
  需要目标，拒绝而不是猜测。
- **表达式能力**：沿用分支守卫的语言；不要在规则里做 I/O 或依赖包 outputs。
- **规则太多**：每条规则一次表达式求值；按声明顺序短路；没有额外的索引或
  优先级，保持模型可读。

## 9. 测试与兼容性

- `crates/acts/src/model/validation.rs`：裸字符串/对象/未知字段/默认动作/
  `params` 序列化的 serde 测试。
- `crates/acts/src/scheduler/tests/flow/rules.rs`：
  - `sch_rules_fail_the_act_before_it_announces`：`error` 在 `Created` 前失败，
    消息来自规则。
  - `sch_rules_pass_on_the_resolved_params_and_scope`：参数覆盖 + 作用域，
    全部不命中时正常启动。
  - `sch_rules_skip_a_concurrent_post_approval`：`$parent`/`$root` 连岗跳过。
  - `sch_rules_auto_approve_when_the_condition_holds`：`next` 自动通过，
    无 `Created`。
  - `sch_rules_error_params_name_the_failure`：`params.ecode`/`params.message`
    覆盖失败事实。
  - `sch_rules_error_params_default_the_unspecified_fact`：只声明
    `params.ecode` 时失败消息仍取规则 `message`。
  - `sch_rules_execute_the_decision_with_the_rule_params`：`submit` 按规则的
    `params` 决策，inputs 落到活动 scope，`message` 落到终态记录。
  - `sch_rules_decision_params_message_wins`：决策的 `params.message` 优先于
    规则 `message` 字段。
  - `sch_rules_skip_without_a_message_reports_no_reason`：无原因的 skip 记录
    不带 `message`。
  - `sch_rules_skip_params_message_wins`：skip 的 `params.message` 优先于规则
    `message` 字段。
  - `sch_rules_decide_without_params_by_default`：无 `params` 时决策不写任何
    覆盖。
- 回归：`cargo test -p acts --lib sch_rules`；`Step.rules` 是
  `#[serde(default)]`，旧模型没有该字段时行为不变。

## 10. 结论

`Step.rules` 把启动前的校验/短路/自动决策变成模型的一部分：条件复用表达式
语言，动作限制在启动时可安全应用的集合，结果走现有的失败/跳过/`ApplyAction`
路径。它不引入新的状态机或操作类型，也不改变 assignment/授权语义；规则在
`Created` 之前生效，客户端只会看到最终结果。
