# Triggers

Workflows declare triggers through the `on` field; a triggered workflow is started by the engine or by a caller. A trigger only declares the start surface of the workflow — it never runs inside a process.

## Trigger Types (`kind`)

| kind | Description | How it fires |
| ---- | ---- | ---- |
| `manual` | Manual trigger | `executor.evt().start("model-id:trigger-id", &payload).await` — returns the process id |
| `chat` | Chat trigger | Same entry, a string message becomes the start input (`params` variable) |
| `hook` | Hook trigger | Same entry, blocks until the workflow completes and returns its outputs |
| `schedule` | Schedule trigger | Fired by the engine timer on a cron expression; cannot be started manually |

```yml
id: m1
name: test
on:
  - id: event_manual
    kind: manual
    name: start by manual
    # default start inputs used when the caller passes no payload
    params:
      value: 0

  - id: event_hook
    kind: hook

  - id: event_chat
    kind: chat

  # cron expression of 6 fields: sec min hour day month dow
  - id: event_schedule
    kind: schedule
    schedule: "0 * * * * *"
    params:
      value: 0
```

- `manual`/`chat`/`hook` fire through `executor.evt().start("model-id:trigger-id", &payload).await`; a `null` payload falls back to the declared `params`.
- `manual` triggers double as web URL triggers — an HTTP transport (e.g. `acts-plugin-web`'s `POST /hooks/{model-id}:{trigger-id}`) starts them with the request body as payload, so no separate `webhook` kind is needed.
- `schedule` triggers keep their run state (`last_run`/`next_run`) on the deployed trigger row and are polled by the engine timer. Re-deploying a model reconciles the trigger data — changed declarations are updated and removed triggers are cleaned up.
- `kind` may also be any registered event package id (custom triggers), fired through the package registry.
