# Set 设置变量

活动 `acts.transform.set` 用来设置当前变量值。

```yml
name: test
steps:
    - id: step1
      uses: acts.transform.set
      params:
        a: 5
        b: hello
```

如果当前父节点或全局有相同名称的变量，则更新该变量的值：

```yml
name: test
vars:
    - name: a
      value: 0
steps:
    - id: step1
      # 此活动将全局变量 'a' 更新为 5
      uses: acts.transform.set
      params:
        a: 5
```
