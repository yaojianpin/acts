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

## 已对齐(无需动)

- Complete/Submit/Skip/Fail on Running/Waiting:ActionPlanner 与 legacy 守卫一致
- 终态任务上的动作:双方均拒绝且不消耗
- 动作身份:legacy act 随机 tid → 上溯 step 的 node id(runtime.rs `shadow_action`)
- Waiting ↔ legacy Interrupt 语义对应;Back 仅 Waiting(与 legacy Back 需先挂起一致)

## 剩余工作(按序)

1. **executor 移植(行投影+重启判别的真前置,拆多片)**:已落效果接缝
   (effects.rs)、act 消息记录、transport 接线与 Message 映射、效果参数对聚合
   变量求值、效果参数 sealed/快照对齐(owner + legacy `ctx.prepare()` 路径)。
   后续片:运行时接线(`Config.kernel_engine` 驱动档:legacy 不启动,kernel
   驱动,emitter/IO 包此时才接)——之后才轮到 行投影 + 重启判别:proc/task/vars
   行改为 CommitReceipt 投影,重启判别按 `kernel/aggregates/<pid>` 存在性
   (`AsyncAggregateRepository::load`)优先于 proc 行。
2. **谓词求值失败的失败路径**:legacy 分支 `if` 求值出错→任务 Error;阴影当前
   InvalidDecision 留存重试(可见但无 catch)。与 Error×catch parity 一并处理
   (catch 分支同样未导入)。
3. **Back 的 legacy 折叠对照**:重入流里 legacy 同节点多任务多状态,折叠规则
   依赖行投影的 receipt 语义,随第 1 项一并收口。

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

## 环境注意

本机 Defender(TQDefender)在并行构建窗口锁 rlib 元文件 → examples 全量构建偶发
E0786/E0463。规避:串行构建;测试收口用 `cargo test -p acts --lib --tests`;
examples 链接验证用 `cargo build -p acts --example <name>`(单跑均成功)。
