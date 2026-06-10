# Branch

Branches allow conditional branching at a step. Set the `branches` attribute to define multiple branches, each with its own `if` condition and list of child steps.

```yml
name: test
steps:
    - id: step1
      uses: acts.transform.set
      params:
        a: 5
    - id: step2
      branches:
        - id: b1
          name: branch 1
          if: '{{ a }} > 0'
          steps:
            - id: step3
              uses: acts.transform.set
              params:
                result: positive
        - id: b2
          name: branch 2
          steps:
            - id: step4
              uses: acts.transform.set
              params:
                result: zero_or_negative
```

## Branch Attributes

| Key | Name | Description |
| ---- | ---- | ---- |
| id | ID | Unique branch identifier |
| name | Name | Branch name |
| if | Condition | When condition is satisfied, execute this branch |
| needs | Dependencies | Predecessor branch IDs, sets Pending state |
| vars | Variables | Local variables |
| steps | Steps | Child steps of this branch |
| inputs | Inputs | Input schema |
| outputs | Outputs | Output schema |

## Branch Dependencies

Use `needs` to declare dependencies between branches:

```yml
branches:
    - id: b1
      needs: [b2]
      steps:
        - id: step3
    - id: b2
      steps:
        - id: step4
```

If branch `b1` depends on `b2`, the engine sets `b1` to Pending state until `b2` is completed.

## Expressions in Conditions

Branch conditions use `{{ }}` expression syntax:

```yml
# Variable comparison
if: '{{ a }} > 0'

# Multi-condition
if: '{{ a }} > 0 && $get("status") == "active"'
```
