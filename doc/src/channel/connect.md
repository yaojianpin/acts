# 连接

应用服务通过 `Client` 创建并连接到服务。

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());

// 连接到 acts-server
client.connect().await?;
```

## 连接选项

```rust
use acts_channel::ChannelOptions;

let options = ChannelOptions {
    // 客户端标识
    client_id: Some("my_client".to_string()),
    // 其他配置
    ..ChannelOptions::default()
};

let mut client = Client::new("http://localhost:8080", &options);
client.connect().await?;
```
