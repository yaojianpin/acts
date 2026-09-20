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

设置的值只写进**当前任务自己的数据**（同名变量在本任务内被覆盖）；它不会再直接改写父任务或全局变量的数据，而是随任务的输出**依次向上传递**：任务 `next` 落到父任务时，本任务的 outputs 折进父任务的数据，父任务再把这一层往上传。

```yml
name: test
vars:
    - name: a
      value: 0
steps:
    - id: step1
      # step1 自己的 a 变成 5；step1 走完并向上传播后，workflow 的 a 才是 5
      uses: acts.transform.set
      params:
        a: 5
```
