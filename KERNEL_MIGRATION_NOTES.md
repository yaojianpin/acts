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
  前驱边重映射,auto 变量随代继承);`validate_preconditions` 变量写 scope 允许指向
  本计划 creates 的新执行;宿主 Cancel 分派在级联 settle 后发 ReenterCommand

## 已对齐(无需动)

- Complete/Submit/Skip/Fail on Running/Waiting:ActionPlanner 与 legacy 守卫一致
- 终态任务上的动作:双方均拒绝且不消耗
- 动作身份:legacy act 随机 tid → 上溯 step 的 node id(runtime.rs `shadow_action`)
- Waiting ↔ legacy Interrupt 语义对应;Back 仅 Waiting(与 legacy Back 需先挂起一致)

## 剩余工作(按序)

1. **嵌套模型导入**:`ProcessSpec::from_workflow` 目前拒绝 branches;需
   branches → `ExecutionCreate`(branch if 选择语义:未选中分支不建或 Skipped)+
   `GenerationAdmissionPolicy`。重入的前向闭包重建(generation.rs)已验证
   creates+变量继承机制可复用。
2. **行投影 + 重启判别**:kernel 聚合为真相源后,proc/task/vars 行改为
   CommitReceipt 投影;重启判别按 `kernel/aggregates/<pid>` 存在性
   (`AsyncAggregateRepository::load`)优先于 proc 行。
3. **parity 矩阵扩展**:Cancel(已有)/Abort/Error/Back × (IRQ/普通)。
   Abort 映射:`ctx.abort_task` 递归祖先 Aborted + 子孙 Skipped(context.rs:537-563)
   ——内核需等价级联(与 SkipTask 同机制,方向向上)。
   Back:Waiting→Backed + GenerationPlanner 重入(机制已通,补宿主分派 + parity)。

## 语义结论(已核实,勿再猜测)

- legacy `TaskState::Interrupt` ↔ kernel `ExecutionState::Waiting`;
  Back 仅 Waiting 合法(action_target)。
- legacy Cancel = 回退重做(task.rs:820-883):redos step 新一代 + 前向 undo/skip;
  已由重入实现。legacy Abort = 终止链路(祖先 Aborted + 子孙 Skipped)。
- join:Skipped 不算 success(死路传播);Backed 等待;JoinBlocked 错误已删除。
- 内核动作身份 = 模型节点 id;legacy 任务 id 运行时随机——parity 比完成态与
  每模型节点一个终态任务,不比 id。

## 环境注意

本机 Defender(TQDefender)在并行构建窗口锁 rlib 元文件 → examples 全量构建偶发
E0786/E0463。规避:串行构建;测试收口用 `cargo test -p acts --lib --tests`;
examples 链接验证用 `cargo build -p acts --example <name>`(单跑均成功)。
