# 流程模型

工作流引擎的执行依赖于流程模型，acts 流程模型是一个规范化的 YAML 格式文件。

## 模型结构

一个完整的流程模型由以下部分组成：

```yml
id: my_model
name: 模型名称

# 默认变量
vars:
  - name: value
    value: 0

# 输入 schema (JSON Schema)
inputs:
  type: object
  properties:
    value:
      type: number

# 输出 schema (JSON Schema)
outputs:
  type: object
  properties:
    data:
      type: object

# 启动事件
on:
  - id: event1
    uses: acts.event.manual

# 执行选项
options:
  exposes:
    - name: output_key

# 步骤列表
steps:
  - id: step1
    uses: acts.core.irq
```

## 核心概念

| 概念 | 说明 |
| ---- | ---- |
| [步骤](./model/step.md) | 流程的基本执行单元，使用 `uses` 指定包 |
| [分支](./model/branch.md) | 条件分支，通过 `if` 条件决定执行路径 |
| [活动](./model/act.md) | 实际动作执行体，使用 `uses` 指定功能包 |
| [配置](./model/setup.md) | 工作流的全局配置，包括变量、事件、输入输出 |
| [包](./model/pack.md) | 可复用的功能模块，分为 core、transform、event 三类 |
