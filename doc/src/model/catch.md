# 异常

异常处理机制允许在步骤发生错误时进行恢复处理。

## 步骤级异常

步骤通过 `catches` 定义异常处理，`catches` 是 `Step` 列表：

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      catches:
        # 匹配特定错误码
        - uses: acts.core.msg
          if: $ecode() == 'err1'
          params:
            key: catch_err1

        # 匹配所有未处理的错误
        - uses: acts.core.msg
          params:
            key: catch_others
```

## 错误处理流程

1. 步骤中的活动触发错误（通过 `EventAction::Error` 或 `acts.core.action`）
2. 引擎依次检查 `catches` 列表中的条件
3. 匹配到第一个满足条件的 catch 后执行对应的处理步骤
4. 处理后步骤继续正常执行
5. 如果没有匹配的 catch，错误向上传递

## 错误码

错误码通过 `ecode` 传递：

```rust
let mut options = Vars::new();
options.set("ecode", "err1");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();
```

在 catch 的 `if` 条件中使用 `$ecode()` 获取错误码。
