# 步骤

步骤(Step)是流程的基本执行单元。每个步骤可以使用一个内置或自定义包(`uses`)，并传递参数(`params`)。步骤按顺序执行，也可以包含分支、异常处理和超时处理。

```yml
name: test
steps:
    - id: step1
      name: step 1
      uses: acts.core.irq
      params:
        key: act1

    - id: step2
      name: step 2
      uses: acts.transform.set
      params:
        a: 10
```

步骤包含的属性有：

| key  | 名 称  | 说 明 |
| ---- | ------- | ---- |
| id | 标识 | 步骤节点的唯一标识 |
| name | 名称 | 有意义的名称，可以是中文等任意字符 |
| desc | 描述 | 步骤描述信息 |
| tag | 标签 | 标签设置 |
| rn | 资源名 | 用于权限控制的资源名称 |
| uses | 包名 | 使用的包名称，如 `acts.core.irq`、`acts.transform.set` 等 |
| params | 参数 | 传递给包的参数 |
| vars | 变量 | 本地变量定义 |
| if | 条件 | 根据条件判断是否跳过当前步骤执行，如 `${{ a }} > 0` |
| catches | 异常 | 步骤错误后进行异常处理，类型为 `Step` 列表 |
| timeouts | 超时 | 超时处理，类型为 `Step` 列表 |
| branches | 分支 | 步骤分支，一个步骤可以有多个分支 |
| next | 下一步 | 当步骤完成后，可直接跳转到指定步骤执行 |
| options | 选项 | 额外选项，如 `exposes` 导出变量 |
| metadata | 元数据 | 用于UI样式的额外信息，不发送给客户端 |

## 多活动步骤

当步骤需要执行多个活动时，可以使用 `acts.core.block` 包，在 `params` 中嵌套定义子活动列表：

```yml
steps:
    - id: step1
      uses: acts.core.block
      params:
        mode: sequence
        acts:
          - uses: acts.core.irq
            params:
              key: act1
          - uses: acts.core.msg
            params:
              key: msg1
```

`acts.core.parallel` 可以对集合进行并行执行，`acts.core.sequence` 可以对集合进行顺序执行。
