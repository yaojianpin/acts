# 事件

工作流可以通过 `on` 字段定义触发事件，当事件被触发时启动工作流。

支持的事件类型：

| 事件 | 包名 | 说明 |
| ---- | ---- | ---- |
| 手动事件 | `acts.event.manual` | 同步执行，立即返回结果 |
| 钩子事件 | `acts.event.hook` | 启动工作流，等待工作流完成后返回 |
| 聊天事件 | `acts.event.chat` | 以字符串输入启动工作流 |

```yml
id: m1
name: test
on:
    - id: event_manual
      uses: acts.event.manual

    - id: event_hook
      uses: acts.event.hook

    - id: event_chat
      uses: acts.event.chat
```

事件通过 `executor.evt().start("model-id:event-id", &vars)` 触发。
