# component-core

**Crate**: `component-core`
**Path**: `lib/component-framework/crates/component-core/`
**Version**: 0.1.0

## Description

Core runtime for the Certus COM-style component framework. Defines the fundamental traits, types, and primitives that all components build on. This is not a component itself -- it is the framework that components are built with.

## Key Traits

| Trait | Purpose |
|-------|---------|
| `IUnknown` | Base trait every component implements. Provides `query_interface_raw`, `version`, `provided_interfaces`, `receptacles`, `connect_receptacle_raw`. |
| `ActorHandler<M>` | Message handler for actor threads. Methods: `handle`, `on_start`, `on_stop`, `on_idle`. |
| `ISender<M>` / `IReceiver<M>` | Generic channel send/receive traits. |
| `ComponentFactory` | Blanket-implemented for closures; used by `ComponentRegistry`. |

## Key Types

| Type | Purpose |
|------|---------|
| `Receptacle<T>` | Thread-safe typed slot (`RwLock<Option<Arc<T>>>`) for declaring required interface dependencies. Methods: `connect`, `disconnect`, `get`, `is_connected`. |
| `InterfaceMap` | Runtime `HashMap<TypeId, Arc<dyn Any + Send + Sync>>` storing a component's provided interfaces. |
| `ComponentRef` | Type-erased `Arc<dyn IUnknown>`. |
| `ComponentRegistry` | Thread-safe string-keyed factory registry. Methods: `register`, `create`, `list`. |
| `Actor<M, H>` | Thread-owning actor component implementing `IUnknown`. Spawns a dedicated OS thread, exposes `ISender<M>` via interface query. |
| `ActorHandle<M>` | Handle returned by `Actor::activate()`. Methods: `send`, `try_send`, `deactivate`. |
| `CpuSet` / `NumaTopology` | NUMA-aware CPU pinning utilities. |

## Channel Implementations

- `SpscChannel` (rtrb-backed single-producer/single-consumer)
- `CrossbeamBoundedChannel` / `CrossbeamUnboundedChannel`
- `KanalChannel`
- `MpscChannel` / `MpscSender` / `MpscReceiver`
- `TokioMpscChannel`

## Free Functions

- `query<I>(component) -> Option<Arc<I>>` -- typed wrapper for `IUnknown::query_interface_raw`
- `bind(provider, iface_name, consumer, receptacle_name)` -- wires a provider's interface to a consumer's receptacle
- `pipe` / `pipe_mpsc` -- forwarder utilities connecting a channel receiver to an actor handle

## Interfaces Provided

None (framework crate, not a component).

## Receptacles

None.
