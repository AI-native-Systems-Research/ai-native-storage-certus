# helloworld-mainline

**Crate**: `helloworld-mainline`
**Path**: `apps/helloworld-mainline/`
**Type**: Application (not a component)

## Description

Minimal integration demo showing component instantiation, interface query, and actor lifecycle without SPDK dependencies.

Instantiates `HelloWorldComponent`, queries its `IGreeter` to print the prefix, then starts a `GreeterHandler` actor and sends four `GreetRequest` messages (`"World"`, `"Rust"`, `"Certus"`, `"Actors"`).

## Component Wiring

```
HelloWorldComponent ---[IGreeter]---> query greeting_prefix
                    \
                     Actor<GreetRequest, GreeterHandler> ---> send GreetRequests
```

## Build

```bash
cargo build -p helloworld-mainline
```
