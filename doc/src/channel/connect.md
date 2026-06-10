
# 安装 acts-channel client库

通过 `cargo` 命令安装：

```bash
cargo add acts-channel
```

# 连接

应用服务通过 `ActsChannel` 创建并连接到服务。

```rust
use acts_channel::ActsChannel;

let mut client = ActsChannel::new("http://localhost:8080");

// 连接到 acts-server
client.connect().await?;
```
