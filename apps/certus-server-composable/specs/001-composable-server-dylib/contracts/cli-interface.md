# CLI Interface Contract: certus-server-composable

## Usage

```
certus-server-composable --config <path> [OPTIONS]
```

## Required Arguments

| Argument | Description |
|----------|-------------|
| `--config <path>` | Path to JSON configuration file (mandatory) |

## Optional Arguments (override JSON config values)

| Argument | Description | Default |
|----------|-------------|---------|
| `--listen <addr>` | gRPC listen address | `0.0.0.0:50051` |
| `--device-pci <addr>` | NVMe PCI address (repeatable) | from config |
| `--drive-count <N>` | Auto-select first N NVMe drives | from config |
| `--memory-tier-size <size>` | Memory pool size (e.g., 2G) | from config |
| `--format` | Format extent managers on startup | `false` |
| `--tls-cert <path>` | TLS certificate file | none |
| `--tls-key <path>` | TLS private key file | none |
| `--poller-base-cpu <N>` | Base CPU core for poller pinning | none |

## Precedence

CLI arguments always override values specified in the JSON configuration file.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Clean shutdown (SIGTERM/SIGINT received) |
| 1 | Configuration error (parse failure, validation error, missing dylib) |
| 2 | Component initialization failure (dylib load error, create_component panic, bind failure) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CERTUS_LIB_PATH` | Colon-separated list of directories prepended to the search path |
