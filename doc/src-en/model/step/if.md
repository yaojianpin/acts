# Step Condition

Use the `if` attribute to control whether the current step executes.

```yml
steps:
    - id: step1
      uses: acts.transform.set
      params:
        a: 10

    - id: step2
      if: '${{ a }} > 0'
      uses: acts.core.irq
      params:
        key: act1

    - id: step3
      if: '${{ a }} <= 0'
      uses: acts.core.msg
      params:
        key: skipped
```

## Expression Syntax

Step conditions use `${{ }}` for variable interpolation:

```yml
# Numeric comparison
if: '${{ count }} >= 10'

# String comparison
if: '${{ status }} == "active"'

# Boolean check
if: '${{ flag }} == true'

# Logical AND
if: '${{ a }} > 0 && $get("b") == "yes"'

# Logical OR
if: '${{ a }} > 0 || $get("b") == "yes"'
```

When the `if` condition is not met, the step is skipped and execution continues with the next step.
