# 包

包(Package)是工作流中可复用的执行单元。每个步骤或活动通过 `uses` 指定包名来调用对应的包。

## 内置包

| 包名 | 类型 | 说明 |
| ---- | ---- | ---- |
| `acts.core.irq` | 中断 | 发起中断请求，等待客户端处理完成 |
| `acts.core.msg` | 消息 | 发送单向消息到客户端 |
| `acts.core.block` | 块 | 包含子活动列表，支持 `sequence` 模式 |
| `acts.core.parallel` | 并行 | 对集合进行并行执行 |
| `acts.core.sequence` | 顺序 | 对集合进行顺序执行 |
| `acts.core.subflow` | 子流程 | 调用另一个工作流模型 |
| `acts.core.action` | 命令 | 执行引擎命令 |
| `acts.transform.set` | 变换 | 设置变量值 |
| `acts.transform.code` | 变换 | 执行 JavaScript 代码 (QuickJS 运行时) |

工作流的启动触发器通过 `on` 字段声明（manual/chat/hook/schedule），参见 [触发器](./hooks.md)。

## 使用示例

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1

    - id: step2
      uses: acts.core.block
      params:
        mode: sequence
        acts:
          - uses: acts.core.irq
            params:
              key: sub_act1
          - uses: acts.core.msg
            params:
              key: sub_msg
```

## 自定义包

可以通过实现 `ActPackage` trait 来扩展自定义包，使用 `engine.extender().register_package(&meta)` 注册。
