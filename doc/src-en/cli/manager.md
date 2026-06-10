# Manager

## Model List

List deployed models:

```
models [count]
    query the current deployed models

    count: expect to load the max model count
```

## View Model

View model data:

```
model <mid> [fmt]
    query the model data
    mid: model id
    fmt: display format with text|json|tree
```

## Process List

List all running processes:

```
procs [count]
    query the current running procs
    count: expect to load the max proc count
```

## View Process

View process data:

```
proc <pid> [fmt]
    query the proc data
    fmt: display format with json|tree, the default is tree
```

## Task List

List all task list of a process:

```
tasks <pid>
    query the proc tasks
    pid: the proc id
```

## View Task

View task data:

```
task <pid> <tid>
    query the task data
```
