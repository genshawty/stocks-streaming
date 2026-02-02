# Trading Stream

A Rust-based real-time stock quote streaming system with a client-server architecture. The server generates and broadcasts stock quotes to subscribed clients over UDP, with TCP used for subscription management.

## Architecture

- **Server**: Generates random stock quotes and streams them to clients via UDP
- **Client**: Subscribes to specific tickers via TCP and receives real-time quotes via UDP
- **Health Checks**: Automatic ping/pong mechanism to detect connection issues

## Quick Start

### Prerequisites

- Rust toolchain (1.70+)
- Make (optional, for convenience)

### Building

Build both server and client:
```bash
make all
```

Or build individually:
```bash
make server
make client
```

Or using cargo directly:
```bash
cargo build --workspace
```

### Running

#### Start the server:
```bash
make run-server
```

Or with custom options:
```bash
cargo run --package server -- -p 8090 -t server/src/data/tickers.txt
```

#### Start the client:

Subscribe to specific tickers:
```bash
make run-client
```

Or subscribe using a tickers file:
```bash
make run-client-file
```

Or with custom options:
```bash
cargo run --package client -- -t AAPL,GOOGL,MSFT
cargo run --package client -- -f path/to/tickers.txt
```

### Makefile Targets

- `make all` - Build both server and client
- `make server` - Build server only
- `make client` - Build client only
- `make run-server` - Run server with default settings
- `make run-client` - Run client subscribing to AAPL
- `make run-client-file` - Run client with tickers from file

### Log Level

Set log level via `LOG_LEVEL` environment variable (default: info):
```bash
LOG_LEVEL=debug make run-server
LOG_LEVEL=debug make run-client
```

Or directly with cargo:
```bash
RUST_LOG=debug cargo run --package server
```

## Documentation

- [Server Documentation](./server/README.md)
- [Client Documentation](./client/README.md)

## Project Structure

```
.
├── server/          # Server implementation
├── client/          # Client implementation
├── Makefile         # Build and run commands
└── Cargo.toml       # Workspace configuration
```

## Known Issues

### Immediate Client Restart

**When**: Stopping a client with Ctrl+C and immediately restarting it may cause connection issues.

**Why**:
- The OS may not immediately release the UDP socket (port 8091) after the client is killed
- Both the old (dead) and new client sockets may briefly coexist, causing UDP packet delivery conflicts
- This can prevent the new client from receiving quotes and starting its ping mechanism

**Workaround**: Wait 5+ seconds between stopping and restarting, or use a different UDP port (`-u` flag) for each client instance.