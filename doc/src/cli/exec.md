# 执行

执行活动针对的是类型为`req`的活动， 当服务端生成req活动后，该活动处于中断状态，等待客户端执行。

## env
每一个命令执行， 都需要一些options参数，该命令生成后续执行动作所需的options参数。
```
env <op> [key] [value] [value-type]
    op: command with set, get, ls
            set: set key and value.
            get: get by key name
            ls: list all env values
            json: show in json format
    key: env key with string type
    value: env value
    value-type: value type with string, int, float and json, the default type is string
```

## 增加

增加一个请求(`req`)活动

```
push <pid> <tid>
    push an action to a step

    pid: proc id
    tid: step task id

    extra options:
        id: act id, it is reqiured
        name: act name
        inputs: input parameters
        outputs: expose vars to its parents
        rets: limits the request options when acting
```

## 删除

删除一个活动

```
remove <pid> <tid>
    remove an action

    pid: proc id
    tid: task id
```

## 提交
提交一个活动

```
submit <pid> <tid>
    submit an action

    pid: proc id
    tid: task id
```

## 完成
完成一个活动

```
complete <pid> <tid>
    complete the action

    pid: proc id
    tid: task id
```

## 退回
退回一个活动

```
back <pid> <tid>
    back to the history task

    pid: proc id
    tid: task id

    options:
        to: set a step id to point out which step to back
```

## 撤销
撤销一个已完成，但是下一步骤还没有完成的活动

```
cancel <pid> <tid>
    cancel the act that is completed before

    pid: proc id
    tid: task id
```

## 跳过
跳过一个活动，断续执行下一步

```
skip <pid> <tid>
    skip the action

    pid: proc id
    tid: task id
```

## 终止
终止一个活动，然后整个流程终止

```
abort <pid> <tid>
    abort the workflow

    pid: proc id
    tid: task id
```

## 错误
将一个活动设为错误，如果该活动没有设置异常处理， 则错误逐级传递，直到整个流程结束。

```
error <pid> <tid>
    set an action as error

    pid: proc id
    tid: task id

    options:
        err_code:  error code, it is required
        err_message: error message
```