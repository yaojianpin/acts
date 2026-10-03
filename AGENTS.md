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

Run the same per-crate suites CI runs (`.github/workflows/test.yml`) — every
`-p` invocation below must pass before delivery; do not substitute a single
`--workspace` run:

```sh
cargo test -p acts
cargo test -p acts-acl
cargo test -p acts-expr --all-features
cargo test -p acts-channel
cargo test -p acts-plugin-*
cargo test -p acts-package-*
cargo test -p acts-store --features sled,sqlite,postgres
cargo test -p acts-cli
cargo test -p acts-server
```

- `acts-store`'s postgres suite (`tests/postgres.rs`) needs a Postgres 16 on
  `localhost:5433` (password `yao`, db `tests`); the broker-backed suites of
  `acts-channel`/`acts-plugin-nats`/`acts-server` need NATS with JetStream on
  `127.0.0.1:4222`. Without these services the run is not green — CI starts
  both, so mirror that locally or say plainly which suites you could not run.
- Prefer extending an existing test near the changed code over creating new
  test files.

## Commits

Follow the existing style: conventional prefix (`feat`, `fix`, `test`,
`chore`, `build`, `security`), optional scope (`feat(acts-store): ...`),
imperative subject, lower-case start.
