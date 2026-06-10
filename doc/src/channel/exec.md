# 执行

通过客户端 Channel 对活动执行操作。

## 完成活动

```rust
use acts::event::EventAction;

let mut outputs = Vars::new();
outputs.set("result", "done");
rt.do_action2(&pid, &tid, EventAction::Next, outputs).unwrap();
```

## 触发错误

```rust
let mut options = Vars::new();
options.set("ecode", "err_custom");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();
```

## 回退到指定步骤

```rust
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Back, options).unwrap();
```

## 取消活动

```rust
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Cancel, options).unwrap();
```

## 跳过活动

```rust
rt.do_action2(&pid, &tid, EventAction::Skip, Vars::new()).unwrap();
```

## 中止活动

```rust
let mut options = Vars::new();
options.set("uid", "u1");
rt.do_action2(&pid, &tid, EventAction::Abort, options).unwrap();
```

## 移除活动

```rust
rt.do_action2(&pid, &tid, EventAction::Remove, Vars::new()).unwrap();
```

## EventAction 参考

| 操作 | EventAction | 参数 | 说明 |
| ---- | ---- | ---- | ---- |
| 下一步 | `Next` | outputs (可选) | 完成当前活动，携带输出数据继续执行 |
| 提交 | `Submit` | — | 将活动标记为已提交 |
| 回退 | `Back` | `to` — 目标步骤ID | 回退到指定步骤 |
| 取消 | `Cancel` | `to` — 目标步骤ID | 取消指定步骤的活动 |
| 跳过 | `Skip` | — | 跳过当前活动 |
| 中止 | `Abort` | `uid` — 用户标识 | 中止当前活动 |
| 错误 | `Error` | `ecode` — 错误码 | 触发错误处理 |
| 移除 | `Remove` | — | 移除活动 |
