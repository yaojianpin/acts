# MSG 消息

活动 `acts.core.msg` 由引擎发起单向消息，发送给客户端后不等待响应，立即继续执行后续步骤。

```yml
name: test
steps:
    - id: step1
      uses: acts.core.msg
      params:
        # 消息关键字key
        key: msg1
      inputs:
        a: 1
```

活动属性如下：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 活动节点的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| tag | 标签 | 标签设置 |
| uses | 包名 | 固定为 `acts.core.msg` |
| params | 参数 | 传递给包的参数，主要包含 `key` 作为消息关键字 |
| inputs | 输入 | 发送给客户端的输入数据 |

## 客户端接收

客户端通过 `on_message` 接收消息：

```rust
engine.channel().on_message(move |e| {
    if e.is_params_key("msg1") && e.is_state(MessageState::Completed) {
        // 消息已收到，直接处理即可，无需响应
        println!("收到消息: {:?}", e.inputs);
    }
});
```
