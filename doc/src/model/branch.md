# 分支

一个步骤可以有多个分支。第一个分支需要设置分支条件，当分支执行时，根据条件确定执行哪一个分支。当有多个条件满足时，满足条件的分支并行执行。

```yml
name: test
steps:
    - id: step1
      branches:
        - id: b1
          # 分支条件表达式
          if: '{{ a }} > 0'
          steps:
            - id: step2

        - id: b2
          # 默认分支
          # 当其他条件全为 false 时执行
          steps:
            - id: step3
    - id: step4
```

分支包含的属性有：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 节点的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| tag | 标签 | 标签设置 |
| if | 条件 | 分支条件表达式，使用 `{{ var }}` 语法引用变量 |
| needs | 前置 | 前置分支id列表，只有前置分支完成后才执行当前分支，可以通过 `needs` 将分支执行线性化 |
| vars | 变量 | 本地变量定义 |
| steps | 步骤列表 | 分支内的步骤列表 |
| inputs | 输入 | 输入schema定义 |
| outputs | 输出 | 输出schema定义 |

## 分支前置依赖 (needs)

当分支设置了 `needs` 后，该分支会进入 `Pending` 状态，等待前置分支完成后再执行：

```yml
branches:
    - id: b1
      if: 'true'
      name: 分支1
      steps:
        - id: step11
          uses: acts.core.irq
          params:
            key: act1

    - id: b2
      if: 'true'
      name: 分支2
      needs:
        - b1
      steps:
        - id: step21
```
