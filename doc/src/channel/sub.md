# 订阅

通过客户端 Channel 订阅工作流消息。

## 订阅消息

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

// 订阅指定 key 的消息
// key 支持 glob 模式，如 "act*" 匹配所有以 act 开头的 key
client.subscribe("my_client", "act*", None, None).await?;
```

## 消息回调

```rust
use acts_channel::{Client, ChannelOptions, Message};

fn on_message(msg: &Message) {
    match msg.r#type.as_str() {
        "req" => {
            // 处理中断请求
            println!("收到请求活动: {:?}", msg);
        }
        "msg" => {
            // 处理消息通知
            println!("收到消息通知: {:?}", msg);
        }
        _ => {}
    }
}

let options = ChannelOptions {
    on_message: Some(on_message),
    ..ChannelOptions::default()
};
```

## 消息类型

| 类型 | 说明 |
| ---- | ---- |
| `workflow` | 流程级别消息 |
| `step` | 步骤级别消息 |
| `req` | 中断请求活动消息 |
| `msg` | 单向消息通知 |
