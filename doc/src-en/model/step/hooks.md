# Step Triggers

Steps do not declare their own triggers. Workflow startup is managed through the workflow-level `on` field.

## Workflow-Level Triggers

Workflow triggers are fired by the engine timer or by a caller. See [Triggers](../hooks.md) for details.

```yml
name: test
on:
  - id: event1
    kind: manual
steps:
  - id: step1
    uses: acts.core.irq
```

## Trigger Flow

1. A trigger fires (manual/chat/hook by a caller, schedule by the engine timer)
2. The workflow instance starts
3. All steps execute in sequence
4. Steps complete, workflow ends

Steps have no trigger declarations of their own; all startup control is via the workflow-level `on` configuration.
