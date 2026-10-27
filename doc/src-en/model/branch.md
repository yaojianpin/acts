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
          if: 'a > 0'
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
| needs | Dependencies | Sibling branch IDs; the branch waits until all of them are terminal |
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

Selected branches of one step run in parallel by default; `needs` adds dependencies
between those parallel branches and serializes the dependent ones. A branch with `needs`
is created `pending` (the kernel's `Created`) and is admitted only after **every** named
predecessor is terminal. The dependency is ordering, not success:
a predecessor skipped by its own `if`, failed or cancelled has still settled and does not
block the dependent. A `needs` branch with no `if` is selected (it is the next stage of the
ordered fan-out); an explicit `if` is still evaluated.

Every id must name a sibling branch in the same `branches` list (a later declaration is
allowed); an unknown id or a dependency cycle refuses the start.

## Expressions in Conditions

A branch condition *is* an expression — written bare, without the `${{ }}` that
marks a placeholder inside a param or var string — and it must evaluate to a
bool:

```yml
# Variable comparison
if: 'a > 0'

# Multi-condition
if: 'a > 0 && status == "active"'
```

Task vars, a step's data (`step1.total`) and a user var (`secrets.TOKEN`) are
ordinary names in an expression — there is nothing to call to read them.
`$get(name)` is for the one case a name cannot spell: a name the expression
builds itself, e.g. `$get(prefix + '_token')` (with `+` between two strings).

A condition may also use the methods a value carries of its own:
`name.length()`, `name.startsWith(p)`, `name.endsWith(p)`, `name.indexOf(p)`,
`name.contains(p)`, `name.is_match(re)`, `items.length()`, `items.contains(v)`
and `obj.length()`. `length()` and `indexOf()` count characters, not bytes; an
injected method of the same name wins over the built-in one.
