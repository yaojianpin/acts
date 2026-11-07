# 计划：加签/减签、催办与消息折叠（2026-11-07）

一次改动面较大，拆成五个可独立提交、独立验证的里程碑。每个里程碑以
"该里程碑的测试全绿 + clippy/fmt"为完成线，按序推进；设计定稿见
`doc/design/assignment.md`（§7.5/§9/§11/§12）、`doc/design/remind.md`、
`doc/design/message.md`。

## M0 设计定稿 ✅

- `doc/design/assignment.md`：§7.5 加签/减签（顺签生成 + owner-only 授权）、
  §9 授权表、§11 API/消息、§12 验收。
- `doc/design/remind.md`（新增）：催办语义——无转办人发给当前 owner
  （remind 记录即通知）；有转办人另发 `acts.core.msg` 给转办人；总是保存
  催办记录。
- `doc/design/message.md`（新增）：消息类型词汇——`act` 退出，act payload
  写 `step`，读侧归一，订阅 glob 由 `act*` 改 `step*`。

## M1 消息类型 `act` → `step`（进行中）

- `event/message.rs`：`r#type` 注释、`is_act()`（`step|act` 遗留兼容）、
  `is_uses/is_irq/is_msg` 改经 `is_act()`（排除 `assignment`/`remind`）、
  `From<data::Message>` 读侧归一。
- `model/info.rs`：`MessageInfo::from_delivery` 归一。
- `scheduler/shell/caps/message/payload.rs`：act payload `kind` 写 `step`。
- `scheduler/shell/caps/message/mod.rs`：`message_from_record` 归一
  （缺 `kind` 或 `act` → `step`）。
- `scheduler/core/model/view/node.rs`：删除 `NodeKind::Act`。
- `scheduler/shell/facts.rs`：`message_kind` 与 row kind 合一。
- 测试清扫：acts / acts-channel / acts-proto 内全部 `act` 字面量；
  `doc/src/channel/sub.md` 订阅示例 `act*` → `step*`。
- CHANGELOG：BREAKING 条目。
- 门禁：`cargo test -p acts --lib`；`cargo test -p acts-channel --lib`；
  `cargo test -p acts-proto --lib`。

## M2 加签/减签

- `scheduler/shell/mod.rs`（`apply_action_once`）：Push/Remove 不再豁免
  per-task 授权——目标（无 `__assignment` 时沿 parent 链最近祖先）的
  assignment 存在则要求 `subject == owner`（`ENOTOWNER`），整条链无
  assignment 保持旧行为。
- `scheduler/shell/operation/action.rs`（`push_plan`）：payload 带
  `owners`(json 数组) + `mode`(before|after，默认 after) 时生成
  `acts.core.sequence` 顺签子活动（`<target>#push<n>`，
  `in` 按 mode 排列 [新增人…, owner] / [owner, 新增人…]，模板
  `acts.core.irq` + `assign.owner = ${{ $item.value }}`），并把目标记为
  已答复（park，join 落定）；`owners` 模式要求目标可解析出单 owner。
- 测试（`scheduler/tests/flow/act.rs` 扩展）：
  - 单 owner 前加签：sequence 先新增人后 owner，目标自动完成；
  - 单 owner 后加签（默认）：先 owner 后新增人；
  - 非 owner push/remove `ENOTOWNER`；无 assignment 旧行为不回归；
  - 会签节点（parallel/sequence 多人）owner 加签/减签。
- 门禁：`cargo test -p acts --lib`。

## M3 催办 `act:remind`

- `event/mod.rs` + `event/action.rs`：`EventAction::Remind`。
- `scheduler/core/commit/command/action.rs`：`ActionKind::Remind`（不迁移
  assignment、不决策）。
- `scheduler/shell/action.rs`：`"remind"` wire 编解码。
- `scheduler/shell/mod.rs`：Remind 校验（活动非终态、有 assignment、
  有认证 subject 且 ≠ owner）。
- `scheduler/shell/operation/action.rs`（`remind_plan`）：
  - 总是写 `remind.created` 记录（`type=remind`，owner/assignee/rev 投影，
    `inputs.from/message`，`ref_msg_id` 指原 Created）；
  - `transferred_from` 非空时另建 `<tid>#remind<n>` 动态
    `acts.core.msg` 子活动（auto/msg map 同 pushed act，params 带
    `to/from/message`）。
- `acl.rs`：`act:remind` → `execute` 组。
- `export/executor/act_executor.rs` + `export/actions.rs`：`act:remind`
  包装与路由。
- `acts-channel`（`remind` 发送方）、`acts-cli`（`act remind` 命令）。
- 测试：非 owner 催办有记录；owner 自催拒绝；未认证拒绝；无 assignment
  拒绝；已转办有 msg 子活动投递转办人；未转办无子活动。
- 门禁：`cargo test -p acts --lib`；`cargo test -p acts-channel --lib`；
  `cargo test -p acts-cli --lib`。

## M4 `proc:graph` 折叠消息

- `scheduler/shell/graph.rs`：`process_graph` 接收消息集合，按 `tid`
  折叠到节点；`Runtime::graph` 从 `store().messages()` 读 pid 的消息传入。
- `model/info.rs` + `acts-channel/src/model/info.rs`：`GraphNode.messages`
  （`GraphMessage{ id, r#type, state, uses, owner, assignee, name,
  timestamp }`，`#[serde(default)]`）。
- 渲染：`ProcessGraph::tree_output` 与 CLI `GraphRenderer` 打印消息行。
- 测试：`scheduler/tests/flow/graph.rs`（节点消息折叠 cell，含 msg 与
  remind 记录）、`export/actions.rs` wire cell、`acts-cli/tests/repl.rs`。
- 门禁：`cargo test -p acts --lib`；`cargo test -p acts-cli --lib`。

## M5 收尾门禁

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`
- `cargo fmt`（或 `--check` 验证）
- 全套测试（AGENTS 清单，含 acts-store 三 feature、acts-server；
  NATS/Postgres 不可用时如实说明）
- CHANGELOG 五条（每里程碑一条）；README/doc/src 中受影响的词汇
  （`act*` 订阅示例、消息类型说明）核对更新。
