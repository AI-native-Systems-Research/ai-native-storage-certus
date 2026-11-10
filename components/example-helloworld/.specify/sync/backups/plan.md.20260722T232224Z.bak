# Implementation Plan: Example Hello World Component

**Branch**: `001-example-helloworld` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

This component is fully implemented. It provides a minimal reference demonstrating the Certus component framework's core patterns: interface definition, component declaration with receptacles, and actor-based message processing. No new implementation work is required.

## Technical Context

The component lives at `components/example-helloworld/` and consists of a single source file (`src/lib.rs`, ~110 lines). It depends on the workspace's `component-framework`, `component-core`, `interfaces`, and `logger` crates. It is consumed by the `apps/helloworld-mainline/` application which demonstrates full wiring.

### File Structure

```
components/example-helloworld/
  Cargo.toml          # Package manifest with workspace dependencies
  README.md           # Component documentation
  src/lib.rs          # All implementation (interface, component, actor handler)
  specs/              # This specification directory
```

## Architecture

```
┌─────────────────────────────────┐
│      HelloWorldComponent        │
│  ┌───────────┐  ┌───────────┐  │
│  │ IGreeter  │  │ ILogger   │  │
│  │ (provides)│  │(receptacle│  │
│  └───────────┘  └───────────┘  │
└─────────────────────────────────┘

┌─────────────────────────────────┐
│       GreeterHandler (Actor)    │
│  - count: u32                   │
│  - logger: Option<Arc<ILogger>> │
│  ┌────────────────────────┐     │
│  │ ActorHandler<GreetReq> │     │
│  │  on_start / handle /   │     │
│  │  on_stop               │     │
│  └────────────────────────┘     │
└─────────────────────────────────┘
```

- `HelloWorldComponent` is a static component providing the `IGreeter` trait.
- `GreeterHandler` is an actor handler activated via `Actor::simple(handler).activate()`.
- Messages (`GreetRequest`) are delivered through a lock-free channel to the actor's dedicated thread.

## Dependencies

| Crate | Usage |
|-------|-------|
| `component-framework` | `Actor`, `ActorHandler`, `define_component!`, `define_interface!` |
| `component-core` | `IUnknown` base trait (auto-derived) |
| `interfaces` | `ILogger` trait definition |
| `logger` | Concrete logger (used transitively in `apps/helloworld-mainline`) |

## Testing

- **Doc test**: The module-level `Quick start` example is a runnable doc test exercising actor creation, message sending, and deactivation.
- **Integration**: The `apps/helloworld-mainline/` application provides a full integration test with logger wiring.
- **CI**: Included in `cargo test --all` (default workspace members).

## Future Considerations

- Could add a unit test module with explicit assertions (currently relies on doc test only).
- Could demonstrate error handling patterns (e.g., actor channel full/disconnected).
- Could showcase `IUnknown::query_interface()` usage for runtime interface discovery.
- Could demonstrate third-party binding (currently only first-party binding is shown in the app).
