# 执行

通过客户端 Channel 对活动执行操作。

## 完成活动

```rust

let mut options = Vars::new();
options.set("result", "done");
client.complete(&pid, &tid, options).unwrap();
```

## 触发错误

```rust
let mut options = Vars::new();
options.set("ecode", "err_custom");
client.fail(&pid, &tid, options).unwrap();
```

## 回退到指定步骤

```rust
let mut options = Vars::new();
options.set("to", "step1");
client.back(&pid, &tid, options).unwrap();
```

## 取消活动

```rust
let mut options = Vars::new();
options.set("to", "step1");
client.cancel(&pid, &tid, options).unwrap();
```

## 跳过活动

```rust
client.skip(&pid, &tid, Vars::new()).unwrap();
```

## 中止活动

```rust
let mut options = Vars::new();
options.set("uid", "u1");
client.abort(&pid, &tid, EventAction::Abort, options).unwrap();
```

## 移除活动

```rust
client.remove(&pid, &tid, EventAction::Remove, Vars::new()).unwrap();
```

## 转办

把审批活动转给另一个业务用户；活动本身不变，只有承办关系改变。

```rust
// expect_rev 与当前承办版本不一致时返回 EASSIGNMENT_CHANGED
client
    .transfer(&pid, &tid, "u2", Some(0), Some("出差"))
    .await
    .unwrap();
```

## 代办

承办人不变，代理人可在有效期内代为决策。

```rust
// expires_at：Unix 毫秒时间戳；None 表示不过期
client
    .delegate(&pid, &tid, "u2", Some(deadline_ms), Some("休假"))
    .await
    .unwrap();

// 撤销代办：owner 重新成为 assignee，代理人立即失效
client.undelegate(&pid, &tid).await.unwrap();
```

转办、代办和撤销都需要调用者是当前承办人，或拥有 `@assign` 能力。目标用户是业务用户，其 `enabled` 与业务权限由业务系统校验。

订阅时可以用 `options.owner` / `options.assignee` 过滤待办：

```rust
let options = Vars::new().with("owner", "u1");
```

完整语义见[承办人、转办与代办](../model/assign.md)。
