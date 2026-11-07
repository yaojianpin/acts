# 消息类型词汇（`Message.r#type` / payload `kind`）

- 状态：定稿（2026-11-07）
- 范围：`crates/acts`、`crates/acts-channel`、`crates/acts-proto`、
  `plugins/*`
- 关联：`design/assignment.md`（assignment 通知）、`design/remind.md`
  （催办记录）

## 1. 背景

模型层级已经只有 `workflow|branch|step`（外加内核为容器包内联活动物化的
虚拟分支 `block`），`act` 不再是模型类型。消息的 `Message.r#type`
（payload 的 `kind` 字段）却仍对带 `uses` 的 step 写 `act`（C6 时代
"act payload 缺省"的遗留），与模型词汇脱节：

- `act` 与 `step` 的区分等价于"该 step 是否带 `uses`"，结构上冗余；
- 内部三个谓词（`is_uses`/`is_irq`/`is_msg`）依赖 `type == "act"`；
- 订阅 glob（`ChannelOptions.r#type`）、存储行、文档都以 `act` 为契约词。

## 2. 决定

`act` 不再是消息类型。带 `uses` 的 step 的消息（act payload 形态）写
`step`；`uses` 是否存在仍是"act 级消息"的判别依据——`is_uses`/`is_irq`/
`is_msg` 改为检查 `type ∈ {step, act(遗留行)}` 且 `uses` 匹配，从而继续
排除 `assignment`/`remind` 这类同样携带 `uses` 的同 `tid/nid` 消息。

取值集合：

| type | 含义 |
| --- | --- |
| `workflow` | 运行根本身的状态消息 |
| `branch` | 分支（含 catches/timeouts 声明分支）的终态消息 |
| `step` | step：无 `uses` 的 task payload 与带 `uses` 的 act payload 统一 |
| `block` | 内核虚拟容器分支 |
| `assignment` | 转办/代办/撤销代办通知（`ref_msg_id` 指向原 `Created`） |
| `remind` | 催办记录（见 `design/remind.md`） |

## 3. 兼容

- 版本未正式发版，无存量行、无旧 payload：不做遗留 `act` 词的读侧归一，
  不保留 `act` 变体。
- 客户端订阅 glob 由 `act*` 改用 `step*`（BREAKING，CHANGELOG 记录）；
  `ChannelOptions.r#type` 仍是普通 glob，`step` 直接收敛所有 act 级消息。
- `NodeKind` 枚举删除 `Act` 变体：payload 写 `step`，`NodeFacts`
  的 row/message kind 合一，act payload 形态由 `uses` 是否存在选择。

## 4. 验收

| 场景 | 期望 |
| --- | --- |
| `acts.core.irq` step 的 Created | `type=step`，`uses=acts.core.irq` |
| 无 `uses` 的 step | `type=step`，无 `uses` |
| 旧内核 payload（缺 `kind`） | 读出 `type=step` |
| 旧行（`type=act`） | 读出 `type=step` |
| `is_msg()` | 对 `type=step` 且 `uses=acts.core.msg` 为真；对 `assignment`/`remind` 为假 |
| 订阅 | glob `step*` 收到 act 级消息；`assignment`/`remind` 各自订阅 |
