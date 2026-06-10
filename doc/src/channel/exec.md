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
