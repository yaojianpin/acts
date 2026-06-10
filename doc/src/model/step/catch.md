# 异常

当步骤发生异常错误时，可以通过 `catches` 定义异常处理。`catches` 是一个 `Step` 列表，每个 catch 步骤通过 `if` 条件匹配对应的错误。

```yml
name: test
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      catches:
        # 匹配错误码 err1
        - uses: acts.core.msg
          if: $ecode() == 'err1'
          params:
            key: catch1

        # 匹配错误码 err2
        - uses: acts.core.msg
          if: $ecode() == 'err2'
          params:
            key: catch2

        # 捕获所有其他错误（无条件）
        - uses: acts.core.msg
          params:
            key: others
```

Catch 步骤的属性与普通 Step 相同，可以使用 `uses` 和 `params`。`if` 条件中使用 `$ecode()` 获取错误码。当 catch 处理后，步骤继续正常执行；如果没有匹配的 catch，错误会向上传递。
