# 订阅

通过客户端 Channel 订阅工作流消息。

## 订阅消息

```rust
use acts_channel::{ActsChannel, ActsOptions};

let mut client = ActsChannel::connect("http://127.0.0.1:10080").await?;

// ActsOptions 的属性支持 glob 模式，如 "act*" 匹配所有以 act 开头的消息
let options = ActsOptions {
    state: Some("{created,completed}".to_string()),
    r#type: Some("act*".to_string()),
    // 其他配置
    ..ActsOptions::default()
};

let sub = client
    .subscribe(
        "client-1",
        move |message| {
            println!("{message:?}");
        },
        // 不结束订阅的故障：负载解码失败、自动 ack 失败
        move |err| eprintln!("subscription fault: {err}"),
        &options,
    )
    .await?;

// 订阅结束：服务端正常关闭为 Ok(())，流或连接失败为 Err(status)
if let Err(err) = sub.wait().await {
    eprintln!("subscription closed: {err}");
}
```


## 消息类型

| 类型 | 说明 |
| ---- | ---- |
| `workflow` | 流程级别消息 |
| `step` | 步骤级别消息 |
| `act` | 活动消息 |
