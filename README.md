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

### Immediate Client Restart Issue

**Problem**: When stopping a client with Ctrl+C and immediately restarting it, the new client may be disconnected after 5 seconds with "no ping" error, even though the old client has stopped.

**Root Cause**:
- The server's `monitor_connection` function initializes the ping timeout timer (`last_ping = Instant::now()`) immediately when a subscriber is created, rather than waiting for the first ping
- The client only starts sending pings after receiving its first UDP packet from the server
- This creates a race condition where the client may not send its first ping within the 5-second timeout window

**Contributing Factors**:
- UDP port binding conflicts when restarting quickly (both old and new client trying to use the same port)
- OS may not release the UDP socket immediately after process termination
- Potential packet loss during the overlap period

**Workarounds**:
- Wait 5+ seconds between stopping and restarting the client
- Use a different UDP port for each client instance (`-u` flag)

**TODO - Potential Fixes**:

1. **Server-side** (Recommended): Modify `monitor_connection` in [server/src/processor.rs:421-479](server/src/processor.rs#L421-L479) to not start the timeout until the first ping is received:
   ```rust
   let mut last_ping: Option<Instant> = None;
   // Don't check timeout until first ping received
   ```

2. **Client-side**: Enable `SO_REUSEADDR` on the UDP socket in [client/src/worker.rs:100](client/src/worker.rs#L100) to allow immediate port rebinding:
   ```rust
   // Add socket2 = "0.5" to Cargo.toml
   use socket2::{Domain, Protocol, Socket, Type};
   let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
   socket.set_reuse_address(true)?;
   socket.bind(&addr.into())?;
   let socket: UdpSocket = socket.into();
   ```

3. **Server-side**: Detect and remove duplicate subscribers sending to the same UDP endpoint before creating a new subscriber