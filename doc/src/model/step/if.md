# 条件

步骤中的条件 `if` 用来判断执行时是否可跳过当前步骤。

```yml
name: test
steps:
    - id: step1
      if: '{{ a }} > 0'
      uses: acts.core.irq
      params:
        key: act1

    - id: step2
      uses: acts.core.msg
      params:
        key: done
```

条件表达式中使用 `{{ var }}` 语法引用变量。当 `if` 条件为 `false` 时，步骤会被跳过。
