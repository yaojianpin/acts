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
| `notice` | 通知：`RunAs::Msg` 包活动的唯一投递（`acts.core.msg` 等，计算后的 params 同时作为 `outputs`），以及催办记录（`act:remind` 发出，`from/to/message` params 同样进 `outputs`；见 `design/remind.md`） |
| `assignment` | 转办/代办/撤销代办通知（`ref_msg_id` 指向原 `Created`） |

内核物化的包装节点只存在于图/行投影，不产生消息：`<step>#item` 模板
step，以及 `<step>#acts`/`<step>#in`/`<step>#act<N>` 虚拟分支。虚拟分支
的图 kind 仍是 `block`（`NodeKind::Block`），但消息流只承载声明的节点与
运行动态生成的活动。

### 2.1 声明节点一律发声（2026-11-12 修订）

消息流是声明的形状：凡是模型里**明确声明**的节点，无论声明类型
（容器、活动、无包 step、分支）都在自己的第一个时刻发消息——带候选池的
step 其 `Created` 随池请求的等待一起发出（携带原始声明 params，池尚未
解析），不等到池的答案。原 `announce` 声明删除（BREAKING，模型里残留的
`announce:` 键被忽略）。只有内核**虚拟生成**的节点不发消息（见上）；
`params.steps` 下声明的 steps 是声明节点，照常发声。被 guard 跳过的节点
以 `Skipped` 作为其唯一消息（此前无 `Created`，之后也无 `Completed`）。

### 2.2 通知的 inputs/outputs（2026-11-12 修订）

`created` 消息的 params 放在 `inputs.params`（与既有行为一致）；
`RunAs::Msg` 活动的唯一投递把**计算后的 params**（含表达式求值，起启时
对活动 scope 解析）同时放进 `outputs`——step 声明了 `exposes` 时按
`exposes` 过滤（字面量原样、`${{ }}` 对 scope 求值、仅名字读同名 param、
缺失补 `null`），未声明则整个 params 都进 `outputs`。

### 2.3 终态消息先于生命周期事件（2026-11-12 修订）

提交自身的 outbox 记录先投递、生命周期事件后发出：客户端读到的消息流以
运行根的 `completed`（`type=workflow`）收尾，`on_workflow_complete` 随后
才关闭这次运行——`Finance check and notification` 之类的最后一步结束后，
客户端先看到它的 `Completed` 与根的 `workflow completed` 消息，再收到
完成事件。

## 3. 兼容

- 版本未正式发版，无存量行、无旧 payload：不做遗留 `act` 词的读侧归一，
  不保留 `act` 变体。
- 客户端订阅 glob 由 `act*` 改用 `step*`（BREAKING，CHANGELOG 记录）；
  `ChannelOptions.r#type` 仍是普通 glob，`step` 直接收敛所有 act 级消息，
  `notice` 收敛 `RunAs::Msg` 投递与催办记录。
- `NodeKind` 枚举删除 `Act` 变体：payload 写 `step`，`NodeFacts`
  的 row/message kind 合一，act payload 形态由 `uses` 是否存在选择。
- 2026-11-12：`remind` 词并入 `notice`（`ref_msg_id` 仍指向原
  `Created`）；`announce` 声明删除；`is_act` 改为"有 `uses` 且非
  `assignment` 投影"（`is_uses`/`is_irq`/`is_msg` 随之）。

## 4. 验收

| 场景 | 期望 |
| --- | --- |
| `acts.core.irq` step 的 Created | `type=step`，`uses=acts.core.irq` |
| 无 `uses` 的 step | `type=step`，无 `uses` |
| `RunAs::Msg` 活动的唯一投递 | `type=notice`，params 在 `inputs`，计算后的 params 在 `outputs`（受 `exposes` 过滤） |
| 催办记录 | `type=notice`，`ref_msg_id` 指向原 `Created`，`from/to/message` params 在 `outputs` |
| 旧内核 payload（缺 `kind`） | 读出 `type=step` |
| 旧行（`type=act`） | 读出 `type=step` |
| `is_msg()` | 对 `type=notice` 且 `uses=acts.core.msg` 为真；对 `assignment` 为假 |
| 带候选池的 step | `Created` 随池等待发出（原始声明 params），池答案不再重复 |
| guard 跳过的 step | 唯一消息为 `Skipped`（无 `Created`、无 `Completed`） |
| 订阅 | glob `step*` 收到 act 级消息；`notice`/`assignment` 各自订阅 |
| 容器 `in` 展开 | 客户端只见容器与生成的 item 活动；`#item` 模板与虚拟分支不发消息 |
| 容器完成 | 子活动 join 落定后才发 `Completed`，fan-out 时不发 |
| 运行结束 | 根的 `workflow completed` 消息先于完成事件投递 |
