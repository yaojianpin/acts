# acts-package-shell

The acts shell package plugin for acts. 

## Installation

```bash
cargo add acts-package-shell
```

## Start
```rust,no_run
use acts::EngineBuilder;
use acts_package_shell::ShellPackagePlugin;

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&ShellPackagePlugin)
        .build()
        .start();
}
```

## Example

```yaml
name: shell example
id: shell-example
inputs:
  my_input:  "hello, world"
steps:
  - name: shell step
    acts:
      - uses: acts.app.shell
        params:
          shell: nu
          content-type: json
          script: |
            let data = "{{ my_input }}"
            $data | split row ',' | each { |it| $it | str trim  } | to json
```