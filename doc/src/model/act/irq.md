# IRQ 中断请求

活动 `acts.core.irq` 由引擎发起中断请求，等待客户端响应。当活动发起时，活动处于中断状态(interrupted)，需要客户端接受并处理后调用完成接口继续执行。

```yml
name: test
steps:
    - id: step1
      uses: acts.core.irq
      params:
        # 活动标识key，发送给客户端的消息key为此值
        key: act1
      options:
        # 导出变量到父节点
        exposes:
          - name: v
```

活动属性如下：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 活动节点的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| tag | 标签 | 标签设置 |
| uses | 包名 | 固定为 `acts.core.irq` |
| params | 参数 | 传递给包的参数，主要包含 `key` 作为消息关键字 |
| inputs | 输入 | 发送给客户端的输入数据 |
| outputs | 输出 | 客户端返回后导出到父节点的变量 |
| options | 选项 | 额外选项，如 `exposes` 导出变量 |

## 客户端处理

客户端通过 `on_message` 回调接收中断请求消息，处理后调用 `do_action2` 完成：

```rust
engine.channel().on_message(move |e| {
    if e.is_params_key("act1") && e.is_state(MessageState::Created) {
        // 处理业务逻辑
        // 完成后调用 Next 继续执行
        rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new()).unwrap();
    }
});
```
