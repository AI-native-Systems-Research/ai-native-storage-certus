# Rust posture

This file is authoritative for this repo: every agent working here reads it
before running a verification step, and it takes precedence over any default
a skill would otherwise assume. Keep it short enough to read in full.

## Detected facts

| Fact | Value | Source |
|---|---|---|
| Edition | 2021 | `[workspace.package].edition` |
| MSRV | 1.81 | `[workspace.package].rust-version`, `clippy.toml` `msrv` (no CI job builds on 1.81; Jenkins uses the agent's default toolchain) |
| Async runtime | none in the core; `tokio` only in `certus-server-yaml` (gRPC/otel) and `component-core` (`tokio::sync` only) | member `Cargo.toml`s |
| `no_std` | no — std everywhere | crate roots |
| Unsafe policy | allowed with review — FFI-heavy (SPDK, CUDA, RDMA, shared memory); every `unsafe` needs a `// SAFETY:` comment | CLAUDE.md coding conventions; no `#![forbid(unsafe_code)]` |

## Lint levels

`[workspace.lints]` in the root `Cargo.toml`; all 51 members opt in with
`[lints] workspace = true`. **Every level is `warn`, nothing is denied** — a
deliberate choice: the set is visible but non-blocking while the backlog
(~870 unwrap/expect/unsafe-doc hits, ~3.5k pedantic) is paid down. Do not
raise a level to `deny` or lower one to `allow` without the user asking.
`clippy.toml` holds only knobs: `msrv` and the `#[cfg(test)]` carve-out for
`unwrap_used`/`expect_used`. `missing_docs` is set per library crate
(`#![warn(missing_docs)]`, e.g. the workload-generator crates), not workspace-wide.

New code: prefer `#[expect(lint, reason = "...")]` over `#[allow]`; for
`unsafe`, write the `// SAFETY:` invariant and a `# Safety` doc on `pub unsafe fn`.

## Commands

```bash
cargo test --all -- --test-threads 1       # default members, as CI does
cargo test -p <spdk-crate>                 # SPDK crates are not default members
cargo clippy --all-targets -- -D warnings  # CLAUDE.md gate for code you touch
cargo fmt --check
```

`clippy -D warnings` over the whole workspace is currently red because of the
warn-level set above; judge your change by not adding new warnings in the files
you touch. Jenkins does not run clippy.

`--all-features` is **not** meaningful: `spdk` and `gpu` features need native
SPDK/CUDA builds, and `ci-fast` / `otel` change test behaviour. Build named
features per crate (see the Jenkinsfile test stages).

Tooling: `cargo audit` — not run. `cargo hack --feature-powerset check` — not
run; worth a scheduled job for `interfaces` (`spdk`/`gpu` gating). `cargo udeps`
— not run. `cargo miri test` — not run; the candidates are the pure-Rust
`unsafe` in `lib/shm-queue` and `component-core` channels (FFI crates can't run
under Miri).

## Public surface

`components/interfaces` (the `I*` traits) and `lib/component-framework`
(`component-framework` facade over `component-core`/`component-macros`).
Components are reached only through their interfaces; `apps/*` are binaries.
