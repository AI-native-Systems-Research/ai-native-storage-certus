# component-macros

**Crate**: `component-macros`
**Path**: `lib/component-framework/crates/component-macros/`
**Version**: 0.1.0

## Description

Procedural macros for the Certus COM-style component framework. Generates boilerplate for interface and component definitions.

## Macros

### `define_interface!`

Generates a trait with `Send + Sync + 'static` bounds and an `Interface` implementation for the trait-object type. All methods must take `&self` and at least one method is required.

```rust
define_interface! {
    trait IGreeter {
        fn greeting_prefix(&self) -> &str;
    }
}
```

### `define_component!`

Generates a struct with auto-implemented `IUnknown`, interface map population, receptacle fields, and an `Arc<Self>`-returning `new_default()` constructor.

```rust
define_component! {
    MyComponent {
        version: "0.1.0",
        provides: [IGreeter],
        receptacles: { logger: ILogger },
        fields: { counter: AtomicU64 },
    }
}
```

## Interfaces Provided

None (proc-macro crate).

## Receptacles

None.
