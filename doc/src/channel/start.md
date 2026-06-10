# 启动

通过客户端 Channel 启动工作流。

## 启动工作流

```rust
use acts_channel::{ActsChannel, ChannelOptions};

let mut client = ActsChannel::connect("http://localhost:8080");

// 启动工作流
let mut vars = Vars::new();
vars.set("a", 100);
client.start("model_id", vars).await?;
```

## 启动参数

启动时可以传递变量来覆盖工作流的默认 `vars`：

```rust
let mut vars = Vars::new();
vars.set("input_value", 42);
vars.set("user_name", "admin");

client.start("my_workflow", vars).await?;
```
