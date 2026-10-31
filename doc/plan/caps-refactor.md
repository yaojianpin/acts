# caps 化重构计划（能力模型 + assign 重构）

- 状态：A–E（2026-10-30）、§5–§6（2026-10-31）已完成；`impl Workers` 仅剩按设计留在内核的
  调度/路由本体
- 日期：2026-10-29
- 设计依据：`doc/design/caps.md`
- 范围：`crates/acts`、`crates/acts-server`、`crates/acts-channel`、`doc/`

## 1. 目标

1. 概念明确：assign 下三种人——责任人（owner/assignee，单人）、代办
   （delegator/delegatee，单人）、候选人（candidates，列表，`CandidateExpr`
   结构）；三种能力分别实现到 `shell/caps/assign/` 下。
2. 删除 `acts-plugin-participant` 参与人插件方案：参与人概念改为候选人，
   计算能力进 `acts.core.candidates` 包；配置 `{ params, uses }`。
3. join 的 delegated 规则增加 `uses`（默认 `acts.core.decide`），实现迁移到
   `caps/join`（含 all/any/partial 本地判定）。
4. `package/core` 新增 `acts.core.candidates` IRQ 包。
5. 删除 Engine 对外注册能力的方法；能力扩展统一走 package；
   `capability` → `caps`，`KernelPorts` → `CapServices`，`KernelHost` →
   `Workers`（`ports` 字段 → `caps`），能力 trait 自带 key，`sys:status`
   输出 caps 列表。

## 2. 阶段与验收

### A. 重命名与简化（已完成）

- `capability` → `caps`；删除 `PortSet`、`CapabilityKind/Provider/Requirement`、
  `ParticipantResolver`；`CapServices` 单槽 `Arc<dyn Trait>`；
  `Workers`/`caps` 字段；`Capability::key()`；`FnLifecycleSink`。
- 删除 Engine 三个注册方法与 runtime 注册表/固化/封口；
  `Assign.provider`、`with_assign_provider`、`required_capabilities`、
  `has_assignment` 发布检查全部移除；owner 解析只走本地原子。
- 删除 `plugins/acts-plugin-participant` 及 server/测试引用；
  `OwnerExpr` → `CandidateExpr` 全局重命名。
- `Assign` 模型改为 `{ owner: Option<Box<CandidateExpr>>, candidates:
  Option<Candidates{params, uses}> }`；`JoinRule::Action` 改为
  `{ params, uses }`（默认 `acts.core.decide`），请求信封 `{ params,
  outcomes }`，decide 包 schema 同步。
- `SystemStatus.caps` + acts-channel 镜像。
- 验收：`cargo check --workspace --all-targets` 通过（进行中）。

### B. join 能力迁移（已完成 2026-10-30）

- `shell/caps/join/{mod,local,action}.rs`：从 `scheduler/core/plan/container/join.rs`
  抽出 `Vote`/阈值判定为纯决策核心；`scheduler/shell/operation/join.rs` 保留
  plan 编排并调用 `caps::join`。
- 验收：现有 join 测试（scheduler/tests/flow/join.rs、container/tests/join.rs、
  model/join.rs）全绿；delegated 规则断言 `uses` 来自规则本身。

### C. 候选人能力与包（已完成 2026-10-30）

- `package/core/candidates.rs`：`acts.core.candidates`（Irq/Core），
  `prepare_params` 解析 params 为 CandidateExpr 并替换本地 context 原子
  （自原插件 `resolve_local` 移植）。（已完成）
- `shell/caps/assign/candidates.rs`：候选池契约（`__candidates_wait`/
  `__candidates_ask`/`__candidates` 标记与 `{ params, uses }` spec）；
  start 时声明了 candidates 的 act 推迟启动（`Ready→Waiting`）、push 请求
  与应答折叠进变量并重入启动（fan-out 参数此时才能读到候选人）在
  `shell/caps/assign/candidates.rs`——切分同 join：caps 侧契约，shell 侧编排。
  请求失败走普通 error walk，skip 重问。（已完成）
- 容器（block/parallel/sequence）允许声明 candidates（owner 仍限人类叶）；
  `declared_expression` 的拒绝表相应放宽。（已完成）
- `shell/caps/assign/{owner,delegate}.rs`：`Assignment`/`Delegation` 状态、本地
  求值（原 `shell/owner.rs` 整体移入并删除）、授权与 rev/形状解析、payload
  投影是纯规则，落 `shell/caps/assign/`；声明检查、单 owner 解析
  （`AssignmentResolver`）、validate_*、snapshot 投影是编排，留
  `shell/caps/assign/mod.rs`；`CapServices` 增加 `kernel.assign` key 槽位。
  （已完成 2026-10-30）
- 验收：候选人 e2e 测试（candidates 计算 → parallel 按 ids 展开多个 owner
  act → join 会签）；业务原子经 candidates 包解析；actor 授权回归
  （ENOTOWNER/过期 delegatee）。（2026-10-30 全部通过：flow/candidates.rs 4 例 +
  全量 acts lib 1719 例。）

### D. 全量门禁（已完成 2026-10-30）

- `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`：通过（0 警告）。
- `cargo fmt`：已执行。
- AGENTS.md 测试清单：acts lib 1719、doc 12、acl 13、expr 92、channel 82、
  plugin nats 9 / grpc 25 / web 38、package http 33 / shell 10 / state 14 /
  javascript 28 / nats 5、cli 31、server 22 全绿；acts-store `--lib` 按设计
  无 lib 单测（覆盖在集成文件），mem 94 / sled 108 / sqlite 108 /
  engine_sqlite 2 全绿。**未能运行**：store 的 `tests/postgres.rs`（本地无
  Postgres 16 @5433）与依赖 NATS JetStream 的 broker 集成 suite（本地无
  NATS @4222）——与 CI 的差别仅这两处。

### E. 文档（已完成 2026-10-30）

- `doc/design/caps.md`（新）；`design/shell-extensibility.md` 重写；
  `design/assignment.md`、`design/owner-expr.md`、`src/model/assign.md` 更新；
  删除 participant-plugin/shell-extensibility/assignment 的 plan 文档与
  `design/participant-plugin.md`；CHANGELOG 记录破坏性变更。
- `design/scheduler.md` 为历史记录，仅加注 `KernelHost` 现名 `Workers`。

## 3. 兼容性说明（破坏性）

- 模型 `assign: { owner, provider }` → `{ owner?, candidates? }`；`provider`
  字段删除（stored models 需迁移，引擎不再读取）。
- `join: { action, params }` → `{ params, uses }`；旧对象形式反序列化报
  unknown field，`action` 键移入 params。
- 参与人插件删除：装过插件的部署改为依赖内置 `acts.core.candidates`。
- `sys:status` 增加 `caps` 字段（客户端镜像同步，向后兼容的字段新增）。

## 4. 关键决策记录

- 能力扩展统一走 package（uses），删除注册表/provider 选择/封口（用户决策，
  2026-10-29）。
- 能力 trait 自带 key；status 输出 caps 列表（用户决策，2026-10-29）。
- `KernelPorts` → `CapServices`，`KernelHost` → `Workers`（`ports` →
  `caps`）（用户决策，2026-10-29）。
- candidates 与 join 的配置统一 `{ params, uses }`（用户决策，2026-10-29）。

## 5. `$assign` 环境变量与 step/子 step（已完成 2026-10-31）

需求：env 增加 `$assign`，可读 `owner`/`assignee`/`delegator`/`delegatee`/
`candidates`（计算后的列表）；每个 step 都有 assign 配置，需要区分「本 step
的 assign」与「子 step（生成的后代）的 assign」，让子 step 能访问父 step 计算
出的候选人来生成子 step。

已按用户确认的建议方案实现（2026-10-31）：

1. **数据来源**：`__assignment`/`__delegation` 是执行级私有事实，候选池计算
   结果写在声明它的执行的 `__candidates`。`$assign` 是 env 端只读投影
   （`shell/caps/assign/projection.rs` 的 `impl Assign`，经
   `Scope::assign_projection` 桥接，`env` 的 `$assign` builtin 暴露）：
   `$assign.owner`、`$assign.assignee`、`$assign.delegator`、
   `$assign.delegatee`、`$assign.candidates`，另随行 `ids`/`participants`/
   `count`。
2. **作用域规则**：身份字段与候选池**各自**沿 scope 祖先链就近解析。
   - 身份字段取最近一个持 `__assignment` 的执行，delegator/delegatee 取该
     执行自己的 `__delegation`（不从无关祖先继承）。
   - 池字段取最近一个已算出候选池（`__candidates`）的执行。
   - 叶子人类 act 内：`$assign.candidates` 是本 act 计算的池（若有）。
   - 容器生成的子 act（`<id>#act{i}`/`<id>#item`）：自身通常只有 owner，
     `$assign.candidates` 命中**父容器**的池 —— 子 step 因此能看到生成它的
     候选人列表。
   - 显式指名：表达式语言的对象不可调用，实际形式为
     `$assign.at("<step-id>")`（node id 取该节点最新 generation；也接受 wire
     `tid`），只读该执行自身的事实、不回溯祖先。
   - 无 assign 事实时回答 `null` 身份与空列表，不报错；无共享聚合的作用域
     （谓词回退）回答空投影。
3. **生成子 step**：`parallel`/`sequence` 的 `in: "${{ $assign.candidates }}"`
   与 item 叶子的 `owner: "${{ value }}"` 组合即是"由候选人生成子 step"；
   `block` 的 `steps` 模板里同样可引用。内核不需要新配置键——**区分靠 scope
   链的就近解析 + 显式执行 id 限定**，配置形状不变。

验收（已覆盖）：`scheduler/tests/flow/candidates.rs` 的
`sch_candidates_child_reads_the_parent_pool_through_assign`（容器 candidates →
子 act `$assign.candidates` 命中父池；`in: "${{ $assign.candidates }}"`；
`$assign.at('approve').ids` 显式命中）与
`sch_assign_without_facts_answers_an_empty_pool`（无 assign 的流程
`$assign.candidates` 为空列表、`$assign.owner` 为 null 而非错误）。

## 6. 能力模型统一与 shell 收敛（2026-10-31，已完成）

用户决策（2026-10-30）：

- caps = 对外扩展面（原预期）；shell 原设计也偏对外，现混有 flow/调度
  操作，思路是**逐步把 shell 的对外能力迁入 caps**，shell 越来越内聚稳定，
  caps 扩展。
- 每个能力一个目录 + 一个能力类型（实现 `Capability`，自带 key）：纯语义
  与编排都是该类型的方法；不做散落自由函数，不往 `Workers` 上堆实现。
  目录不必迁出 scheduler。
- `Workers` 只做槽位管理（`attach_*`）与分发；能力不回调内核，副作用只经
  返回值/计划意图，由内核 commit。
- 桥：对外接缝以 **Context** 为标准桥（ScopeSpec → headless Context）；
  内核能力以 **snapshot/事实入、计划意图出** 为桥。详见
  `doc/design/caps.md` §3（功能盘点/能力清单/调用关系/迁移顺序）。

已落地：

- `AssignmentResolver` 从 Workers 附加槽并入 `CapServices.assignment`
  （key `kernel.assign`，默认实现 `RuntimeAssignmentResolver`，未装配时
  assign 失败关闭）；`scheduler::shell` 重新私有化，caps 无法引用 shell
  内部（编译期强制边界）。
- assign 能力成形：授权/改派/解析/注入/ask/settle 全部是 `Assign` 的方法
  （`impl Assign` 分布于 `caps/assign` 与 `shell/caps/assign`）；`Workers` 仅剩
  `attach_assignment_resolver` 与薄分发 `resolve_assignment_owner`；
  owner 表达式本地求值回 shell（`shell/caps/assign/evaluate.rs`，resolver 缝的
  内核实现）。
- join 纯语义方法化：纯判定物理落 core `join_rules.rs`（core 纯度架构测试
  禁止 core 文本出现 `crate::scheduler::shell`，caps/join 对其再导出）。

已完成（2026-10-30 续）：

- **消息投影能力**：`shell/caps/message/` 拆为 transport（`mod.rs` 的
  `MessageForwarder`）与 payload（`payload.rs` 的 `step_uses`/
  `act_message_payload`/`task_message_payload`/`operation_message_payload`/
  `attach_join_message`，全部为 snapshot 上的自由函数）；
  `attach_join_message` 移出 `propagate.rs` 的 `impl Workers`。
- **join 编排方法化**：`Join::settlement`/`ask`/`outcomes`/`has_live_work`/
  `apply_verdict`，`propagate.rs` 直呼 `Join::settlement`。

已完成（2026-10-30 续二）：

- **读投影**：`read_model.rs`/`projection.rs` 归组为 `shell/caps/read/`；
  `RowProjector` 槽位并入 `CapServices.read`（`kernel.read`），
  `Workers.projection` 字段删除。

已完成（2026-10-30 续三）：

- **operation 规划方法收敛**：`ExecuteTask` 的 `guard_verdict`/`guard_skip_plan`、
  `DispatchTimeout` 的 `timeout_branches`/`guard`/`round`/`plan`/
  `empty_remainder`、`PropagateTask::flow_of` 全部从 `impl Workers` 迁为各
  驱动类型的关联方法（需宿主设施的传 `host: &Workers`）。

已完成（2026-10-31）：

- **`ApplyAction` 规划方法组收敛**：`action.rs` 的 18 个规划方法
  （`completion_plan`/`waiting_after_effect_plan`/`waits_for_timeouts`/
  `container_fanout_plan`/`park_for_children_plan`/`transfer_plan`/
  `delegate_plan`/`undelegate_plan`/`push_plan`/`waits_for_pushed`/
  `fold_effect_outcome`/`failure_plan`/`package_failure_plan`/
  `catch_fanout`/`attach_operation_message`/`param_fanout_children`/
  `fanout_command`/`join_fanout_failure_plan`）从 `impl Workers` 迁为
  `impl ApplyAction` 的关联方法：需要宿主设施的（`task_operation_plan`、
  `evaluate_branch_predicate`、`caps.effects`）以 `host: &Workers` 参数串入，
  纯规划（阈值/折叠/消息附着/子节点构造）不带宿主。`ExecuteTask` 的失败
  路径改呼 `ApplyAction::failure_plan(host, …)`；`impl Workers` 在
  `action.rs` 中已清空。

保留在内核（按设计不成能力、不迁移）：

1. `dispatch.rs`/`return_to_parent.rs`/`mod.rs` 的调度与路由本体留在内核。
