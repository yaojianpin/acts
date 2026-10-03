# AGENTS.md

Guidance for coding agents working in this repository.

## Project

`acts` — a fast, lightweight, extensible workflow engine (Rust, edition 2024).

- `crates/` — core engine (`acts`, `acts-expr`, `acts-channel`, `acts-proto`, `acts-acl`, `acts-store`, `acts-server`)
- `packages/` — workflow packages (http, shell, state, nats, javascript)
- `plugins/` — integrations (grpc, web, nats)
- `examples/` — example packages/plugins

Dependency versions are declared **only** in `[workspace.dependencies]` of the
root `Cargo.toml`; members must use `workspace = true` and may only add
features. `deny.toml` + `cargo deny check` enforce this (single-version policy,
MSRV 1.88, license/advisory gates).

## Commit requirements (mandatory)

Every commit MUST pass both gates **before** you commit. Run them from the
repository root:

```sh
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo fmt
```

- Clippy runs exactly what CI runs (`.github/workflows/rust.yml`); any warning
  fails the build (`-D warnings`). Fix the code — never silence a lint with
  `#[allow(...)]` without a written justification at the attribute.
- `cargo fmt` formats all code; CI verifies with `cargo fmt --check`. If you
  want to verify without rewriting files, run `cargo fmt --check`.
- `--locked` means: if you changed any `Cargo.toml`, run `cargo update -p <crate>`
  (or `cargo check`) first so `Cargo.lock` is in sync, or clippy will fail on
  the lockfile check.
- Changes touching dependency manifests also require `cargo deny check bans`
  to stay warning-free; refresh `deny.toml`'s `skip` baseline when the graph
  moves (cargo-deny names every stale entry in its output).

## Tests

`cargo test --workspace` must pass before delivery. Prefer extending an
existing test near the changed code over creating new test files.

## Commits

Follow the existing style: conventional prefix (`feat`, `fix`, `test`,
`chore`, `build`, `security`), optional scope (`feat(acts-store): ...`),
imperative subject, lower-case start.
