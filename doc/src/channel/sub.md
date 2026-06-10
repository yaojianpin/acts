# 订阅

通过客户端 Channel 订阅工作流消息。

## 订阅消息

```rust
use acts_channel::{ActsChannel, ActsOptions};

let mut client = ActsChannel::connect("http://localhost:8080");
client.connect().await?;

// 订阅指定 key 的消息
client.subscribe("my_client", "act*", None, None).await?;

// ActsOptions里的属性支持 glob 模式，如 "act*" 匹配所有以 act 开头的
let options = ActsOptions {
    tag: "your tag",
    state: "{created,completed}"
    r#type: "act*"
    // 其他配置
    ..ChannelOptions::default()
};
client
    .subscribe(
        "client-1",
        move |message| {
            println!("{message:?}");
        },
        &options,
    )
    .await;
```


## 消息类型

| 类型 | 说明 |
| ---- | ---- |
| `workflow` | 流程级别消息 |
| `step` | 步骤级别消息 |
| `act` | 活动消息 |
