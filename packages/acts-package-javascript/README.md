# acts-package-javascript

The acts package plugin that runs JavaScript code with an embedded
[QuickJS](https://bellard.org/quickjs/) runtime, registered as the
`acts.app.javascript` workflow act.

The engine's own `${{ }}` expression evaluator is `acts-expr` (in the `acts`
crate); QuickJS lives here, behind the `acts.app.javascript` package
that executes arbitrary JavaScript for variable computation and transformation.
Task data reaches the script through `${{ }}` placeholders (evaluated by the
engine), and the script hands data back by returning a JSON object.

## Installation

```bash
cargo add acts-package-javascript
```

## Start

```rust,no_run
use acts::Engine;
use acts_package_javascript::CodePackage;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_package::<CodePackage>()
        .start()
        .await
        .unwrap();
}
```

## Example

```yaml
id: code_demo
ver: 0.1.0
vars:
  - name: value
    value: 21
steps:
  - uses: acts.app.javascript
    params: |
      return { doubled: ${{ value }} * 2, upper: "hello".toUpperCase() };
```

The JavaScript runs in its own QuickJS realm per evaluation, with the
`console` logging methods and `Array.prototype.union`/`intersection`/
`difference` extensions available.
