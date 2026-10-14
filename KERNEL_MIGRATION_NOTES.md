# 内核迁移对账笔记(`feat/kernel` 分支)

kernel 影子迁移的工作状态与对账结论。续做任何 P2 切片前先读本文。
门禁:每笔提交前 `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings`
+ `cargo fmt` + `cargo test -p acts --lib --tests`(唯一允许的红:
`reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`,迁移期已知)。

## 已完成(按提交序)

- 内核接线(kernel/ 37 文件 + runtime_state::OperationIdentity + op 词表 parse)
- LaneRouter 轮转均衡(调度族 queue+gate 共享实例,writer 独立实例,删除后 forget)
- Task 去 Weak<Process>:proc 归 Context,全链显式传参
- kernel_host 影子外壳:异步驱动(跑在引擎 KvStore 上)、start→ExecuteTask→ApplyAction→
  PropagateTask 纵切、wake 丢失由 recovery 扫描收敛
- Config.kernel_engine(默认关):Runtime start 影子建档 + 驱动任务(wake/tick 恢复/投递)
- 动作路径:KernelHost::apply_action(intent 先行→claim→ActionPlanner 重规划→原子提交);
  auto/等待判定按 `ActPackageDefinition.run_as`(Func/Msg 自完成,Irq 等待,未注册→默认 Irq 等待);
  Runtime::do_action 影子对账钩子(legacy act 随机 tid → 上溯 step 的模型节点 id);
  影子动作落在传播未达的 Created 上时重试而非丢弃
- SkipTask 级联:死终态前驱(Cancelled/Failed/Aborted/Skipped)→ 后继 Created→Ready +
  SkipTask op(Ready→Skipped + PropagateTask 续传);`Skipped` 移出 join success 集;
  `Backed` 前驱改等待;codec tag 追加 6(不重排,golden fingerprint 不变)
- Waiting park:手动任务 ExecuteTask 直接 Ready→Waiting(状态表新增 Ready→Waiting,
  对齐 legacy Interrupt);apply_action 的 ApplyAction arm 接受 Running|Waiting
- Cancel→GenerationPlanner 重入:`plan_reentry` 接受 Backed|Cancelled,重建 source 的
  前向闭包(source 新代 Ready + ExecuteTask,下游新代 Created 由传播逐个接纳,
  前驱边重映射,**父边同闭包重映射**(嵌套闭包的父必须指向新代,join 依赖它),
  auto 变量随代继承);`validate_preconditions` 变量写 scope 允许指向
  本计划 creates 的新执行;宿主 Cancel 分派在级联 settle 后发 ReenterCommand
- 嵌套模型导入(kernel/container.rs + kernel_host):`from_workflow` 接受 branches。
  机制:模型事实(`__model` JSON、`__auto` 每步节点判定、workflow vars、各 step 声明
  vars)全部落成根 scope 变量,聚合自回放不依赖 spec;容器(branch step / branch)在
  自己的 ApplyAction hop 做 fan-out(`ContainerPlanner::plan_fanout`:未选中分支
  建 Ready+SkipTask(每模型节点一个终态执行,Skipped 分支的内部步骤不建),选中分支
  Ready+ExecuteTask,branch 内部链首步 Entry 其余 Created 排队),源执行 Waiting 停靠;
  每个 hop 的 PropagateTask 附带 `plan_join`(父 Waiting + 全子终态且无 Aborted →
  父 Completed + PropagateTask,对齐 legacy auto_complete 的 abort 让位);
  子接纳走 `GenerationAdmissionPolicy`(Absent/已完代→分配新代, rewind 重建的
  Created→Readmit 不重复分配, 活跃→Ignore);branch if 选择复用 kernel
  `BranchPlanner::decide/select`,谓词求值 = acts_expr 对聚合 scope 链(近者胜)+
  动作 inputs 覆盖层(legacy 先写 action vars 再调度分支);"已 fan-out 的容器"
  以"存在非 Created 子"判定,其上的 Complete 是强停(对齐 legacy 对 Running step
  的 Next)。级联扩展:死源 + Ready/Running/Waiting 后继 → SkipTask op(状态变更
  归 hop;传播 op 序列改按快照 next-sequence,避免与既有 hop 撞 id);
  终态执行上的 ApplyAction hop 落空 plan 而非报错。
  导入仍拒绝(不静默降级):branch `needs`;branch 内 step 的 `if`/`while`
  (kernel Skipped=死路,legacy step guard=穿行,一个状态不能两义);
  step 同时带 next+branches 沿 legacy 视 branches 为死配置。谓词求值失败 =
  InvalidDecision(hop 留存重试,阴影内可见;legacy 对应任务错误路径,Error parity
  属剩余工作 3)。
- parity 矩阵 Abort/Error/Back(kernel/upward.rs + generation.rs + host):
  Abort = 向上级联(`UpwardPlanner::plan_upward`,祖先全链 Aborted,**终态祖先也
  重写**——状态表新增 终态→Aborted/Failed,对齐 legacy abort_task 的无条件改写;
  已 Aborted 祖先让位)+ 向下死源级联,其中 Aborted 源的 Running 后继走新
  `AbortTask` hop(Running→Aborted,legacy 祖先行对 running 子的处置;codec tag
  追加 7 不重排);Completed 兄弟非祖先、保持原状。
  Error/Fail = 同一 upward 机制 → Failed(legacy emit_error→parent.set_err 把
  祖先链改写为 Error,parity 测试对 legacy 实测确认)+ SkipTask 下游级联;
  **catch 分支仍未导入**——带 catch 的错误路径不在 parity 内(cutover 前)。
  Back = `plan_reentry` 闭包根改在 `to` 命名步的最新代(Backed 源必须在闭包内,
  否则 SourceOutsideRedoClosure 拒绝;redo 重跑 to 下游全部,Backed 旧代保持),
  宿主对 Back 依 inputs 的 `to`(ACT_TO)派发重入;缺 `to` 拒绝(对齐 legacy 文案)。
  legacy 对照实测:Abort/Fail 与 legacy 逐步骤终态一致(acts 折叠进所属 step,
  Interrupt→Waiting/Error→Failed 映射);**Back 的 legacy 折叠对照未做**——重入流
  里 legacy 同节点多任务多状态(旧 Backed + 新 Waiting),折叠规则要等行投影
  (legacy 行变成 receipt 投影)才有干净语义,当前以 kernel 机制测试收口。
- executor 移植第 1 片(kernel_host/effects.rs):效果接缝。宿主可附
  `EffectRunner`;AutoAction hop 对 auto( Func/Msg)且带 `uses` 的 step 执行其
  包(`PackageEffectRunner`:engine 注册表 + headless Process/Task 上下文,参数走
  legacy 同一 `fill_params` 路径),outputs 以变量写并入**同一原子提交**;包错误 =
  该执行 Failed + error.code/message,由传播的错误行走接管(legacy 包错误路径)。
  契约与 legacy 相同:效果先于提交(at-least-once,崩溃重放会重跑包)。Irq 的
  消息发射与 Msg 出站消息不在本片(消息 parity 另拆)。宿主不附 runner 时行为
  不变(阴影自动完成);运行时接线与 cutover 翻转是后续片(阴影期先跑真实效果
  会有双执行,IO 类包必须等驱动切换)。
- executor 移植第 2 片:act 消息。每个 `uses` step 的 ExecuteTask hop 写一条
  **持久 outbox 记录** `message.created`(payload JSON:nid/uses/params 原始模型
  值;key 带 pid+nid+gen,一代一条),由附接的 transport 投递——outbox 的
  claim/confirm 给消息幂等投递与崩溃重放,对齐宿主架构;Irq 执行即 Waiting 在
  其消息上,Func 执行随后完成(legacy Created+Completed 对)。驱动翻转时
  transport 把记录转成引擎 Message(tid=节点 id,与动作钩子同键);阴影期
  ShadowTransport 仍只记日志,无双投。已完成消息暂由 task.completed 记录承载,
  事件名映射属 transport 接线片。
- executor 移植第 3 片:transport 接线 + 效果参数保真。
  (a) 效果的 params 现在对**聚合 scope 变量**求值:共享的 `scope_vars`(执行 →
  根的 scope 链,近者胜)注入 headless task 数据,`${{ ... }}` 走 legacy 同一
  fill_params 路径解析(谓词求值也改用它,消除重复实现)。
  (b) outbox transport 变成宿主可附接(`attach_outbox_transport`,默认 Shadow;
  serve 用附接的那个),提供 `message::message_from_record`(message.created→
  Created、task.completed→Completed、task.failed→Error、task.skipped→Skipped;
  tid/nid=节点 id,type=act,inputs.params/error.*、outputs 逐字段还原)与
  `MessageForwarder`(记录→引擎 `Message`);uses step 的 completed/failed/
  skipped hop 现在把 {nid,name,uses,params,mid,outputs} 写进记录 payload,非 uses
  记录 payload 为空、不成消息。`OutboxTransport` 加 Send+Sync(宿主跨任务 clone)。
  驱动翻转时接 emitter;阴影期不接(否则与 legacy 双投)。
- executor 移植第 4 片:效果参数的 sealed/快照对齐(本片)。
  `ProcessSpec` 携带运行属主 `ScopePolicy`(runtime::start 从 start options 取出
  的 owner 随 spec 落成根 scope 的 `__owner`,与 legacy proc env 同键;
  `linear()`/无 owner = deny_all,即 legacy 无 env 键时的默认)以及**启动 inputs**
  (owner pop 后的 options 落根 scope,排在 workflow vars 之后 → 同名 input 覆盖
  声明 var,与 legacy `load_with_vars` 的 append 同序;这是 scope 键来自
  `proc:start` 的 `uid` 时的关键,否则聚合缺该键 → `on_missing=Skip` 不 seal);
  `EffectRequest` 把 owner 交给 headless proc,并由 runner 走 **legacy 同一 task
  init 路径** `ctx.prepare()`(`init_vars` + `resolve_sealed`):快照 scope 键从
  聚合 scope 链解析、按 owner `allows` 校验后 `set_sealed`,params 再 `fill_params`
  解析 —— `${{ $profile.x }}` 与 `${{ v }}` 与 legacy 同源(`$profile` 带 `$`,
  与 legacy 表达式一致)。`scope_vars` 过滤 `__` 私有键(引擎事实
  `__model`/`__auto`/`__owner` 不进表达式变量空间,legacy 任务也读不到)。host 级
  对跑 legacy 实测(同一 engine/owner/快照,scope 键来自启动 inputs):kernel 效果
  输出 = legacy 包收到的 params(kernel 聚合变量是值的 JSON 编码);属主缺 scope
  授权 → 效果 `Failed`(Denied),无 token 输出泄漏。
- executor 移植第 5 片:运行时接线(驱动档)。`Config.kernel_engine` 从「影子」
  改为「驱动」——开关打开时 kernel 就是引擎:
  (a) `Runtime::create` 给宿主接 `PackageEffectRunner`、`MessageForwarder`
  (outbox → `Emitter::emit_message`,act 级消息)与**进程生命周期 sink**
  (`ProcessEvent::Started/Completed/Errored`);host 每个 hop 提交后按新快照上报:
  存活 → Start;全终态 → root `Failed` 走 Errored(读 root 的 error.code/message)、
  其余终态走 Completed 并按 `model.exposes` 校验 outputs(失败转 Errored)——
  对齐 legacy `on_proc`;上报每进程一次(内存去重,同 legacy 事件语义)。
  (b) `Runtime::do_action` 直接路由 `host.apply_action`:kernel 消息以模型节点 id
  为 tid,客户端 tid 已是执行键,无需 act→step 上溯,也没有 resident 检查与
  shadow 镜像(影子钩子删除);`act:push/remove/set_process_vars` 无 kernel 对应,
  驱动档下显式报错而非静默降级。
  (c) `Runtime::start` 驱动档只建聚合(owner + 启动 inputs)并 `start_process`,
  **不 launch**、不写 proc/task 行;返回的 `Process` 仅供本调用 API 的 id
  (`runtime.proc(pid)` 为 None,进程目录仍照 config.workdir 创建)。
  (d) headless 效果作用域带进程 workdir:`<workdir>/<pid>`(复用
  `prepare_workdir`),`$env.WORK_DIR`/`Context::workdir` 与 legacy 同名同目录,
  IO 类包不再失去目录约束。
  engine 级实测(kernel_engine=on,host 作为引擎驱动):Func 链经 channel
  `on_complete` 收到完成事件、聚合全终态、`runtime.proc(pid)` 为 None;Irq 步的
  Created 消息由 kernel outbox 经 emitter 送达 channel(订阅者按 params key 命中)、
  `act:complete` 由 host 服务并收口;`${{ $env.WORK_DIR }}` 效果参数 =
  `<workdir>/<pid>`。
  已知边界(后续片):读 API(proc/task/msg 行)未投影,驱动档下为空 —— 属行投影片;
  包上下文写回(`$set`/`set_env`/task data 等 headless 写)未折回聚合;Msg 出站消息
  与 Irq 的包执行未接(消息 parity 片);进程目录 `<workdir>/<pid>` 由 legacy
  sweeper(随行)清理,行投影前驱动档下不会回收。

- executor 移植第 6 片(本片):驱动档读模型 + 重启判别 + 包上下文写回 + act 消息保真。
  (a) 行投影(`kernel_host/projection.rs`):宿主每笔提交后从新快照把 **proc/task/vars 行**
  物化进既有集合 —— 只投影该计划触及的执行 + proc 行(outbox 结算后再刷一次 proc 行以置
  `removable`)。proc 行:根状态、mid/name、model JSON、`__owner`/`__workdir`/`__env`、
  err、removable;task 行:模型节点自身的 NodeData(根行 tid=`$`、node_data 是模型节点,与
  legacy 同形)、kind/名字/边、状态映射(Created→Pending、Waiting→Interrupt、Failed→Error,
  其余一一对应)与时间(首次投影打点,读改写保留;重入代清 end_time);vars 行:该执行 scope
  的 data JSON(私有键与 `auto` 不入行)。**每节点只投影最新一代**(store 以 pid+tid 为键,
  重入节点只留存活实例,与「每节点一个终态任务」的 parity 口径一致)。`removable` = 全终态
  + outbox 全部 Delivered + 该 pid 无未结算 delivery(先 barrier 该 pid 写分片,再看
  `has_unsettled_deliveries`);任务转为终态时同步 `close_deliveries`(legacy writer 的
  close),ack 通道的投递行因此不会永久挡住 sweeper。命中后既有 sweeper 删除行 +
  `<workdir>/<pid>` 目录。驱动档 `proc:get`/`task:get`/`msg:ls`/`Runtime::proc` 由此可读。
  (b) 重启判别:驱动档 `Runtime::start` 先问 `KernelHost::exists`(=
  `AsyncAggregateRepository::load`)再退回 proc 行 —— 汇总文档优先于行;行已被 sweeper 回收的
  终态 run 仍拒绝同 pid 重启(聚合还在)。启动期恢复仍由 serve 首个 tick 的 `recover()` 承担
  (恢复索引重推未结操作),未新增 pass。
  (c) 包上下文写回:效果结束由 runner 返回增量 —— 返回的 outputs、headless task 自身新增/
  改动的 data(delta,避免把父作用域变量复制成局部遮蔽)、`Context::set_env` 的进程 env(并入
  根 scope 私有 `__env`;后续效果再灌进 headless proc env,`$env.X` 与 legacy 进程 env 同义)。
  全部并入完成那一笔原子提交;proc 行的 env 亦投影 `__env`(headless workdir 机制不变)。
  (d) act 消息保真:每个 `uses` step 的**起点** hop 先按 legacy `run()` 语义 resolve(定义 +
  `definition.validate(params)`,Irq/Msg 同样),解析后的 params/options 落私有 `__params`/
  `__options`;起点/完成/失败/跳过四种记录复用同一组事实,不重跑 headless scope(其 snapshot
  seal 只准在起点失败)。记录 payload 补齐 legacy `Task::create_message` 的 inputs(params、
  options、`step` 链接、`model`、前驱作用域 outputs、error.*);客户端动作决定的终态
  (complete/submit/skip/cancel/abort/fail)在 ActionPlanner 的记录上附 act 载荷,message 侧
  补齐 task.submitted/backed/cancelled/aborted/removed 主题映射。resolve 失败按 legacy 序列
  处理:起点只能 Ready→Running,失败由效果 hop 定 Running→Failed。
  对跑实测(`kernel_host::tests::kernel_act_messages_match_the_legacy_run`,同 workflow 双引擎):
  legacy 逐消息为 `workflow created / step s1 created / act completed / step s1 completed /
  step s2 created / act created / act completed / step s2 completed / workflow completed`;
  折叠到(节点, 状态, 有无包)后与 kernel **逐项相等**;两侧读模型行(act 折到所属 step 节点)
  折叠后**逐项相等**;根节点消息由 lifecycle 通道(on_start/on_complete)承担,两侧一致。
  已知边界(明确记录,未静默降级):纯 step(无 `uses`)与 workflow 根的 **task 级消息** 目前
  不进 `on_message` 流(legacy 会发 type=step/workflow 的 task 消息);多前驱 join 在行投影里
  收敛为一个节点一行(单个 prev);vars 行 `sealed` 为空(聚合不存 sealed 快照);时间戳是投影
  时打点(聚合无时钟)。

- 谓词失败路径(旧队列「谓词求值失败的失败路径」项,本片收口):分支 `if` 求值失败不再
  让 hop 无限重试。
  - fan-out 时该分支被**准入并在其首个 hop 失败**:fan-out 给它钉私有 `__branch_error`
    = {code,message},`execute_task_plan` 读到即走 `failure_plan`(错误变量 + Failed +
    传播);其余分支照常按谓词选择,`else` 兄弟被挡(legacy 的 else 只在全部 if 兄弟 skip
    之后才跑,而报错的那个没有 skip)。
  - 状态表新增 `Ready → Failed`,附 legacy 依据:分支谓词在 `Branch::init` 求值,任务
    还没进 Running 就报错。
  - 传播 hop 里 **upward 判定优先于容器 join**:同一 hop 里祖先被两条路径同时决定会撞
    `DuplicateStateChange`;legacy 的顺序是 `parent.set_err` 先于 auto-complete 看到子任务
    结算,所以 `upward_to` 非空时不再计算 join。
  - 对跑实测(`kernel_invalid_branch_predicate_matches_the_legacy_run`):s0 Completed、
    step1 与根 Failed、b1 Failed、b2 与 s2 Skipped,逐节点与 legacy 折叠一致。
  - 导入期**拒绝** `step.catches`/`step.timeouts`(legacy 的 tree 真会建 Catch/Timeout
    子节点、`Step::on_error`/`on_timeout` 真会分派,静默丢弃等于换掉错误路径);新单测
    `catch_and_timeout_declarations_are_rejected_not_silently_dropped`。
  - 谓词求值的后端:驱动档已接到引擎表达式环境(见下一条),自备子集求值器(acts_expr,无环境
    函数)只是没有 runner 时的回落。
  - 顺带:`folded_legacy_states` 跳过 `state=None` 的 legacy 任务(legacy 会留下从未启动的
    分支内部步骤,如未进入分支的 `s11`/`s21`;kernel 根本不建它们)。

- 分支谓词接到引擎环境(catch 的前置):`kernel_host/predicate.rs` 的 predicate seam —— 驱动档
  把每个分支 `if` 放到**引擎自己的表达式环境**上求值:一个 headless task 上的
  `ctx.eval::<bool>`(legacy `Branch::init` 同一条路径),所以 `$env`/`$ecode()`/`$cost()`
  等环境函数按 legacy 语义参与判定;完成动作的 inputs 作为覆盖层叠在 scope 变量之上(legacy 先
  把 action vars 写进 task scope 再调度分支)。求值失败(=引擎环境判不了:未设变量、非布尔、
  编译错)转成 `PredicateValue::Invalid` → 该分支 Failed(即上面的谓词失败路径),**不是** hop
  失败(否则 hop 会一直重试)。没有 runner 的 host(内核自带测试、无 Runtime 的嵌入式)回落原先
  自备子集求值器(变量/运算符/值方法),行为不变。headless scope 抽到 `kernel_host/scope.rs`,
  效果与谓词两个 seam 共用。
  对跑实测(`kernel_env_function_predicate_matches_the_legacy_run`):`$env.WORK_DIR != 'x'`
  的分支两侧都 Completed、`== 'x'` 的都 Skipped,逐节点一致。

- Back/重入的读模型对跑(旧队列「Back 的 legacy 折叠对照」项收口)+ 驱动档动作的 CAS 重试:
  - `kernel_back_read_model_serves_the_redone_generation`:同一 Back 流(irq 步 `act:back` 带
    `to=s1`)在两引擎各跑一次,取「redo 后、重做 act 再次 Created」时点的 task 行:legacy 行是
    per-instance(同一节点上旧 step `completed` + 旧 act `backed` + 新 step `running` + 新 act
    `interrupted`),kernel 行是 per-node-survivor(一节点一行 = 最新代:s1 `completed`、
    s2 `interrupted`,没有 backed/running 历史行)。两引擎只能在「存活实例」这层比对;kernel 的
    「一行 = 最新一代」就是行投影的契约,legacy 的「一行 = 一实例」是历史。按 timestamp 逐实例
    折叠不可靠(重入在同一毫秒内完成),所以不再做时间折叠。
  - 顺带修掉 e2e 暴露的驱动档真 bug:客户端动作在并发提交下会把 CAS 竞争(serve 循环同时在提交
    同一 aggregate 的 hop)抛给调用方。现在 `apply_action` 对 conflict 类失败有界重试(8 次,
    重读快照重新规划;**拒绝类**错误照旧立即返回),`run_reentry` 在自己内部重试(重入是
    cancel/back 的实质,丢了即 run 卡死;失败尝试留下的 carrier op 是惰性的 —— 其执行已决,hop
    提交空余量)。repository 失败按类型归一:`CommitError::VersionConflict`(stale plan)与 CAS
     `Conflict` 变体都归为 ECONFLICT 类(新增 `repository_error`,kernel 侧导出
    `AsyncRepositoryError`),执行 hop 的冲突语义不变(仍是留给 recovery 重推)。

 - 错误动作输入名对齐(catch 的另一前置):`Runtime::kernel_kind` 原来读 `code`/`message` 并各自
   给默认值,而 legacy 读的是 `ecode`(**必填**,缺了报 "cannot find 'ecode' in options")与
   `message` —— 并且 `$ecode()` 读的正是任务 data 里的 `ecode`。现在驱动档按 legacy 名映射
   (缺 `ecode` 同样报错),于是同一条 `act:error` 在两引擎落到同一节点、同一 code。注意:客户端
   的 `ecode`/`message` 会作为普通 scope 变量写进该执行(legacy `task.set_data(&ctx.vars())`
   同),所以 catch 分支的 `$ecode()` 从作用域链上就能读到失败步的 code —— catch 接入时不再需要
   额外的别名映射。
   对跑实测(`kernel_error_action_carries_the_legacy_inputs`):两侧 step 行的 `err` 字符串完全
   相等(`{"ecode":"boom","message":"boom-msg"}`)。

- **catch 导入与错误路由(本片,「剩余工作 1」的前半;timeout 仍拒绝)**

  (a) **导入**(`kernel_host/model.rs`):接受 `step.catches`(handler 是嵌套 Step,可带自己的
  branches/vars/再嵌套 catches)。handler 的 `if` 是它自己的选择(legacy `Step::init` 守卫),
  导入后在 fan-out 时(成功路径与失败路径都用)**引擎表达式环境**求值:True→Entry、False→Skip
  (handler 落 Skipped,即 legacy 的 skip)、判不了/求值为 null→首 hop Failed(复用
  `__branch_error` 机制);`$ecode()` 因此可判。仍拒绝:`timeouts`(见「剩余工作 1」)、同一 step
  同时声明 `branches`+`catches`(legacy 的 child 集含两者且都调度,import 只有一种 fan-out
  形状)、handler 的 `while`(需要 step 循环机)、branch 内 step 的 `if/while`(原样)。
  **新增拒绝**:嵌套 step/branch 必须有 id——legacy 的 `prepare_step_ids` 会在自己的 clone 上
  补随机 id,driver 从 model 取 id,未命名只会在 hop 里 wedge,故导入期即拒(`examples/catches/model.yml`
  的 catch step/branch 就没 id;driver 尚未实现 id 分配,已登记)。

  (b) **失败路由**(`KernelHost::catch_fanout`):包错误(`effect_plan` 的 runner `Err`)与客户端
  `act:error`(`Failure{code,message,ecode}`;`ecode` 按 `ActError` 变体取:Package→`""`、
  Exception→其 ecode,对齐 `Task::set_err` 写进任务数据的映射)都先问 catch:节点声明 catches 且该
  执行未在判定中 → **保持 Running**(legacy `Step::on_error`:`set_sign(ERROR)`+回到 Running)、
  写私有 `__sign=error`(raw——引擎事实,不进谓词空间,也不是 JSON)、写 `ecode`/`message`
  (legacy 任务数据名,`$ecode()` 读它)与 `error.code`/`error.message`,并把每个 handler 作为容器
  子执行准入(Entry/Skip,子带 `__sign=catch` = legacy `TASK_SIGN=CATCH`;`ContainerFanoutCommand`
  新增 `source_park` 字段,失败路由传 `Running`、成功路径传 `Waiting`);并发一条 `task.failed`
  记录(legacy 失败 act 自己的 Error 消息;key `failed:<pid>:<tid>:<gen>:<op-sequence>`,序数保证
  与后续 step 记录不撞键——提交期的 `OutboxAlreadyExists` 是硬错误)。

  (c) **catch join**(`ContainerPlanner::plan_catch_join`,纯函数 + 单测):CATCH 子全部结算且无
  Aborted 时——**全部 Skipped → 父 `Running→Failed`**(错误重新上浮:父自己的 PropagateTask hop
  走既有 upward 错误行走 + 下游 skip 级联,并补一条 step 自己的 `task.failed` 记录)、**有非
  Skipped → 父 `Running→Completed`**(错误被接住,传播继续到链后继)。source/parent 的判定全读
  聚合事实(`__sign`),与 spec 无关。

  (d) **动作守卫**:`apply_action` 预检里,声明 catches 的 step 在「判定中(`__sign=error`)」或
  「已派发 handler 子」时拒绝一切动作(legacy 折叠的 act 已终态,`do_action` 一律拒),不消费操作;
  `effect_plan` 对 raced 的 auto hop 返回 bare remainder(不 wedge、不二次完成)。

  (e) **顺序修正**:`effect_plan(None)` 原来「先 fan-out 后效果」,对 `uses`+容器 step 会跳过效果
  (legacy 顺序是 act 先完成/失败、step 再 `check_in_children` 调度 children)。现在效果先跑
  (失败即走失败路由),成功后再 fan-out,效果 writes 并入同一笔提交。

  **对跑实测**(同 workflow 双引擎,`run_catch_engine` 订阅 act 的 Created 消息后从测试体发
  `Next`/`Error`;legacy 侧读运行返回的活动进程,不读行——已结算 run 的行归 sweeper):
   - `kernel_catch_matches_legacy_when_the_step_completes_without_error`(成功路径,**唯一能
     与 legacy 终态全量对跑**的 catch 场景):step 的 act 成功完成 → handler 被无符号派发 →
     `$ecode()` 为 null → `if` 分支 Skipped、`else` 分支跑 msg 步 → step/根 Completed。两侧
     `assert_parity` 逐节点一致(kernel 侧断言 c1 Completed、b1 Skipped、b2/h2 Completed、s1
     Completed、h1 不存在)。
   - `kernel_catch_matches_legacy_when_the_error_is_caught`:irq step 收 `act:error(ecode=err1)`
     → handler 分支 `$ecode()=='err1'` 选中并跑 msg 步 → **两侧 handler 子树逐节点一致**
     (legacy c1/b1/h1 全 completed;kernel 同),且 legacy 的 c1 带 `Sign::CATCH`(与 kernel 的
     `__sign=catch` 同源);kernel 侧 s1/根 Completed。
   - `kernel_catch_matches_legacy_when_every_handler_skipped`:handler 带 `if $ecode()=='err1'`、
     错误码 err2 → 两侧 handler 都 Skipped;kernel 侧 s1 Failed、根 Failed、后继 s2 Skipped。
   - **实测到的 legacy 缺陷(重要)**:legacy 里「act 失败 + step 声明 catches」会**永久卡死**。
     失败的 act 从不跑自己的 `next`,其 propagation phase 停在 `None`,于是 step 的
     `check_uses_action` 门槛(`is_propagation_done`)永远 Stop,`auto_complete`——即「全部
     CATCH 子 skip → 错误上浮 / 有 CATCH 子完成 → step 完成」——**根本不可达**。实测证据(fold
     诊断):`s1:running sign=ERROR phase=None done=false`、act `error phase=None done=false`、
     根 `running`;handler 子树照常跑完(dispatch 来自 `Step::on_error`),但 step/根永不结算,
     后继永不调度(`s2` 甚至从不创建)。所以 catch 的 join 语义只能对准代码意图,不能与 legacy
     终态对跑;上面两个错误用例的 legacy 断言即钉住这一分歧(legacy: step/根 running;kernel:
     Completed/Failed)。
   - 顺带实测:legacy 客户端 `act:error` 会**跳过该 act 父 step 的链兄弟**;含 catch 的 step 之后
     成功时会重新调度那个被 skip 的后继(同节点两实例:Skip 历史 + Completed 存活)。kernel 不复制
     这一步跳(靠错误级联/正常传播),终态一致。
   - 聚合变量编码:谓词/效果读的变量空间要求 **JSON 文本**(`scope_vars` → `headless_scope` 用
     `from_str` 解码,raw 字符串会被静默丢弃,例:`err1` 不合法 → 变量缺席)。`__`-私有事实
     (`__sign`)保持 raw(不进谓词空间)。既有 `ActionPlanner` 把客户端**字符串**输入原样(raw)
     写入:容错读方(`json_text`/`input_vars`/projection)能读,但谓词读不到——本片未改这条既有
     路径,只在 catch 自己的写入上用 JSON 文本。
   - 已知边界(明确登记,未静默降级):`uses`+容器(branches/catches)的 **act 完成消息**仍不在
     fan-out 路径发(既有 fan-out 抢占 completion 所致;legacy 会发 act Completed)。

- **timeout 的导入与派发(本片,「剩余工作 1」收口)**
  (a) **导入**(`kernel_host/model.rs`):接受 `step.timeouts`(handler 是嵌套 Step,可带自己的
  uses/vars 以及 branches/catches,递归走同一套 place 规则)。分支的 `if` 是它的**窗口**——由
  派发器在引擎表达式环境上求值(`$cost_in(...)`/`$cost()`),故允许;`while` 拒绝(需要 step
  循环机)。**新增拒绝**:同一 step 同时声明 `catches`+`timeouts`——catch join 用一个子集决定
  父步,而 timeout 子也落进同一个 child 集,两个集合暂时分不开(同 branches+catches 的理由)。
  模型查找(`walk_step`/`find_step`)与容器形状查找现在都下钻 `timeouts`(handler 自己可以是
  容器)。
  (b) **时间事实**:新增每执行的持久 cost 起点 `__started`(JSON 毫秒,私有变量),在 start hop
  (`Ready → Running|Waiting`)那一笔原子提交里写入,**只为声明了 `timeouts` 的节点**写(它是内核
  事实,不进表达式/行)。没有动 `ExecutionSnapshot`/snapshot codec——聚合变量是既有机制。legacy
  的 `Task::cost()` 从任务**创建**计时,而内核的链执行在 seed 时就预建(创建时间 = 整个 run 的
  年龄),所以取「该执行开始自己的活」那一刻作锚(语义最近、durable 的对应物)。tick 的 cost =
  注入的 now − `__started`(缺省/时钟回退 → 0,不会凭空开窗)。
  (c) **tick 派发 = 一个自续期的持久操作** `OperationKind::DispatchTimeout`(codec tag **追加 8**,
  不重排;commit digtag 同步)。start hop 在同一笔提交里 arm 这个 timer;`Pending`/`RetryScheduled`
  在 recovery 扫描里**每 tick 都被 Wake**,所以 tick 节奏 = 引擎 tick(`config.tick_interval_secs`,
  默认 15s)= legacy 的 process tick。一轮 = legacy 的 `Step::on_timeout` +
  `dispatch_next_timeout_branch` + 分支自己的 `Step::init`:声明序里第一个「未 claim 且无 live
  执行」的分支,`Blocked`(首个未 claim 分支在飞)→ 本轮结束;**guard 在引擎表达式环境上求值**
  (headless scope + 覆盖层 `__cost` = 冻住的 cost):`True` → 派发(Entry,复用
  `ContainerPlanner::plan_fanout` 的单子形状),`False` → 本轮不派发也不 claim(下轮重评,
  纯读的 preflight 让 `Idle` 直接 `Ok(false)`、**不消耗 timer 操作**),`null`/`Invalid` → 该分支
  带 `__branch_error` 准 Entry、首 hop Failed(legacy 分支 init 求值错误)。派发那一笔里同时写
  owner 的 `__cost`(冻结值)与 `__timeouts`(one-shot 标记,与 legacy 同名同义),并在还有未
  claim 分支时 arm 下一个 timer;所有分支都 claim 完或 owner 已决 → timer 提交空余量 retire
  (不再唤醒)。**没有照抄 catch**:timeout 不是失败路由,park/失败/上浮都不参与。
  (d) **owner 等待**:legacy `auto_complete` 数**全部**子任务(timeout 子含),所以 act 已完成但
  还有 timeout 子在飞时,内核**不**在此 hop 完成该执行:act 的完成记录(含 act 消息)照写,执行
  `Running → Waiting` park(act 本来就在 Waiting 的则无需状态变更),由容器 join(全子终态且无
  Aborted)在最后一个子结算时完成并传播。收敛论证(未构造竞态测试):子先结算、park 后提交时
  park 的计划版本过期 → CAS 冲突重试后按「无 live 子」走正常完成,不会 wedge。
  (e) **表达式空间的例外**:`scope_vars` 放行 legacy 自己的 `__cost` 数据(`$cost()`/`$cost_in`
  读的正是它,legacy 从任务 data / 父链读),其余 `__` 私有事实(`__model`/`__auto`/`__owner`/
  `__sign`/`__started`/`__timeouts`)仍不进表达式。
  (f) **容器 join 的 park 标记(本片暴露的既有 bug)**:`plan_join` 原来只要求父在 `Waiting`,
  但 `Waiting` 也是「在等自己的 act」(legacy `Interrupt`)的状态 —— 于是 timeout 子一结算,
  它的传播 hop 就把**还在等 act 的 owner 判成 `Completed`**(对跑实测:内核侧 s1 在 t1 fire
  后立刻 Completed,t2 永远没机会 fire;legacy 侧 s1 仍 running)。修法:新增私有变量
  `__parked=children`(**独立于 catch 的 `__sign`**,理由见下),由**宿主**两条路径写
  (容器 fan-out `container_fanout_plan`、timeout/等待 park `park_for_children_plan`),
  `plan_join` 只判带标记的父。踩到的第二个坑:最初把 park 标记复用 `__sign`,而 **catch handler
  自己常常是容器**(它的 fan-out 会覆写 handler 的 `__sign`),把 catch join 要读的
  CATCH 标记冲掉 → 失败步永不结算(实测 `kernel_catch_matches_legacy_when_the_error_is_caught`
  的内核侧卡在 s1 Running)。分成两个键后 catch 路径不受影响。纯 planner 侧新增
  `parked_for_children` + `PARK_VAR`/`PARK_CHILDREN`,单测覆盖「未标记的 Waiting 父不 join、
  标记后才 join」。
  (g) **顺带修 `fanout_command` 的「已 fan-out」判定**:它原来数**所有** parent 相同的子执行,
  于是一个 step 上已经 fire 的 timeout 子会把它的 branches 误判成「已经 fan 过」→ branches
  永不 fan-out(legacy 是 step 跑到 children 时调度 branches,与 timeout 无关)。现在只数该容器
  形状声明的子节点。新增机制单测
  `a_timeout_branch_does_not_mask_the_branches_of_the_same_step` 钉住(branches+timeouts 同
  step:先 fire timeout 子、再完成 act → branches 仍 fan-out、step 等两套子、最后一个结算时 join)。
  **对跑实测**(`kernel_timeout_branches_match_the_legacy_run`,同 workflow 双引擎,`tick=1s`):
  s1 为 irq 步(act1 等待客户端)+ 两个 timeout(`$cost() >= 300` / `$cost() >= 1500`,各带 msg
  act);两引擎都按声明序先 fire t1(msg1)再 fire t2(msg2),act1 `act:complete` 后 s1/根/s2 结算
  且 run 结束。legacy 折叠取**每节点最新实例**(`latest_folded_legacy_states`:legacy 每 tick 给未
  开窗的分支重派一个 Skipped 实例,内核每节点只有一个执行),逐节点与 kernel **相等**
  (`assert_parity`)。内核侧机制另外单测收口(`the_timeout_timer_fires_a_branch_once_and_the_owner_waits_for_it`):
  开着的窗随 start hop 的 wake 立刻派发、one-shot 不重派、owner 的 act 完成后 park 等子、子结算后
  join 完成、owner 决出后 timer retire;`__cost` 经 scope 链可读。
  **已知边界(明确登记,未静默降级)**:①timer 的第一轮随 start hop 的 wake 立刻跑(不等 tick),
  所以「窗口本来就开着」的分支(`$cost() >= 0` 之类)比 legacy 早至多一个 tick fire;未开窗的
  行为与 legacy 一致(下个 tick 重评)。②`timeouts` + `catches` 同一 step 拒绝(理由见 (a))。
  ③timeout handler 的 `while` 拒绝;handler 内的 branches/catches 允许(容器机制通用)。
  ④投影口径:`__cost`/`__timeouts`/`__started` 是私有键,**不进** vars 行(legacy 会把
  `__cost`/`__timeouts` 写进任务 data 行)——属既有「私有键不入行」口径。
  ⑤owner 的 act 完成但 timeout 子在飞时,内核的行状态原先是 Waiting(legacy 是 Running):
  **已由 A′ 片修正**(park 统一改 `Running`)。

- **根的完成语义 = 容器 join(「剩余工作 1」收口;A′:park 状态改 `Running`)**
  (a) **park 状态统一为 `Running`**(不再 `Waiting`):legacy 的 step 在子任务结算前一直
  `running`(`auto_complete` 判的正是 running 的 step,实测 `__sign: IN_CHILDREN`),`Waiting`
  只留给「自己的 act 没人应答」= legacy `Interrupt`。改动:容器 fan-out 的 `source_park` 用
  `Running`(源若正 `Waiting`,即 act 刚被应答 → 同笔 `Waiting → Running`,对齐 legacy `next`
  回 Running);timeout/等子 park 同理(等待计划写 `Waiting → Running`);`plan_join` 判
  「`Running` + `__parked` 标记」,其 `from` 改为 `Running`。
  (b) **根 = 对种子链的容器**:`container_shape(root)` 返回 `Steps(&model.steps)` —— 头节点
  `Entry`(已存在 `Created` → Ready + ExecuteTask)、其余 `Queued`(保持 `Created`,由传播逐个
  接纳);根在自己的 hop 不再判 `Completed`,而是 park(`Running` + 标记),由 join 在**全部种子
  子执行**结算后完成并传播(rewind 产生的新代 parent 也是根,所以一起被等)。于是 `proc:get` 的
  proc 行在子任务在飞时读 `running`(legacy 同)。线性(无 `__model`)的 P1 仓不受影响:
  `fanout_command` 读不到 model 直接 None,P1 平链 parity 不变。
  (c) **`Queued` 子遇「已存在」不再被 readmit**:`GenerationAdmissionPolicy` 对已存在且
  `Created` 的子返回 `ReadmitExisting`(rewind 重建场景需要它),但 `Queued` 子由传播接纳;fan-out
  把它 `Created → Ready` 却不发任何操作,它会停在那儿(既没跑也挡住 join)。现在 `Queued` +
  已存在 → 原样保留。这同时修掉 rewind 后内部链 `Queued` 子被 readmit 的潜在 wedge。
  (d) **状态表新增 `Running → Cancelled`**:park 改 Running 后,客户端 Cancel 一个「子在飞」的
  父步(原先是 `Waiting → Cancelled`)需要这条转移 —— planner 早已允许(`ActionPlanner` 的
  `Ready|Running|Waiting + Cancel → Cancelled`),表此前更严。legacy 的依据差异:legacy 的 cancel
  处理器要求被撤销的 **step** 处于 biz-success(它是从 act 上溯到 step 的形态),与内核「动作直接
  指向节点」的形态不同 —— 属既有 divergence,park 前后都是内核独有的接受面,登记在案(见剩余工作)。
  **对跑实测**(`kernel_parent_state_matches_legacy_while_a_fired_branch_runs`):同 workflow
  (s0 init irq / s1 irq + timeout t1 带 irq act),先让 t1 fire、再完成 act1(子仍在飞)——
  此刻 **legacy 的 s1 step 任务与 legacy 根任务都是 `running`,内核的 s1 与 `$` 都是 `Running`**;
  随后完成 t1,两引擎同一套终态结算且 `assert_parity` 逐节点相等。机制侧新增
  `the_root_stays_running_until_its_seeded_chain_settled`(链头 act 未应答时根 `Running`、
  `process_completed` false;应答后链尾与根 `Completed`)。既有 branches / cancel(含 rewind
  readmit)/ catch / branch-selection 用例按新口径更新后全部转绿。
  **已知边界**:①根的 gen0 比它关闭的 gen 活得久(`host_all_terminal_generations_0` 因此排除根)
  —— 根是整条链(含 rewind 新代)的容器,是语义而非漏洞。②cancel 的 legacy 守卫差异见 (d)。
  ③`proc`/`task` 行状态现在与 legacy 同(原先「park 在 Waiting、legacy 在 Running」的偏差消失)。

- **消息 parity 收口:task 级消息 + 容器的 act 完成消息(队列 1+3 本片)**
  (a) **记录→消息映射**(`kernel_host/message.rs`):payload 的 `uses` 变为可选,`type` 取 `kind`
  (缺省 "act")。于是同一个 outbox 记录机制承载两种形状:act 形状(`uses` 步:uses/params/step)
  与 task 形状(nid/name/kind/inputs{model}/outputs)。
  (b) **谁发/何时发 = legacy `Runtime::initialize` 的 `on_task` 规则**(实测+源码):任务状态
  **不是 pending/running** 就发(`Ready|Interrupt → MessageState::Created`),且 `is_emit()` 为真;
  `Act::init` 关 emit、`Act::run` 开(所以 act 的 Created 只在 Irq 出现),`Branch::init` 关 emit
  且从不打开 ⇒ **branch 任务全程静默**。内核对应:`hop_message_payload`(uses → act payload,
  其余 → task payload,**branch 节点返回 None**),覆盖创建(`message.created`,现在对**所有**执行写,
  不再只看 has_package)、终态(complete/skip/fail/abort)、客户端动作(经 `attach_hop_message`,
  原先对无包执行直接返回、现改名为 `attach_hop_message`)、失败路径与 cascade hop(abort 也补发)。
  (c) **容器**:`uses`+branches/catches 的步在 **fan-out 那一笔**发 act 完成记录(legacy 的 act
  正好在那时完成);**无包容器**改在 **join 那一笔**发自己的 task 完成记录(`attach_join_message`;
  legacy 的 step 在 fan-out 时是 `running`,按 (b) 不发),键带 join 的 op sequence
  `completed:<pid>:<tid>:<gen>:<seq>`,与 fan-out 的 `completed:<pid>:<tid>:<gen>` 不撞键。
  (d) **根**:根现在既走 lifecycle 通道(on_start/on_complete)**也**在 on_message 上发 workflow
  类型的 task 消息 —— legacy 两者都做(实测断言两侧 workflow-type 消息均为 true)。
  **对跑实测 1**(`kernel_task_and_act_messages_match_the_legacy_run`,workflow 现含一个纯 step
  s3):两侧折叠集合逐项相等(`s1|created|true` / `s1|completed|true` / `s2|created|true` /
  `s2|completed|true` / `s3|created|false` / `s3|completed|false`,实测 dump 逐项打印核对),
  读模型行折叠相等,workflow 类型消息两侧都出现。
  **对跑实测 2**(`kernel_container_messages_match_the_legacy_run`:s1 纯 step+branch、s2
  uses+branch):容器自身的完成消息两侧折叠相等,且 branch 任务在两侧都静默。
  **已知边界**:①`Proc::state()`/lifecycle 的触发时点未变(仍是全执行终态),根在 on_message 上
  的消息由 (d) 补上。②act 的 *记录条数* 与 legacy 的 *任务条数* 不同(内核把 act 折进 step),
  折叠到(node, state, uses)后相等,与既有口径一致。

- **driver 端节点 id 分配(队列 1 收口)**
  (a) `ProcessSpec::from_workflow` 在导入前**克隆** model,并赋值 id —— 镜像 legacy `tree/build.rs`
  的 `prepare_workflow_ids`/`prepare_step_ids`/`prepare_branch_ids`:空的 step/branch/catch/timeout
  节点 → `utils::shortid()`,空 workflow id → `utils::longid()`(即每条消息的 `mid`);判定与 legacy
  同(只看 `is_empty()`)。**调用方的 model 不被改写**(legacy 同样是先 clone 再赋值);赋完 id 的副本
  既用于导入、也序列化进 `__model` —— 聚合即事实,hop 才能寻址。
  于是「未命名嵌套节点 → 导入期拒绝」只剩**空白(trim 后为空)**id 一种(legacy 会原样保留空白 id,
  内核拒绝替它发明语义;拒绝文案与注释已改)。
  (b) **实测**:`unnamed_nested_nodes_get_ids_like_the_legacy_tree`(catch + branch + 内层 step 全无名
  → 导入成功、`__model` 与 auto map 都带新 id、调用方 model 未被改写);`the_unnamed_catch_example_matches_the_legacy_run`
  **直接加载 `examples/catches/model.yml`**(`include_str!`,即发布的那份文件)双引擎对跑:完成 init
  与 work act(成功路径;两侧都**不**派发 handler,见上面的「catch 成功路径的修正」)→ 两引擎的 act
  key 集合都是 `{init, act1}` 且落定节点集合一致(`$`/prepare/step1/final),run 都结算。
  两引擎的节点 id 都是运行时随机(legacy 也是),故按 **act key** 比对(与既有「parity 不比 id」口径
  一致)。

- **catch 成功路径的修正(消息/id 片暴露:内核原先偏 legacy)**
  实测:legacy 的 tree builder 把 `catches` 建成 `NodeOutputKind::Catch` 子节点,而
  `Node::children()` 只回 `Normal` 类别 —— 所以 `check_in_children` **不会**访问 handler,
  **成功完成不会派发 handler**(只有 `Step::on_error` 在失败时派发)。
  实测证据:`examples/catches/model.yml` 成功路径的 legacy 任务树里**完全没有 handler/分支任务**
  (只有 workflow/prepare/step1/final 与其 act);而内核原先会 fan-out 它们,handler 的 `else`
  分支还真的会跑它的 msg 步骤 → 不只多出节点,**侧效应也分歧**。
  修正:`walk_step` 只把 step 的 `branches` 当容器形状(`ContainerShape::Catches` 变体与
  `fanout_command` 里那段 catch 构造整块删除;`catch_fanout` 自己按 `step.catches` 建子,不受影响)。
  catch 失败路径(接住/上浮)用例不变;成功路径用例
  `kernel_catch_matches_legacy_when_the_step_completes_without_error` 改成「两侧都没有 handler
  执行」——它原先的折叠比较对该子树是**空对空**(legacy 根本没建那些节点),所以没抓到这条分歧。
  ⚠️ 旧笔记与宿主注释里「`check_in_children` 调度 catch 子节点 ⇒ 成功也跑 handler」的说法**是错的**,
  以本条为准。

- **流事实归一(队列「B」选项;替代 `__parked` + `__sign=error`)**
  (a) `__flow`(私有)一个键、两个值:`children`(已 fan-out、等子结算 → 普通 join 判)与
  `catches`(失败已定、在判 catch → catch join 判);**缺省 = 本执行处于自己 act 的流**
  (act 在飞 = `Running`、等人应答 = `Waiting`、未开始)。读侧只经 `kernel::execution_flow()`
  (返回 `ExecutionFlow`)与两个谓词 `parked_for_children`/`deciding_catches`:`plan_join` 读
  `children`,`plan_catch_join` 读 `catches`。于是「`Running` + 有子」的两种流由**显式值**区分,
  不再依赖「没有标记」这种隐式规则(将来新增 park 路径也不会被误读成 catch 判定)。
  (b) `__sign` 只剩**角色**事实:`catch`(被派发的 handler 子,对应 legacy `TASK_SIGN=CATCH`),
  与 `__flow` 是**两条轴**(谁决定我 vs 我是谁的子),必须分键 —— handler 自己常常是容器,它的
  fan-out 只写 `__flow=children`,不再覆盖父 join 要读的角色(这就是本片之前踩到的 clobber 缺陷;
  新增回归测试 `a_handler_container_keeps_its_role_while_parked_for_children`:handler 保持
  `catch` 角色 + 自己进 `children` 流 + 普通 join 结它 + catch join 结失败步,全链跑通)。
  (c) **timeout 故意不进流值域**:timer 与 act 的流**并行**(act 在跑/在等时都会 fire),单值流键
  装不下它;「有个 timeout 在等」的准确表达就是 timer 操作本身(`DispatchTimeout`:持有节奏、
  由 recovery index 每 tick 重新唤醒)。这条写进 `FLOW_VAR` 的文档注释,免得下一个人再想合并。
  (d) 单测:`the_flow_fact_is_one_axis_with_two_values`(两值 + 缺省 + 未知值当缺省);
  `join_waits_for_running_children_and_never_fires_on_an_aborted_one` 增补「判定中的父不被普通
  join 判」;`catch_join_only_fires_for_a_flow_decided_step_and_its_catcher`(原 sign 版)改读新键并
  增补「parked 父不被 catch join 判」;宿主 catch/timeout/root/cancel/example 用例全绿。

- **timeout + catches 同一 step(队列 1 收口)**
  (a) **导入**:不再拒绝。同时修掉一条**已失效的拒绝**:`branches` + `catches` 同 step 原本被拒,
  理由是「一个 fan-out 形状不能同时表示两套子树」—— 但那条理由依赖「成功时 `check_in_children`
  也会派发 catch 子」这一**已被实测推翻**的前提(见「catch 成功路径的修正」)。两套子树其实分属两条
  **互斥**路径:branches 是 `Normal` 子(step 完成时 visit),catches 是 `Catch` 子(只有
  `Step::on_error` 派发),所以同 step 声明两者合法,现在允许 ✓。
  (b) **catch join 面向混合子集**(`plan_catch_join`):
  ①**触发**从「settling 的是 CATCH 子」放宽为「settling 的是本步自己的子」—— 声明了 timeouts 的
  步,最后落定的那个子往往就是被 tick fire 的 timeout 分支,它必须能触发结算;
  ②**门槛**仍覆盖**全部**子(任一非终态 → 不判;任一 Aborted → 让位给 abort walk);
  ③**投票只数 `__sign=catch` 的 handler 子**(legacy `is_empty_catched` 的 sign 过滤):全部 Skipped
  → Failed(错误上浮),有非 Skipped → Completed(接住);**无** handler 子则拒绝猜(返回 None);
  ④`__flow=catches` 仍是「这一步在判 catch」的唯一事实,与 `__flow=children` 的普通容器互斥。
  判定期间 timeout 子照旧由 tick 派发/等待(legacy 的 tick 判据只看 Running/Interrupt)。
  (c) **实测**:纯 planner `catch_join_decides_a_mixed_child_set_from_its_handlers`(接住/上浮两种
  投票 × timeout 子落定/在飞 × 外来执行结算不触发 × 无 handler 子拒绝);
  `catch_join_only_fires_for_a_flow_decided_step_and_its_catcher` 改按新契约(角色不再卡触发、
  无角色子不投票);导入单测覆盖两套声明同时导入(auto map 两套都在);
  端到端(内核档、真 tick)`a_failed_step_waits_for_its_timeout_branch_before_the_catch_join`:
  t1 先被 tick fire → act1 报错(ecode `E1`)→ handler 被选中并跑到等待 → 完成 handler 后 **step
  仍 `Running`**(timeout 子还在飞,门槛生效)→ 完成 t1 后由 **t1 自己的结算**触发 catch join →
  step `Completed`、run 结算。
  与 legacy 的终态对跑仍不可能:legacy 的失败 act 不跑自己的 `next`,step 永不结算(既有登记)。

- **timer 的第一轮不再早于 tick(队列 2 收口)**:arm 那次 hop 之后,新操作会被宿主统一唤醒
  (`receipt.new_operations`),于是「窗口本来就开着」的分支(`$cost() >= 0` 之类)会比 legacy
  早至多一个 tick fire。修法在**唤醒路径**上区分(**不新增持久事实**):`execute_one` 收尾的唤醒
  跳过「本次 hop 是 `ExecuteTask`(执行起点)刚 arm 的那个 `DispatchTimeout`」—— 起点 arm 的 timer
  只由 recovery 扫描(每 tick)唤醒,与 legacy「第一个 tick 才有第一轮」一致;被**一次 round**
  重新 arm 的 timer 仍被立刻唤醒,保住 legacy `Step::next` 的链式交接(上一分支结算后立即评估
  下一分支)。单测两处改为「先断言 tick 前无派发,再 `sleep 2ms` + `recover` 驱动一次 tick」。
  (两条走过的弯路记在此以免重走:①「首轮提交 arm 标记 + 替补 timer」—— 替补同样被宿主立刻唤醒,
  标记挡不住那次评估;②「`cost_ms == 0` 才 Idle」—— 真实 hop 序列耗时已过毫秒,等于没生效。)

- **重入做成持久操作(「剩余工作 1」settle 的前置修复;本片)**:驱动档的 Cancel/Back 重入原先由
  宿主侧 `run_reentry` 在效果提交**之后**单独规划(每笔 cancel 还要多 3~4 笔提交),与 serve 循环的
  hop 抢同一 aggregate ⇒ 重试风暴。实测 trace(上一笔 pinning 测试 5/6 红):
  ①效果提交成功但随后的 `Complete` 事件丢 CAS → 重试的 preflight 看到自己刚改过的状态,把**成功
  报成拒绝**(`ActionNotAllowed{state: Cancelled}`);②`run_reentry` 的 8 次重试被并发 hop 耗尽 ⇒
  **重入丢失**(没有新代,run 卡死);③每次失败留下的 carrier op 堆积 ⇒ 仓库抛
  `RecoveryIndexMismatch{process_id}`(硬完整性错误)。
  修法:新增 `OperationKind::ReenterTask`(codec tag **追加 9**,不重排;`commit.rs` 的 digtag 同步)
  —— 决定那一笔**原子提交**里同时写 `__reenter_target`(私有事实:`Back`=客户端 `to`,`Cancel`=被
  关闭的执行自身)并建这个 op;其 hop 跑 `GenerationPlanner::plan_reentry` 并提交(丢 CAS 由
  recovery 收敛),宿主侧 `run_reentry`/`run_reentry_once` 整体删除。`apply_action` 另外**钉住
  intent 身份**(首个 attempt 定的 execution+operation id):重试先看这条 op —— 已
  `Committed`/`Completed` ⇒ 直接 `Ok`(自己的提交永不报成拒绝),仍在 `Claimed`/`CommitPrepared`
  ⇒ 先 `ScheduleRetry` 再复用同一条 op(claim fence = attempt+1),不从已移动的快照上重新推导 id。
  顺带:`Back` 缺 `to` 的拒绝移到 durable intent **之前**(对齐 legacy 在 `back_task` 之前读 `to`);
  旧测试 `back_without_to_is_refused_without_consuming_state` 钉的是「效果已落盘、再报错」的旧行为,
  已改为「拒绝即不消费聚合版本、步骤仍在 Waiting」。
  实测:pinning 用例 8/8 绿(修复前 5/6 红),全量 `-p acts --lib --tests` **1260 passed**(只剩已登记
  的 stall 红),该用例耗时从 ~1.8s 降到 ~0.17s(重试风暴消失)。

- **Cancel 守卫对齐 legacy(「剩余工作 1」收口;本片)** —— 内核不再有「Cancel 独有的接受面」。
  (a) **实测 legacy**(临时脚本单跑 legacy,逐任务 dump;`Cancel` 处理器源码 `task.rs:820-883`):
      处理器从 act 上溯到它的 parent step,要求该 step `is_biz_success`(Completed/Submitted/Backed);
      再 `follows(is Step && is_acts)` 收集**链上**(走 `next`,容器 children 不算)前向的 uses 步:
      空 ⇒ `cannot find cancelled tasks`,全部 `is_completed` ⇒ `not allowed to cancel`(重复投递守卫)。
      实测(2 个 irq 步,act1 完成后 act2 在等):
      - 接受流:cancel(act1 的 tid)→ Ok;**旧的 act1/step1 保持 Completed**、**新增一个 step1 实例**
        (Running + 新 act Interrupt)、**act2 → Cancelled**、**step2 → Completed**;答 redo act1 与新的
        act2 后整条 run 结算(根 Completed)。
      - 整条 run 结算后再 cancel ⇒ `task('<step>') is not allowed to cancel`;
      - 链上没有前向 uses 步(唯一一个 irq 步)⇒ `cannot find cancelled tasks`。
  (b) **内核接受面**(`ActionPlanner::action_target`):`(Completed|Submitted) + Cancel → Backed`,其余
      状态一律 `ActionNotAllowed`。被撤销的那一代取 legacy **act 侧的事实 `Backed`**(`plan_reentry`
      的 source 也接受它);状态表删掉不再有生产者的 `(Ready|Running|Waiting) → Cancelled`,加
      `(Completed|Submitted) → Backed`。(`Cancelled` 状态与 codec 保留,见下面的边界 ③。)
  (c) **前向守卫**(`KernelHost::validate_cancel_forward_work`,durable intent 之前的 preflight):沿
      **链**走 source 那一代的前向闭包(容器 child 用 `parent == current` 排除 —— legacy 的 `follows`
      只走 `next`),取 uses 节点(无 `__model` 的线性 spec 是 legacy 表达不了的形状 ⇒ 回落「任一前向
      执行还在飞」的精神);空 ⇒ `cannot find cancelled tasks`,全终态 ⇒ `not allowed to cancel`
      (与 legacy 文案一致,拒绝不消费聚合版本)。
  (d) **对跑实测**:`kernel_cancel_matches_the_legacy_rewind`(同 workflow + 同一客户端脚本,两引擎
      cancel 都 Ok;结算后 `assert_parity` 逐节点相等,s1 两代、b1/s11 各两代)与
      `a_cancel_needs_a_finished_step_with_live_forward_work`(三条拒绝);
      `legacy_cancel_refuses_a_step_whose_work_is_not_done` 改为**两侧都拒绝**(分歧消失,内核侧另断言
      「拒绝不消费、s1 仍只有一代」)。既有「在飞 cancel」用例按新口径重写:
      `cancelling_a_finished_container_rewinds_its_branches`、
      `cancelling_a_finished_container_readmits_the_rewound_branches`(readmission 仍覆盖)、
      `cancelled_head_cascades_skip_through_the_chain`(线性 spec:头与下游步都设为手动;head gen0
      现在断言 `Backed`)。
  **登记边界(未静默降级)**:①被撤销那一代的**前向旧代**在内核是 `Skipped`(死源级联),legacy 的对应
      事实是前向 act `Cancelled` + 它所属 step `Completed`;parity 用「每节点最新一代」口径(两引擎在
      那一层相等),旧代的差异登记在此。②被撤销节点自身:内核把该代记 `Backed`(legacy act 的事实),
      legacy 的 step 事实(Completed)折叠不进同一个节点行。③`Cancelled` 不再由客户端动作产生。
      ④`KernelHost` 的「动作 intent op 不带动作载荷」老洞仍在:见剩余工作 1。

- **`load()` 的多键读竞态收口(上一片登记项;本片)**:`AsyncAggregateRepository::load` 一次
  `read_many` 读 aggregate + recovery index + outbox index,而 `KvStore::many` 的默认实现逐个
  `one()`(`MemoryStore` 未覆写 ⇒ 三次独立加读锁):并发提交落在两次读之间时,三个文档来自不同
  版本,`decode_loaded_aggregate` 抛 `RecoveryIndexMismatch`/`OutboxIndexMismatch` 被当成硬完整性
  错误抛给调用方(上一片的重入重试风暴里实测过一次)。修法:`DocumentRepositoryError::is_read_race()`
  认定这两个错误 = 「读到跨写入的混合版本」,loader 有限重读(`LOAD_ATTEMPTS = 4`);健康的读一次
  成功,真损坏重读耗尽后照旧上抛。单测两处(脚本化 store 供第一/前 N 次混合三元组):
  `load_retries_a_read_that_raced_a_writer`(混合 → 重读 → 一致版本,2 次读)与
  `load_surfaces_a_mismatch_that_never_heals`(始终混合 → 抛错,读了 LOAD_ATTEMPTS 次)。
  说明:这条只能单测钉住——e2e 触发需要「提交恰好落在两次读之间」,不可确定性复现;e2e 侧只保证
  正常路径不受影响(全量 `-p acts --lib --tests` 绿)。

- **动作 intent 自带动作载荷(再上一片登记项;本片)**:客户端动作的 durable intent 原先只是
  `OperationKind::ApplyAction`(无载荷),于是崩溃/租约过期后由 recovery 驱那条 hop 时走
  `effect_plan(None)` = **自动完成**,而不是客户端原本的动作(Fail/Abort/Skip/Submit/Cancel 的
  「决定」本身会漂)。修法:deciding 那一笔除了建 op,还把动作与 inputs 写成私有事实
  `__action@<op-sequence>`(JSON 文本:`{"kind":{"action":"fail","code":…,"message":…},"inputs":[["k","v"],…]}`);
  hop 处理 `ApplyAction` 时按**自己的 sequence** 读它 → 走同一个 `effect_plan(Some(kind), inputs)`
  (与客户端路径同一条 planner,含 Cancel/Back 的 `ReenterTask` 接线);读不到(rewind 留下的惰性
  carrier)才回落自动完成。key 带 op sequence:同一执行上后来的动作不会被当成这条 op 的。
  实测:`a_recovered_action_intent_applies_the_action_not_a_completion` —— 按 deciding 的形状手工提交
  Fail intent,deciding 调用不跑、只靠 `recover` 驱 hop:s1 → Failed、`error.code = E9`、s2 被错误
  行走 Skip;同一测试后半段钉住**回落**(无 fact 的裸 `ApplyAction` op 仍走自动完成、run 正常结算)。

## 已对齐(无需动)

- Complete/Submit/Skip/Fail on Running/Waiting:ActionPlanner 与 legacy 守卫一致
- 终态任务上的动作:双方均拒绝且不消耗
- 动作身份:legacy act 随机 tid → 上溯 step 的 node id(runtime.rs `shadow_action`)
- Waiting ↔ legacy Interrupt 语义对应;Back 仅 Waiting(与 legacy Back 需先挂起一致)

## 剩余工作(按序)

(空)`load()` 竞态(上一片登记)与动作 intent 载荷(再上一片登记)均已收口;新发现按上面「已完成」
的惯例登记在前,再排期。

**第二轮(去 legacy:删 cache/scheduler、合并 kernel_host、全面切换)的计划、方案与提示词
已移到 `KERNEL_CUTOVER_NOTES.md`;本文件的语义结论仍是那边的行为依据,勿删。**

## 语义结论(已核实,勿再猜测)

- legacy `TaskState::Interrupt` ↔ kernel `ExecutionState::Waiting`;
  Back 仅 Waiting 合法(action_target)。
- legacy Cancel = 回退重做(task.rs:820-883):**守卫** = act 的 parent step 处于 biz-success
  (Completed/Submitted/Backed),且链上(`follows` 走 `next`)要有 uses 步、不全是终态(否则
  `cannot find cancelled tasks` / `not allowed to cancel`);**效果** = 被撤销 act 所属 step 新增一代
  (redo;旧 step 保持 Completed)+ 前向链的 act → Cancelled、其 step → Completed;被撤销的那个 act
  实测保持 Completed。内核对应:目标节点 `Completed|Submitted` → `Backed` + `ReenterTask` 重入
  (与 Back 共用同一条重入),前向旧代走死源级联 → Skipped(登记差异,见「Cancel 守卫对齐」片)。
  legacy Abort = 终止链路(祖先 Aborted + 子孙 Skipped)。
- join:Skipped 不算 success(死路传播);Backed 等待;JoinBlocked 错误已删除。
- 容器 join(branch/分支步)与链 join 不同:legacy `auto_complete` 数**全终态**子
  (Skipped 算已结算)且任一子 Aborted 时让位;kernel `plan_join` 同——Skipped 分支
  不会杀死父容器。
- legacy 分支节点:全部兄弟分支都建任务(init 时按 if 跳掉),未选中分支的内部
  步骤不建任务;else 仅当所有 if 兄弟皆 skip 才运行(kernel `BranchPlanner::select`
  同语义:全 True 入选,无 True 才落 else,全不中 = NoMatch)。
- branch 谓词的可见变量:legacy = 任务 scope 链 + 已写的 action vars;kernel =
  聚合 scope 链(近者胜)+ fan-out 动作 inputs 覆盖层。
- legacy Abort:`abort_task` 祖先全链无条件改写 Aborted(终态不豁免,根含),
  首层兄弟 Skipped,祖先的 running 子 Aborted、pending 子 Skipped;kernel 对应
  upward→Aborted + AbortTask hop。
- legacy Error(no catch):`set_err` 把 act、step、根的整条祖先链改写 Error;
  step 的兄弟(下游链)Skipped;kernel 对应 upward→Failed + SkipTask 级联。
- legacy Back:`to` 命中 prev 链上的历史 step;redo 该步(每 (node,prev) 一个
  存活 redo),current→Backed、兄弟 Skipped、path running→Completed/pending→
  Skipped,redo 的传播为下游 step 建新实例(重跑);kernel 对应闭包根=to 最新代
  + Backed 源必须在闭包内 + 下游新代由传播重接纳。
- 内核动作身份 = 模型节点 id;legacy 任务 id 运行时随机——parity 比完成态与
  每模型节点一个终态任务,不比 id。决策步(abort/error)的 parity 比对:
  legacy 折叠(acts 入 step、根→$、Interrupt→Waiting、Error→Failed)后逐节点
  相等,kernel 多出的预建链节点须全部终态。
- legacy 任务消息流(实测 type=step/act/workflow):step 任务与 workflow 根也发 task
  消息(emit 默认开、非 pending/running 即发);act 的 Created 只在 Irq 出现
  (Func/Msg 的 run 返回时状态仍是 Running,被 handler 跳过),act 的 Completed 与
  对应的 step Completed 都发。kernel 把 act 折进 step、只发 act 消息;折叠到
  (节点, 状态, 有无包)后与 legacy 逐项相等(见本片实测),根节点消息走 lifecycle
  通道 on_start/on_complete。
- 行投影口径:一行 = 一个模型节点(最新代);task 行 id = pid+tid(无分隔符),根行
  tid=`$`、node_data.id = 模型节点 id(与 legacy 同形);legacy 的 step/act tid 都是
  随机 id、act 行的 `parent` 指向 step 的 tid,跨引擎比较须用 node_data.id(act 折到
  其 parent 行的 node_data.id)。legacy 行滞后于完成信号(写分片队列),对跑前要先
  `cache.flush()`。
- 任务状态字符串:`TaskState` 经 `From<TaskState> for String` 落小写(`Interrupt` →
  "interrupted");行投影出的 proc 状态在根 Ready/Running/Waiting 时是 "running"、
  根 Created 时 "none"、终态取根自己的终态。
- legacy 分支谓词在 `Branch::init`(**不是** run)里求值,所以失败是 `Ready → Error`
  (任务根本没进 Running);kernel 状态表据此新增 `Ready → Failed`。
- legacy catch/timeout(源码 `scheduler/process/task/step.rs` + `tree/build.rs`):`Step.catches`
  建成 `NodeOutputKind::Catch` 子节点、`Step.timeouts` 建成 Timeout 子节点;`Step::on_error`
  在这些子节点存在且任务还没有 sign 时 `set_sign(Sign::ERROR)`、把该 step 状态改回 Running、
  以 `TASK_SIGN = CATCH` 逐个调度 Catch 子节点,然后才 `emit_error` 继续上浮;`auto_complete`
  的判据是「该 step 有 ERROR sign 且全部 CATCH 子节点都 Skipped」→ 恢复错误(继续上浮),
  否则该 step 完成(错误被接住)。catch 分支用 `$ecode()`(读任务 data 的 `ecode`)选错误码。
- **catch 的四个实测事实(本片)**:①CATCH 子节点是 handler(Step,可再带 branches),其 `if`
  在它自己的 `Step::init` 求值,失败即 Skipped;②`$ecode()`/`$ECODE` 来自*该任务 data 的
  `ecode`*,handler 的 data 由被失败 step 的 outputs(含 act 错误时折进去的 `ecode`/`message`)
  经 prev 链喂入;③**handler 子树能跑,但 step 永不结算**:失败的 act 从不跑自己的 `next`,
  其 propagation phase 停在 `None`,`check_uses_action` 的 `is_propagation_done` 门槛永远
  Stop → `auto_complete` 不可达(实测:s1 `running`、act `error phase=None done=false`、根
  `running`;后继 step 甚至从不创建)——所以上面那条「全部 skip → 上浮 / 有完成 → 接住」的
  代码意图在 legacy 里**不可达**,kernel 照代码意图实现(见本片记录);④客户端 `act:error`
  会跳过该 act 父 step 的链兄弟,step 之后成功时那个后继被重新调度(同节点多实例)。
- catch 的 kernel 事实:`__sign=error`(判定中的 step)与 `__sign=catch`(被派发的 handler)
  是聚合里的私有事实(handler 上的对应 legacy `TASK_SIGN=CATCH` 已实测),catch join 完全
  读它们;判定只由 catch join 做,`Running → Completed`(接住)/`Running → Failed`(上浮)
  都在状态表内。
- **timeout 的实测事实**:
  ①legacy `Process::do_tick` 只对 `is_timeouts()`(NodeContent::Step 且 `timeouts` 非空)且
  `running || interrupted` 的任务派发;`Step::on_timeout` 先 `task.set_data(__cost = task.cost())`
  (即 `$cost()`/`$cost_in()` 读的是冻结值),再 `dispatch_next_timeout_branch`。
  ②派发规则(实测 `sch_step_timeout_many` 的 proc dump):声明序里第一个**未 claim 且不在飞**
  的分支被 `schedule_once`;分支自己的 `Step::init` 才求值窗口(`if`),不成立就 Skipped 且
  **不 claim**,于是下一个 tick 又派一次(实测同一节点出现 Skipped + Completed 两个实例,
  间隔 = tick 周期);已 claim 的分支跳过,不在飞检查只针对候选分支自己。
  ③claim 标记 `__timeouts`(Vec<String>)与冻结的 `__cost` 都写在**owner(被计时 step)的
  task data** 里(实测 `data={"__cost":1617,"__timeouts":["timout1"]}`),即聚合事实的同名同义物。
  ④分支读 `$cost()` 走 **data 的父链**(`env::functions::current_cost`:先本任务 data 的 `__cost`、
  再逐级父任务的 `__cost`、最后 `task.cost()`),所以 timeout 子读到的就是 owner 的冻结 cost。
  ⑤legacy `auto_complete` 数**全部**子任务(`Process::children` = parent_id 相同者,timeout 子
  含),`is_completed()` 含 Skipped/Error/Aborted;所以 act 完成但 timeout 子在飞时 step 不结算
  (内核对应 park + join)。
  ⑥`timeouts` 是 NodeOutputKind::Timeout 子节点,`Node::children()`(Normal)看不到它们——
  `check_in_children` 不会调度它们,只有 tick 会。
  ⑦分支自己的 `next` 在完成时会调 `dispatch_next_timeout_branch` 交接给下一个分支;skip 的
  分支直接 Parent(不交接)。内核不需要这条链:每 tick 重评列表即可。
- **task 消息的发出规则(实测 + 源码)**:legacy `Runtime::initialize` 的 `on_task` 在任务状态
  **不是 pending/running** 且 `is_emit()` 为真时,用 `Task::create_message` 发一条消息到
  `on_message`(type = `node.kind()`,`uses` = `node.uses()`,所以*带 uses 的 step* 的消息也带
  uses → 折叠口径下与 act 消息同形)。`Ready|Interrupt → Created`;`Act::init` 关 emit、`run` 开
  (⇒ act 的 Created 只在 Irq 出现),`Branch::init` 关 emit 且从不打开(⇒ branch 任务静默,
  只有其内部 step 发)。根任务既走 `on_task`(on_message,type=workflow)也走 `on_proc`
  (on_start/on_complete)。内核:同一形状的记录由每个 hop 的 outbox 承载,`branch` 节点不写 payload。
- **父状态不变式(实测)**:legacy 的父任务(step 与 workflow 根)在**子任务结算前一直 `running`**
  (`check_in_children` 之后 step 的 `next` 返回 Stop、`auto_complete` 只在全子 completed 时结算;
  实测 step `state=running` + `__sign: IN_CHILDREN`,根 `running`),`Interrupt` 只表示「自己的 act
  没人应答」。内核对应:容器 park 用 `Running` + 私有 `__parked=children` 标记,`Waiting` 仅留给
  未应答的 act;`plan_join` 只判 `Running`+标记,所以子任务结算不会去判一个「还在等 act」的执行。
  根是「对种子链(含 rewind 新代)的容器」,同样按此结算。

## 环境注意

本机 Defender(TQDefender)在并行构建窗口锁 rlib 元文件 → examples 全量构建偶发
E0786/E0463。规避:串行构建;测试收口用 `cargo test -p acts --lib --tests`;
examples 链接验证用 `cargo build -p acts --example <name>`(单跑均成功)。

全量 `--lib` 跑时 legacy reliability 套件对负载敏感:除上面点名的
`scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`,
`...::error::sch_error_duplicate_delivery_is_terminal_noop` 也在全量负载下偶发失败。
catch 片实测:该 stall 红在**改动 stash 掉的基线**上单跑同样失败(1/1 failed),所以它与内核
改动无关;判定回归时以「stash 基线对照 + 与本次改动是否同路径」为准,不要只看单测复跑。

timeout 片实测补充:`kernel_host::tests::kernel_catch_*` 这批「双引擎 + 真 tick」的用例在**同
进程并行**跑时会翻 —— 翻的是 **legacy 侧**断言(`legacy_node_states` 读到 `["none"]`,或失败 act
的 error 竞过 auto_complete 把 legacy 判成 completed),同批单跑即绿;这类翻转与本次改动不同路径
(legacy scheduler 未动),按上面的口径判定。

流归一/item-1 片实测补充:同类翻转也在 `kernel_host::tests::kernel_error_action_carries_the_legacy_inputs`
上出现过。同一棵树连跑三次全量:两次各带一个旋转的 dual-engine 红(单跑均绿)、一次
**1258 passed / 只剩 stall 那条**。判定以「单跑绿 + 至少一次干净全量」为准。
