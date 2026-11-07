# scheduler/shell 扩展性设计

- 状态：定稿（能力模型已按 `design/caps.md` 落地；本文为 shell 视角的现状与规则）
- 日期：2026-10-29
- 范围：`crates/acts/src/caps`、`crates/acts/src/scheduler/shell`、`crates/acts/src/scheduler/runtime.rs`
- 历史：本文曾是 PortSet/能力注册方案的设计稿；该方案已被"能力走 package"
  取代，旧的迁移阶段（P1–P5）随之作废，见 `doc/plan/caps-refactor.md`。

## 1. 职责切分

一次操作的生命周期：

1. 读取聚合/恢复索引，claim 操作（`Workers` + `recovery`）。
2. 预检、固定 durable intent（`operation/*`、`planning`）。
3. shell 阶段执行外部工作：包效果（`effects`）、分支谓词（`predicate`）、
   assignment 解析（`caps/assign` 经 `shell/assign` 编排）。
4. 计划提交（`core/plan` + 聚合仓库），投影读模型（`projection`）。
5. 投递 outbox（`delivery`）、发生命周期事件（`lifecycle`）、跑定时器
   （`timers`）。

1、2、4 是**正确性内核**（状态机、CAS、恢复、admission、授权、投影），
不开放替换；3、5 是**能力**，见 `design/caps.md`。

## 2. 现状

`Workers`（原 KernelHost，`shell/mod.rs`）持有：

- `caps: Arc<RwLock<CapServices>>` —— 四个端口槽位
  （effects/predicates/delivery/lifecycle），装配期 `attach_*` 安装，
  last wins，serve 后只读约定（无运行时替换）。
- `assignment`（`AssignmentResolver` 槽位）—— start 时在 headless scope 上
  解析 `assign.owner`（本地原子；业务原子由 `assign.candidates` 的 uses 包
  处理）。
- 读模型（`projection`）、定时器（`timer_guider`）—— 内核设施，非端口。

`Runtime::create` 是装配点：`PackageEffectRunner`、`MessageForwarder`、
`FnLifecycleSink`（转发 emitter 事件）、`EnginePredicateRunner` 依次 attach。

## 3. 扩展规则

1. **新业务数据 / 外部副作用 → package**（`ActPackage`，模型用 `uses` 声明）。
   参与人计算（`acts.core.users`）与 join 裁决（`acts.core.decide`）是
   两个范例：内核定信封与契约，业务在客户端。
2. **新持久状态 / 新操作 → core**（设计评审）。
3. **新传输 → 插件**（`ActPlugin` 注册包/挂 transport；无能力注册 API）。
4. **确定性内核封闭**：操作状态机、plan、提交、授权、投影不接受实现替换。
5. **环境扩展**：`ActUserVar` / `ActFunction`（`Engine::register_env_var` /
   `register_env_function`）扩展表达式可读/可调用的面，不绕过授权。

## 4. 能力 trait 与 key

每个端口 trait 以 `caps::Capability`（`fn key(&self) -> &str`）为 supertrait；
`Workers::caps()` 汇总已安装实现的 key，`sys:status` 的 `caps` 字段输出。
内置 key：`kernel.effect`、`kernel.predicate`、`kernel.delivery`、
`kernel.lifecycle`、`kernel.join`、`kernel.assign`（shadow 默认投递为
`shadow.delivery`，测试用 transport 为 `test.delivery`/`test.lifecycle`）。

## 5. 后续

- `$assign` 环境变量已实现（最近祖先 + `$assign.at("<step-id>")` 显式执行）：
  见 `doc/design/caps.md` §8。
- `AssignmentResolver` 已并入 `CapServices.assignment`（`kernel.assign`）。
