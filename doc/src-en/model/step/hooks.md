# Step Events

Steps themselves do not have independent event hooks. Step events are managed through the workflow-level `on` field.

## Workflow-Level Events

Workflow events are triggered globally. See [Events](../hooks.md) for details.

```yml
name: test
on:
  - id: event1
    uses: acts.event.manual
steps:
  - id: step1
    uses: acts.core.irq
```

## Event Flow

1. Client triggers an event
2. The workflow instance starts
3. All steps execute in sequence
4. Steps complete, workflow ends

Steps do not have their own independent event mechanisms; all event control is via the workflow-level `on` configuration.
