# 配置

步骤的 `vars` 用于定义步骤级别的本地变量：

```yml
name: test
steps:
    - id: step1
      vars:
        - name: local_a
          value: 5
      uses: acts.core.irq
      params:
        key: act1
```

步骤也可以通过 `options.exposes` 导出变量到父节点：

```yml
name: test
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      options:
        exposes:
          - name: a
```
