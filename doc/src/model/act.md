# 活动

活动(Act)是最小的执行单元，代表一个具体的操作。活动通过 `uses` 指定使用的包，通过 `params` 传递参数。

活动属性如下：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 活动的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| tag | 标签 | 标签设置 |
| rn | 资源名 | 用于权限控制的资源名称 |
| uses | 包名 | 使用的包名称 |
| params | 参数 | 传递给包的参数 |
| inputs | 输入 | 输入数据 |
| outputs | 输出 | 导出数据 |
| options | 选项 | 额外选项，如 `exposes` 导出变量 |
| metadata | 元数据 | UI元数据，不发送给客户端 |

## 活动类型

活动根据 `uses` 指定的包来决定其行为：

| 包名 | 类型 | 说明 |
| ---- | ---- | ---- |
| `acts.core.irq` | 中断请求 | 由引擎发起中断，等待客户端响应后继续 |
| `acts.core.msg` | 消息 | 发送消息到客户端，不需要响应 |
| `acts.transform.set` | 设置 | 设置变量值 |
| `acts.transform.code` | 代码 | 执行 JavaScript 代码 (QuickJS) |
| `acts.core.block` | 块 | 包含子活动列表，按模式执行 |
| `acts.core.parallel` | 并行 | 对集合进行并行执行 |
| `acts.core.sequence` | 顺序 | 对集合进行顺序执行 |
| `acts.core.subflow` | 子流程 | 调用另一个工作流 |
| `acts.core.action` | 命令 | 执行引擎命令 (error, complete 等) |

## 活动示例

```yml
# 中断请求 - 等待客户端完成
- uses: acts.core.irq
  params:
    key: act1

# 消息 - 单向通知客户端
- uses: acts.core.msg
  params:
    key: msg_notify

# 设置变量
- uses: acts.transform.set
  params:
    a: 10
    b: hello

# 执行脚本
- uses: acts.transform.code
  params: |
    let x = $get("a");
    $set("result", x * 2);
```
