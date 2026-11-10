# example-helloworld

**Crate**: `example-helloworld`
**Path**: `components/example-helloworld/`
**Version**: 0.1.0

## Description

Demonstration component that shows how to define an interface, implement a component, and use the actor model. Defines a local `IGreeter` interface and a `HelloWorldComponent` that implements it. Also provides `GreeterHandler`, an `ActorHandler` that prints greetings and optionally logs via an injected `ILogger`.

## Component Definition

```
HelloWorldComponent {
    version: "0.1.0",
    provides: [IGreeter],
    receptacles: { logger: ILogger },
}
```

## Interfaces Provided

| Interface | Methods |
|-----------|---------|
| `IGreeter` (local) | `greeting_prefix(&self) -> &str` -- returns `"Hello"` |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging of greeting events |

## Actor Support

- `GreeterHandler` -- `ActorHandler<GreetRequest>` that prints greetings and tracks count
- `GreetRequest { name: String }` -- actor message type
- Constructors: `new()`, `with_logger(Arc<dyn ILogger>)`
