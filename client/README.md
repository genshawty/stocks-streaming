# Trading Stream Client

The client component of the trading stream system. It subscribes to stock tickers via TCP and receives real-time quotes via UDP.

## Features

- **TCP Subscription**: Connects to server and subscribes to specific tickers
- **UDP Quote Reception**: Receives real-time stock quotes
- **Automatic Reconnection**: Retries connection on failures with exponential backoff
- **Health Monitoring**: Sends periodic pings and monitors server responsiveness
- **RTT Tracking**: Measures round-trip time for connection health
- **Graceful Shutdown**: Handles Ctrl+C with clean resource cleanup

## Usage

### Basic Usage

Subscribe to specific tickers:
```bash
cargo run --package client -- -t AAPL,GOOGL,MSFT
```

Or using Make:
```bash
make run-client          # Subscribes to AAPL
make run-client-file     # Uses tickers from file
```

### Command-Line Options

```bash
cargo run --package client -- [OPTIONS]

Options:
  -m, --master-addr <ADDR>       Master server address [default: 127.0.0.1]
  -p, --master-tcp-port <PORT>   Master TCP port [default: 8090]
  -u, --worker-udp-port <PORT>   Worker UDP port [default: 8091]
  -t, --tickers <TICKERS>        Stock tickers (comma-separated)
  -f, --tickers-file <PATH>      Path to tickers file (one per line)
  -h, --help                     Print help
```

### Examples

Subscribe to multiple tickers:
```bash
cargo run --package client -- -t AAPL,GOOGL,TSLA
```

Use a tickers file:
```bash
cargo run --package client -- -f tickers.txt
```

Connect to remote server:
```bash
cargo run --package client -- -m 192.168.1.100 -p 8090 -t AAPL
```

With debug logging:
```bash
RUST_LOG=debug cargo run --package client -- -t AAPL
```

## Architecture

### Components

1. **Worker** ([worker.rs](./src/worker.rs))
   - TCP connection management with automatic reconnection
   - UDP listener for receiving quotes
   - Ping/pong health monitoring
   - Graceful shutdown coordination

2. **Protocol Handler**
   - Parses length-prefixed TCP messages
   - Deserializes UDP stock quotes
   - Handles server acknowledgments and errors

### Connection Flow

1. Client attempts TCP connection to server
2. On success, sends Subscribe command with ticker list
3. Spawns UDP listener thread on specified port
4. Waits for server acknowledgment
5. Starts periodic ping thread after first UDP packet
6. Prints received quotes to stdout
7. Monitors connection health via ping/pong RTT
8. Auto-shutdown if pings go unanswered for 20 seconds

### Health Monitoring

- Pings sent every 2 seconds
- Tracks last 10 ping RTT measurements
- Shuts down if all 10 pings are unanswered (20 second timeout)
- Reconnects automatically on connection loss

## Configuration

### Tickers File Format

One ticker symbol per line:
```
AAPL
GOOGL
MSFT
```

### Environment Variables

- `RUST_LOG`: Set log level (error, warn, info, debug, trace)

## Development

### Building

```bash
cargo build --package client
```

### Running Tests

```bash
cargo test --package client
```

### Project Structure

```
client/
├── src/
│   ├── main.rs           # Entry point and CLI parsing
│   ├── worker.rs         # TCP/UDP client implementation
│   └── errors.rs         # Error types
└── Cargo.toml
```

## Troubleshooting

### Connection Issues

If the client fails to connect:
1. Verify server is running: `make run-server`
2. Check firewall allows TCP port 8090 and UDP port 8091
3. Verify server address with `-m` flag
4. Enable debug logging: `RUST_LOG=debug`

### No Quotes Received

If connected but not receiving quotes:
1. Verify tickers exist in server's tickers file
2. Check UDP port 8091 is available
3. Look for firewall blocking UDP traffic
4. Check server logs for errors

### Ping Timeout

If client shuts down with "no pongs received":
1. Network connectivity issues
2. Server overloaded or crashed
3. Firewall blocking UDP responses
4. Check server logs for errors