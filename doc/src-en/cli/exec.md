# Execute

Execution is for activities of type `req`. When the server generates a req activity, it is in an interrupted state, waiting for the client to execute.

## Env

Each command execution requires some options parameters. This command generates the options parameters needed for subsequent execution actions.

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

## Push

Push a request (`req`) activity:

```
push <pid> <tid>
    push an action to a step

    pid: proc id
    tid: step task id

    extra options:
        id: act id, it is required
        name: act name
        inputs: input parameters
        outputs: expose vars to its parents
        rets: limits the request options when acting
```

## Remove

Remove an activity:

```
remove <pid> <tid>
    remove an action

    pid: proc id
    tid: task id
```

## Submit

Submit an activity:

```
submit <pid> <tid>
    submit an action

    pid: proc id
    tid: task id
```

## Complete

Complete an activity:

```
complete <pid> <tid>
    complete the action

    pid: proc id
    tid: task id
```

## Back

Back an activity:

```
back <pid> <tid>
    back to the history task

    pid: proc id
    tid: task id

    options:
        to: set a step id to point out which step to back
```

## Cancel

Cancel an activity that is completed but whose next step has not yet been completed:

```
cancel <pid> <tid>
    cancel the act that is completed before

    pid: proc id
    tid: task id
```

## Skip

Skip an activity and continue to the next step:

```
skip <pid> <tid>
    skip the action

    pid: proc id
    tid: task id
```

## Abort

Abort an activity, terminating the entire workflow:

```
abort <pid> <tid>
    abort the workflow

    pid: proc id
    tid: task id
```

## Error

Set an activity as error. If the activity has no error handling configured, the error propagates upward until the entire workflow ends.

```
error <pid> <tid>
    set an action as error

    pid: proc id
    tid: task id

    options:
        err_code:  error code, it is required
        err_message: error message
```
