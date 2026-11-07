# 催办（remind）设计

- 状态：定稿（2026-11-07）
- 范围：`crates/acts`、`crates/acts-channel`、`crates/acts-cli`
- 关联：`design/assignment.md`（assignment/delegation）、`design/message.md`
  （消息类型词汇）

## 1. 目标

非节点 owner（发起人、监督者、管理员）可以催办一个人类审批活动：

- **没有转办人**：通知该活动当前的 owner——`type=remind` 记录即通知，
  记录的 `owner/assignee` 指向当前 owner，owner 的订阅（`type=remind`
  glob）收到投递；
- **有转办人**（任务已被转办，`transferred_from` 非空）：向转办人
  （转办后的 owner，即当前 owner）再投递一条 `acts.core.msg` 消息；
- 两种情况都保存催办记录（消息行 + `proc:graph` 折叠）。

非目标：

- 不做催办升级或超时自动催办（那是 step rules / timers 的事）。
- 不限制催办次数，不聚合重复催办。
- 催办不改变任务状态机：不决策、不改 assignment、不生成新的审批活动。

## 2. API

```text
act:remind {pid, tid, message?}
```

- wire 名 `act:remind`，`EventAction::Remind`。
- ACL 组 `execute`（决策动作同组；owner 之外的人必须能发）。
- `ActsChannel::remind(pid, tid, data)` 是 channel 的拼写；
  CLI `act remind` 同名。

## 3. 校验

在 `apply_action`（拒绝不消耗 durable intent）与 plan（重放安全）各执行一遍：

1. 活动存在、非终态、带 `__assignment`——没有 assignment 的活动没有可催办
   的 owner，拒绝。
2. 调用者必须携带认证 subject，且 `subject != assignment.owner`：owner 无需
   自催；反向拒绝沿用 `ENOTOWNER` 词头（`the owner cannot remind their own
   act`）。无 subject（未认证/引擎内部）一律拒绝——催办的语义就是"非 owner
   提醒 owner"。

## 4. 效果

一次 commit，无状态迁移：

1. **催办记录（总是）**：写一条 topic `remind.created` 的 outbox 记录，消息
   `type=remind`，同 `pid/tid/nid`；`owner/assignee/rev` 投影当前
   assignment（收件人就是它）；`inputs.from` 是催办人，`inputs.message`
   是留言；`ref_msg_id` 指向活动的原 `Created`（读 outbox payload 的
   `msg_id`，重启不丢）。落 `messages` collection，`msg:ls` 可列。
2. **转办消息（有转办人才有）**：`assignment.transferred_from` 非空时，
   在目标下再创建一个动态 `acts.core.msg` 子活动 `<tid>#remind<n>`：
   `auto`/msg map 写法同 pushed act，params 携带
   `{ to: <转办人>, from, message }`，投递即达（`acts.core.msg`
   一次 `Completed`，自完成）。目标对它短暂 park，子活动落定后 join 放行。

## 5. 查询

- 记录的持久形态：聚合 outbox 日志（随 snapshot 持久、可重放）；ack 通道
  为其物化 `messages` collection 行，`msg:ls` 可列。
- `proc:graph` 把聚合 outbox 的消息记录按 `tid` 折叠到节点
  （`GraphNode.messages`）：`remind` 记录与 `acts.core.msg` 消息都在其中。

## 6. 验收

| 场景 | 期望 |
| --- | --- |
| 非 owner 催办 | 写一条 `type=remind` 记录，`owner` 指向当前 owner |
| owner 自催 | 拒绝，无记录 |
| 未认证催办 | 拒绝 |
| 无 assignment 的活动 | 拒绝 |
| 已转办的任务 | 记录之外再生成 `acts.core.msg` 子活动，params 带 `to`（转办人） |
| 未转办的任务 | 只有记录，无子活动 |
| graph | 该节点的 `messages` 含催办记录与 msg 消息 |
