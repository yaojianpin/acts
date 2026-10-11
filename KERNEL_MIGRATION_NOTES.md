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

## 已对齐(无需动)

- Complete/Submit/Skip/Fail on Running/Waiting:ActionPlanner 与 legacy 守卫一致
- 终态任务上的动作:双方均拒绝且不消耗
- 动作身份:legacy act 随机 tid → 上溯 step 的 node id(runtime.rs `shadow_action`)
- Waiting ↔ legacy Interrupt 语义对应;Back 仅 Waiting(与 legacy Back 需先挂起一致)

## 剩余工作(按序)

1. **catch/timeout 的导入与错误路由**(本片只拒绝、未导入):legacy 的语义 = `Step::on_error`
   把 Catch 子节点按 `TASK_SIGN = CATCH` 调度、该 step 回到 Running,`auto_complete` 再判:
   全部 CATCH 子 skip → 错误继续上浮,有 CATCH 子完成 → 该 step 完成(错误被接住);
   `timeouts` 同形(`Step::on_timeout`,先落 `$cost()`)。**前置已就绪**:谓词求值已在引擎表达式
   环境上(见上),catch 分支用 `$ecode()` 选错误码(`examples/catches/model.yml`)可判;错误动作的
   `ecode`/`message` 输入名也已对齐(见上),`ecode` 作为普通 scope 变量落在失败步上,作用域链即可
   读到 —— 接入 catch 只剩准入/路由本身。
   接入方案(已推演,未实施):(i) 失败 hop 发现该节点声明了 catch → 不再走 `failure_plan`,
   而是**保持该执行的 Running**、写错误变量,并把每个 catch step 作为容器子执行准入(Entry,
   各自内部链由它自己的 fan-out 接管);(ii) headless 谓词作用域要把错误变量按 legacy 名
   (`ecode`/`message`)喂进 headless task 的 data,`$ecode()` 才判得了(catch 的全部选择逻辑
   都在谓词里);(iii) 容器 join 要 catch-aware:父执行带错误变量且**全部 CATCH 子 Skipped** →
   父 Failed + 向上错误行走;有 CATCH 子 Completed → 父 Completed(错误被接住)。(iv) 先量
   legacy 一个最小 catch 场景(irq 步收 `act:error` 带 ecode → catch 分支 `$ecode()=='err1'`
   跑 msg 步)的折叠终态,再照抄。
2. **workflow 根的完成语义与 legacy 不一致(本片 e2e 发现,登记)**:kernel 在根自己的 hop 就把它
   判 `Completed`(根不是 fan-out 容器,`container_shape(root)` 返回 None),legacy 的根要等它
   **全部子实例**结算(Back/重入后根仍是 Running)。后果:驱动档下重入在飞时,`proc:get` 的
   proc 行状态(投影取根的态)显示 completed,与 legacy 的 running 不符。修法方向:让种子出来的
   根按容器/join 语义结算(根 park 等子结算,或 join 覆盖根),单独一片 + 对跑;`kernel_back_read_model_serves_the_redone_generation`
   里已留了「不再往前断言根」的注释位。
3. (登记,未排期)**无 `uses` 的 task 级消息**:纯 step 与 workflow 根在 legacy 也发 task
   消息(见「语义结论」);driver 档目前只发 act 消息,根走 lifecycle 通道。若客户端按
   type=step/workflow 订阅,需给无包执行也写消息记录、并把节点种类带进
   `message_from_record`(其 type 目前恒为 "act")。

## 语义结论(已核实,勿再猜测)

- legacy `TaskState::Interrupt` ↔ kernel `ExecutionState::Waiting`;
  Back 仅 Waiting 合法(action_target)。
- legacy Cancel = 回退重做(task.rs:820-883):redos step 新一代 + 前向 undo/skip;
  已由重入实现。legacy Abort = 终止链路(祖先 Aborted + 子孙 Skipped)。
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

## 环境注意

本机 Defender(TQDefender)在并行构建窗口锁 rlib 元文件 → examples 全量构建偶发
E0786/E0463。规避:串行构建;测试收口用 `cargo test -p acts --lib --tests`;
examples 链接验证用 `cargo build -p acts --example <name>`(单跑均成功)。

全量 `--lib` 跑时 legacy reliability 套件对负载敏感:除上面点名的
`scheduler::tests::reliability::stall::sch_next_cut_sweep_recovers_every_write_boundary`,
`...::error::sch_error_duplicate_delivery_is_terminal_noop` 也在全量负载下偶发失败
(单独跑 3/3 通过)。判定回归时以「单测复跑 + 与本次改动是否同路径」为准。
