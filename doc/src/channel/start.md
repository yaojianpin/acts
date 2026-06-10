# 启动

通过客户端 Channel 启动工作流。

## 启动工作流

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

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

## 事件触发启动

如果工作流定义了 `on` 事件，也可以通过事件触发：

```rust
use acts::executor::Executor;

let mut vars = Vars::new();
vars.set("data", "event_data");
executor.evt().start("model-id:event-id", &vars)?;
```
