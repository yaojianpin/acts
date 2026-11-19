# Outbox 持久交接与消息恢复

- 状态：实现中（2026-11-19）
- 范围：`crates/acts` 的 outbox、message、delivery、cleanup
- 关联：`design/message.md`、`plan/2026-11-19-outbox-handover.md`

## 1. 问题

`OutboxState::Delivered` 曾只代表“transport 的同步函数返回了”。生产
`MessageForwarder` 把消息交给内存 Emitter 后立即确认，Canonical Message 和
Delivery Row 由异步 Handler 稍后写入。T0 确认、T1 崩溃时，重启后 Outbox 不再
重试，消息与投递责任却不存在。

同时分页 Cursor 取“最后一条成功 Record”，而页配额取 Index Entry。整页损坏时
`records=[]` 且 `next=None`，后续健康消息永久不可达。

## 2. 目标不变量

```text
I1 Outbox Delivered => Receipt 可定位并校验 Canonical Message
I2 同一 Outbox Record 重复 Dispatch => 同一 Message/Delivery ID
I3 Canonical Message 先于 Outbox 确认持久化
I4 在线通知失败只影响延迟，不丢投递责任
I5 Cursor 单调表示“最后检查的 Index Entry”，坏页不能阻塞后页
I6 Store 读错误不删除 Delivery；只有确定 None 才是孤儿
I7 Cleanup 只在 Receipt 可验证、Delivery 已结清后执行
```

## 3. 交接协议

```text
Claim Outbox Record
→ message_from_record（缺失 msg_id 时由 pid+key+topic 派生稳定 ID）
→ DurableMessagePublisher.publish
   → 从持久 Task Row 取 start/end
   → 按 message_id 幂等写 Canonical Message（内容 Hash 校验）
   → 对每个已注册且匹配的 ack Channel 写确定性 Delivery Row
   → 返回 DurableMessageReceipt { message_id, payload_hash,
                                  delivery_count, delivery_hash }
→ Outbox ConfirmDelivered(typed receipt)
→ Emitter 在线通知（仅延迟优化）
```

`MessageForwarder` 的 `deliver` 改为 async；生产 Runtime 只能构造
`MessageForwarder::durable(emit, publisher)`。测试专用的 `new(emit)` 只在
`cfg(test)` 存在并要求显式调用。

## 4. 身份

```text
MessageId   = "msg_" + xxh3_128(pid, outbox_key, topic)  // payload 无 msg_id 时
DeliveryId  = "del_" + xxh3_128(message_id, chan_id)
ReceiptHash = content_hash(canonical message fields)
```

重复 Dispatch 读到已存在对象时比较内容；同 ID 不同内容返回 Corruption，而不是
覆盖或再建一份。Delivery 创建由 `Store::store_message_delivery` 幂等化：已存在
且身份一致即成功。

## 5. Receipt

```rust
enum OutboxReceipt {
    DurableMessage(DurableMessageReceipt),
    Legacy(String), // 只读旧数据
}
```

Codec 升级到 schema v2；v1 的 Delivered 字符串解码为 `Legacy`。Cleanup 验证：

- `DurableMessage`：Message 必须存在且 `content_hash` 相等，否则拒绝清理；
- `Legacy("shadow")`：已知“未发送却确认”，永远拒绝清理；
- 其他 `Legacy`：历史数据没有可推导的 message id，按历史事实信任。

## 6. 分页与恢复

- `OutboxPage.next` 是 `entries[0..limit].last().id`，即使该 Entry 没有产出
  Record。
- Caller 在 `records.is_empty()` 且有 `next` 时推进 Cursor 并继续；预算按
  `examined` 记账，不以成功 Record 数计。
- Aggregate 缺失/损坏、Record 缺失、Index 解码失败分别记录
  `OutboxPageFailure`，按 pid 隔离并交给 repair/poison 计数。
- Retry Timer：`find_opt`。
  - `Ok(Some)` 重发；
  - `Ok(None)` 真孤儿，删除；
  - `Err` 保留 Delivery 并让本轮 Timer 记为失败，下轮重读。

## 7. 清理

`RowProjector::removable` 与 `cleanup_process` 都调用
`delivered_receipts_verified`。Aggregate 已不存在时跳过校验（无 Receipt 可验）；
存在时必须通过。Delivery 未结清仍由 `has_unsettled_deliveries` 阻止。

## 8. 明确不做

- 不引入外部 Broker Observation 协议；本仓库生产 Transport 是进程内消息层。
- 不为旧 `Legacy` Receipt 伪造类型化语义；只提供只读审计。
- 不把已 ACK 的 Delivery 物理删除改为长期 Ledger；当前 `Completed` 行随进程
  清理，足够支撑现有 Contract。
