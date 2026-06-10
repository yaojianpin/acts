# Action 命令

使用 `acts.core.action` 执行引擎命令，如触发错误、完成步骤等。

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      timeouts:
        # 超时后触发错误
        - uses: acts.core.action
          if: $cost_in('8s')
          params:
            action: error
            options:
              ecode: err_timeout
```

## 支持的命令

| 命令 | 说明 |
| ---- | ---- |
| `error` | 触发错误，可传递 `ecode` 指定错误码 |

## 客户端命令

客户端也可以通过 `do_action2` 执行以下操作来影响活动状态：

| 操作 | EventAction | 说明 |
| ---- | ---- | ---- |
| 完成 | `Next` | 完成当前活动，继续下一步 |
| 提交 | `Submit` | 提交当前活动 |
| 回退 | `Back` | 回退到指定步骤 |
| 取消 | `Cancel` | 取消指定活动 |
| 跳过 | `Skip` | 跳过当前活动 |
| 中止 | `Abort` | 中止当前活动 |
| 错误 | `Error` | 标记活动为错误 |
| 移除 | `Remove` | 移除活动 |

```rust
// 完成活动
rt.do_action2(&pid, &tid, EventAction::Next, Vars::new()).unwrap();

// 触发错误
let mut options = Vars::new();
options.set("ecode", "err1");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();

// 回退到指定步骤
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Back, options).unwrap();
```
