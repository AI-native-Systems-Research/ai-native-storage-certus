# Feature Specification: Example Hello World Component

**Feature Branch**: `001-example-helloworld`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `example-helloworld` component is a demonstration component that illustrates core Certus component framework patterns. It showcases:

1. Defining a custom interface (`IGreeter`) via the `define_interface!` macro.
2. Implementing a component (`HelloWorldComponent`) via the `define_component!` macro.
3. Declaring a receptacle dependency (`ILogger`) for optional structured logging.
4. Using the actor model (`GreeterHandler`) to process messages on a dedicated thread.

The component serves as a reference implementation and onboarding tool for developers new to the Certus component framework.

## User Scenarios & Testing

### User Story 1 - Learning the Component Framework (Priority: P1)

**As a** developer new to Certus,
**I want** a minimal working example of a component with an actor,
**So that** I can understand how interfaces, components, receptacles, and actors fit together.

**Acceptance Criteria:**
- The component compiles with `cargo build -p example-helloworld`.
- The actor can be activated, sent messages, and deactivated without errors.
- The code is well-documented with doc comments and a runnable example in the module docs.

### User Story 2 - Demonstrating Receptacle Wiring (Priority: P2)

**As a** developer,
**I want** to see how an optional `ILogger` receptacle is consumed by an actor handler,
**So that** I understand how to wire dependencies between components.

**Acceptance Criteria:**
- `GreeterHandler::with_logger(...)` accepts an `Arc<dyn ILogger + Send + Sync>`.
- When a logger is wired, lifecycle events (`on_start`, `on_stop`) and message handling produce log entries.
- When no logger is wired, the component still functions (prints to stdout/stderr only).

## Requirements

### Functional Requirements

| ID | Requirement | Status |
|----|-------------|--------|
| FR-1 | Define `IGreeter` interface with a `greeting_prefix(&self) -> &str` method | Implemented |
| FR-2 | `HelloWorldComponent` provides the `IGreeter` interface, returning `"Hello"` as the prefix | Implemented |
| FR-3 | `HelloWorldComponent` declares an `ILogger` receptacle | Implemented |
| FR-4 | `GreetRequest` message type carries a `name: String` field | Implemented |
| FR-5 | `GreeterHandler` implements `ActorHandler<GreetRequest>` with lifecycle hooks | Implemented |
| FR-6 | `handle()` increments an internal counter and prints a numbered greeting | Implemented |
| FR-7 | Optional `ILogger` integration logs on start, each greeting, and stop | Implemented |

### Non-Functional Requirements

| ID | Requirement | Status |
|----|-------------|--------|
| NFR-1 | Module-level doc comment with a runnable `Quick start` example | Implemented |
| NFR-2 | Zero `clippy` warnings under `-D warnings` | Implemented |
| NFR-3 | No unsafe code required | Implemented |

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `IGreeter` | Interface trait | Provides `greeting_prefix()` |
| `HelloWorldComponent` | Component struct | Implements `IGreeter`, declares `ILogger` receptacle |
| `GreeterHandler` | Actor handler | Processes `GreetRequest` messages on a dedicated thread |
| `GreetRequest` | Message struct | Contains `name: String` for the greeting target |

## Dependencies

| Dependency | Role |
|------------|------|
| `component-framework` | Core framework (Actor, ActorHandler, macros) |
| `component-core` | Base traits (IUnknown) |
| `interfaces` | Shared interface definitions (ILogger) |
| `logger` | Logger component implementation; a dependency of this component's `Cargo.toml`, but not currently wired by `apps/helloworld-mainline` (see Implementation Notes) |

## Success Criteria

- Component builds and tests pass: `cargo test -p example-helloworld`
- Module doc example compiles and runs successfully via `cargo test --doc -p example-helloworld`
- Serves as a clear, minimal reference for new component authors

## Implementation Notes

- The `IGreeter` interface is defined locally in `lib.rs` rather than in the shared `interfaces` crate, demonstrating that interfaces can be component-local.
- The actor handler uses `eprintln!` for lifecycle status and `println!` for greeting output, separating concerns between diagnostic and application output.
- The `Default` impl delegates to `new()`, satisfying `clippy::new_without_default`.
- `apps/helloworld-mainline/` runs the component's actor without a logger wired (`GreeterHandler::new()`); it does not depend on the `logger` crate and does not demonstrate `ILogger` wiring. Logger integration (`GreeterHandler::with_logger(...)`) is currently exercised only at the unit/doc-test level within this component, not by the mainline app. (Corrected 2026-07-22 via spec-sync; previously stated a full logger-wiring integration example existed in the app.)
