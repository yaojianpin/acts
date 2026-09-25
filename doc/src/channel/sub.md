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

该 id 在服务端是调用者主体命名空间下的一段（`{subject}/{client_id}`）：不同主体的
两个订阅者都可以叫 `client-1` 而互不冲突，因为主体是该通道键的前缀，而同一个键上的注册
会顶替旧的 handler。主体只是给这个键划命名空间——真正决定收到哪些消息的是通道自己的
过滤器（`type`/`state`/`uses`/`options`），而不是发出消息的流程归谁启动；拿到消息后
能做什么，则由调用者持有的授权决定（能否打开这条流本身就由 `msg:sub` 决定）。
见[访问控制](../access.md)。


## 消息类型

| 类型 | 说明 |
| ---- | ---- |
| `workflow` | 流程级别消息 |
| `step` | 步骤级别消息 |
| `act` | 活动消息 |
