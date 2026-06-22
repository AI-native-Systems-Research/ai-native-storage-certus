# Contract: IRemoteLookup::batch_lookup

## Interface

**Crate**: `interfaces`
**File**: `components/interfaces/src/iremote_lookup.rs`
**Trait**: `IRemoteLookup`

## Method Signature

```rust
fn batch_lookup(
    &self,
    entries: &[(CacheKey, IpcHandle)],
) -> Vec<Result<(), RemoteLookupError>>;
```

## Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `entries` | `&[(CacheKey, IpcHandle)]` | Slice of (key, destination) pairs to look up |

## Return Value

`Vec<Result<(), RemoteLookupError>>` — one result per input entry, preserving positional order.

## Behavior Contract

### Pre-conditions

- Component must be instantiated via `define_component!`
- No requirement on connection state (method handles both cases)

### Post-conditions (placeholder)

| Condition | Behavior |
|-----------|----------|
| Not connected | All entries return `Err(RemoteLookupError::NotConnected)` |
| Connected, non-empty slice | Each entry logged via ILogger, returns `Err(RemoteLookupError::NotFound)` |
| Connected, empty slice | Returns empty `Vec` |

### Invariants

- Return vec length == input slice length (always)
- Result order matches input order (positional correspondence)
- No side effects beyond logging (placeholder)
- Method never panics

## Error Variants

| Variant | When |
|---------|------|
| `NotConnected` | `is_connected()` returns `false` |
| `NotFound` | Placeholder behavior when connected |
| `TransportError(_)` | Reserved for future network implementation |
