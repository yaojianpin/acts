# 2026-11-19 Outbox 持久交接改造

依据：`design/outbox-handover.md`。

## P1 阻止永久丢失

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | `OutboxTransport::deliver` 改 async；生产 Forwarder 先持久化再确认 | 在确认后任意指令崩溃，重启可读 Message/Receipt |
| 2 | `MessageForwarder::durable` 是唯一生产构造器；`new` 仅 `cfg(test)` | 生产构建不存在只发内存的构造路径 |
| 3 | Cursor 取最后检查 Index Entry；空记录页仍推进 | 前 N 条全坏，第 N+1 条健康记录被处理 |
| 4 | Retry 用 `find_opt`，Store Err 保留 Delivery | 临时读错误后 Delivery 仍存在并被重试 |
| 5 | Cleanup 校验 Receipt/Message；Shadow 永不清理 | Delivered 但 Message 缺失时 `removable=false` |

## P2 稳定身份与类型化 Receipt

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | 稳定 Message/Delivery ID | 重复发布只产生一行 |
| 2 | Canonical Message 内容 Hash 校验 | 同 ID 不同内容报 Store 错误 |
| 3 | `OutboxReceipt` typed + codec v2 + v1 兼容 | v1 数据可读为 Legacy；新数据为 Durable |
| 4 | `DurableMessagePublisher` 集中创建 Message/Delivery | Transport 不再依赖任意 Emitter Handler |
| 5 | ACK 按 Channel 的 subject 前缀限制；行锁保证幂等 | 普通调用者不能 ACK 其他 subject 的行 |

## P3 原子交接边界

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | Claim → publish → confirm 每个崩溃点可重放 | 发布后崩溃：Claim 过期重试收敛 |
| 2 | 发布失败映射 RetryAt，内容冲突映射 Quarantine | Store 失败不产生 Delivered |
| 3 | Durable Task Row 提供消息窗口 | 重启前后同一 Task 的 cost 一致 |

## P4 容量与运营

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | Pass 预算同时限制 examined 与 delivered | 全坏 Index 不会无限扫描 |
| 2 | Metrics 增加 examined/bad_index | 指标与 Pass 报告一致 |
| 3 | 坏 pid 继续隔离、修复成功后自动退出 poison | 其他 pid 不被阻塞 |
| 4 | 类型化诊断（IndexCorrupt/AggregateMissing/RecordMissing/AggregateCorrupt） | 日志和指标能区分 |

## 测试

- 单元：stable id、receipt codec/validation、publisher 幂等/并发/冲突、
  targets 注册与匹配、receipt 校验、retry 分类。
- 分页：全坏页 + 后置健康记录、cursor 单调、空 records 推进。
- 集成：生产 Runtime 的 Forwarder 在确认前写入 Message/Delivery；Shadow
  Receipt 阻止清理。
- 最后统一运行 `cargo test -p acts --lib`、`--doc` 与 AGENTS.md 列出的
  per-crate 套件；资源受限时按说明使用 `-j 2` 并报告未运行项。
