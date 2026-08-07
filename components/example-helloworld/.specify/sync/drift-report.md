# Drift Report: example-helloworld

Generated: 2026-08-07T15:29:55Z
Spec: components/example-helloworld/specs/001-example-helloworld/spec.md
Implementation: components/example-helloworld/src/lib.rs, Cargo.toml

## Summary

| Metric | Count |
|--------|-------|
| Aligned | 10 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced | 0 |

The example-helloworld component is fully aligned with its backfilled spec. All FR-1..FR-7 and NFR-1..NFR-3 match `src/lib.rs`.

## Detailed Findings

### Aligned

- FR-1 `IGreeter` with `greeting_prefix(&self) -> &str` — `src/lib.rs:25-29`.
- FR-2 `HelloWorldComponent` provides `IGreeter`, returns `"Hello"` — `src/lib.rs:32-46`.
- FR-3 `HelloWorldComponent` declares `ILogger` receptacle — `src/lib.rs:36-38`.
- FR-4 `GreetRequest` carries `name: String` — `src/lib.rs:49-52`.
- FR-5 `GreeterHandler` implements `ActorHandler<GreetRequest>` with lifecycle hooks (`on_start`/`on_stop`) — `src/lib.rs:84-108`.
- FR-6 `handle()` increments counter and prints numbered greeting — `src/lib.rs:92-98`.
- FR-7 optional ILogger logs on start/each greeting/stop — `src/lib.rs:86-88,94-96,101-106`.
- NFR-1 module-level runnable Quick start doc — `src/lib.rs:6-17`.
- NFR-2/NFR-3 (clippy-clean, no unsafe) — `Default` delegates to `new()` (`src/lib.rs:78-82`); no `unsafe` in source.
- Dependencies match Cargo.toml (`component-framework`, `component-core`, `interfaces`, `logger`).

## Unspecced Code

None.

## Conflicts / Spec references to nonexistent artifacts

- Implementation Notes reference `apps/helloworld-mainline/` behavior (runs actor without a logger wired). This is descriptive context, not a claim of an in-crate artifact; the note was already corrected on 2026-07-22 to reflect that the mainline app does not demonstrate ILogger wiring. No drift.

## Recommendations

- None. Component is clean.
