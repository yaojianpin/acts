# 客户端 Channel

客户端 Channel 用于在应用服务连接工作流服务，用以订阅消息、执行工作流任务等工作。

## 安装

通过 `cargo` 命令安装客户端库：

```bash
cargo add acts-channel
```

## 支持的语言

| 语言 | 库 |
| ---- | ---- |
| Rust | [acts-channel](https://github.com/yaojianpin/acts-channel) |
| Python | [acts-channel-py](https://github.com/yaojianpin/acts-channel-py) |
| Go | [acts-channel-go](https://github.com/yaojianpin/acts-channel-go) |

## 基本用法

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());

// 连接服务
client.connect().await?;

// 订阅消息
client.subscribe("client1", "act*", None, None).await?;

// 部署模型
client.deploy(&model_str).await?;

// 启动工作流
let mut vars = Vars::new();
vars.set("input", 100);
client.start("model_id", vars).await?;
```
