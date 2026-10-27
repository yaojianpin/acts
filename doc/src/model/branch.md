# 分支

一个步骤可以有多个分支。第一个分支需要设置分支条件，当分支执行时，根据条件确定执行哪一个分支。当有多个条件满足时，满足条件的分支并行执行。

```yml
name: test
steps:
    - id: step1
      branches:
        - id: b1
          # 分支条件表达式
          if: 'a > 0'
          steps:
            - id: step2

        - id: b2
          # 默认分支
          # 当其他条件全为 false 时执行
          steps:
            - id: step3
    - id: step4
```

条件是一个表达式（不带 `${{ }}`）：task 的变量、某个 step 的数据（`step1.total`）、
user var（`secrets.TOKEN`）都是普通名字，直接写 `a > b` 即可；`$get(name)` 只用于
名字本身是运行时算出来的情况，例如 `$get(prefix + '_token')`（`+` 两侧都是字符串）。

条件里也可以用值自带的方法：
`name.length()`、`name.startsWith(p)`、`name.endsWith(p)`、`name.indexOf(p)`、
`name.contains(p)`、`name.is_match(re)`、`items.length()`、`items.contains(v)`、
`obj.length()`；`length()` 与 `indexOf()` 按字符计数而非字节，注入的同名方法优先。

分支包含的属性有：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 节点的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| tag | 标签 | 标签设置 |
| if | 条件 | 分支条件表达式，使用 `${{ var }}` 语法引用变量 |
| needs | 前置 | 兄弟分支 id 列表，所有前置分支到达终态后才执行当前分支，可以通过 `needs` 将分支执行线性化 |
| vars | 变量 | 本地变量定义 |
| steps | 步骤列表 | 分支内的步骤列表 |
| inputs | 输入 | 输入schema定义 |
| outputs | 输出 | 输出schema定义 |

## 分支前置依赖 (needs)

同一步骤下被选中的多个分支默认并行执行；`needs` 用来在这些并行分支之间建立依赖，
把它们的一部分串行化。设置了 `needs` 的分支先以 `pending`（内核的 `Created`）状态创建，
不立即执行；只有列表中 **所有**前置分支都到达终态（completed / skipped / failed /
cancelled 等都算）后，它才会被唤醒执行。`needs` 只决定顺序，不要求前置分支成功；
前置分支被自己的 `if` 跳过不会阻塞后续分支。
没有 `if` 的 `needs` 分支视为选中（它是顺序中的下一段），显式写出的 `if` 仍照常判断。

`needs` 只能引用同一个 `branches` 列表中的兄弟分支（可以引用声明在后面的分支），
未知名或循环依赖会在启动时被拒绝。

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
