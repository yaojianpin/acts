# 订阅

客户端可通过`sub`命令订阅消息

```
sub <client_id> [type] [state] [tag] [key]
    subscribe server message
    type, state and tag are all support glob string

    client_id:  client id
    type: message types are in workflow, job, step, branch and act.
    state: message state in created, completed, error, cancelled, aborted, skipped and backed.
    tag: message tag which is defined in workflow model tag attribute.
    key: message key

    for examples:
    1. sub all messages:
    sub  1
    2. sub all act messages:
    sub 1 act
    3. sub created and complete messages
    sub 1 * {created,completed}
    4. sub all messages that the tag starts with abc
    sub 1 * * abc*
    5. sub all messages that the key starts with 123
    sub 1 * * * 123*
```