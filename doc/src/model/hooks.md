# 触发器(Triggers)

工作流通过 `on` 字段声明触发器（Trigger），当触发器被触发时启动工作流。触发器只描述工作流的启动方式，本身不会在流程内执行。

支持的触发器类型（`kind`）：

| kind | 说明 | 触发方式 |
| ---- | ---- | ---- |
| `manual` | 手动触发 | `executor.evt().start("model-id:trigger-id", &vars)`，立即返回进程 id |
| `chat` | 聊天触发 | 同上，以字符串消息作为输入（写入 `params` 变量） |
| `hook` | 钩子触发 | 同上，阻塞等待工作流完成并返回其输出 |
| `schedule` | 定时触发 | 引擎定时器按 cron 表达式自动触发，不可手动启动 |

```yml
id: m1
name: test
on:
  - id: event_manual
    kind: manual
    name: start by manual
    # 触发时无调用方参数时使用的默认输入
    params:
      value: 0

  - id: event_hook
    kind: hook

  - id: event_chat
    kind: chat

  # cron 表达式 6 段: 秒 分 时 日 月 周
  - id: event_schedule
    kind: schedule
    schedule: "0 * * * * *"
    params:
      value: 0
```

- `manual`/`chat`/`hook` 通过 `executor.evt().start("model-id:trigger-id", &payload)` 触发；`payload` 为空时使用声明里的 `params` 作为启动输入。
- `manual` 触发器也可以作为 web url trigger：HTTP 传输层（如 `acts-plugin-web` 的 `POST /hooks/{model-id}:{trigger-id}`）以请求体为 payload 启动它，因此不需要单独的 `webhook` kind。
- `schedule` 触发器在部署时记录运行状态（`last_run`/`next_run`），由引擎定时器轮询触发；重新部署模型时引擎会同步触发器数据——声明变化则更新、被移除的触发器会被清除。
- `kind` 也可以填其它已注册事件包 id，走包注册表触发（自定义触发器）。
