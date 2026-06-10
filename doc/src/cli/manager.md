# 管理

## 模型列表
列出已部署的模型
```
models [count]
    query the current deployed models
    
    count: expect to load the max model count
```

## 查看模型
查看模型数据
```
model <mid> [fmt]
    query the model data
    mid: model id
    fmt: display format with text|json|tree
```

## 流程列表
列出运行中的所有流程
```
procs [count]
    query the current running procs
    count: expect to load the max proc count
```

## 查看流程
查看流程数据
```
proc <pid> [fmt]
    query the proc data
    fmt: display format with json|tree, the default is tree
```

## 任务列表
列出流程所有任务列表
```
tasks <pid>
    query the proc tasks
    pid: the proc id
```

## 查看任务
查看任务数据
```
task <pid> <tid>
    query the task data
```
