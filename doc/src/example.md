# 案例

以下示例展示了 acts 工作流引擎的各种使用场景。

## 基础示例

| 示例 | 说明 | 路径 |
| ---- | ---- | ---- |
| 简单循环 | 使用 JavaScript 代码实现循环累加 | [examples/simple](https://github.com/yaojianpin/acts/tree/main/examples/simple) |
| While 循环 | 使用 `while` 条件实现循环累加 | [examples/while](https://github.com/yaojianpin/acts/tree/main/examples/while) |
| 程序构建 | 使用 Rust Builder API 构建工作流 | [examples/model_build](https://github.com/yaojianpin/acts/tree/main/examples/model_build) |

## 交互示例

| 示例 | 说明 | 路径 |
| ---- | ---- | ---- |
| 动作交互 | 使用 IRQ 中断请求与客户端交互 | [examples/actions](https://github.com/yaojianpin/acts/tree/main/examples/actions) |
| 审批流程 | 多角色审批工作流（PM、GM） | [examples/approve](https://github.com/yaojianpin/acts/tree/main/examples/approve) |
| 消息通知 | 使用 MSG 消息发送单向通知 | [examples/message](https://github.com/yaojianpin/acts/tree/main/examples/message) |

## 错误与超时

| 示例 | 说明 | 路径 |
| ---- | ---- | ---- |
| 异常处理 | 使用 catches 捕获和处理错误 | [examples/catches](https://github.com/yaojianpin/acts/tree/main/examples/catches) |
| 超时处理 | 使用 timeouts 处理步骤超时 | [examples/timeout](https://github.com/yaojianpin/acts/tree/main/examples/timeout) |

## 高级特性

| 示例 | 说明 | 路径 |
| ---- | ---- | ---- |
| 事件驱动 | 使用 on 事件触发工作流启动 | [examples/event](https://github.com/yaojianpin/acts/tree/main/examples/event) |
| 子流程 | 使用 subflow 调用子工作流 | [examples/subflow](https://github.com/yaojianpin/acts/tree/main/examples/subflow) |
| 自定义包 | 创建和注册自定义包 | [examples/package](https://github.com/yaojianpin/acts/tree/main/examples/package) |
| 自定义变量 | 注册和使用自定义用户变量 | [examples/user_var](https://github.com/yaojianpin/acts/tree/main/examples/user_var) |

## 插件示例

| 示例 | 说明 | 路径 |
| ---- | ---- | ---- |
| HTTP 请求 | 使用 acts-package-http 发送 HTTP 请求 | [examples/plugins/http](https://github.com/yaojianpin/acts/tree/main/examples/plugins/http) |
| Shell 执行 | 使用 acts-package-shell 执行 Shell 脚本 | [examples/plugins/shell](https://github.com/yaojianpin/acts/tree/main/examples/plugins/shell) |
| 状态管理 | 使用 acts-package-state 管理状态 | [examples/plugins/state](https://github.com/yaojianpin/acts/tree/main/examples/plugins/state) |

## 运行示例

```bash
# 运行审批流程示例
cargo run --example approve

# 运行异常处理示例
cargo run --example catches

# 运行超时处理示例
cargo run --example timeout
```
