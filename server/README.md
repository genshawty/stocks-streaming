# Trading Stream Server

The server component of the trading stream system. It generates random stock quotes and broadcasts them to subscribed clients via UDP.

## Features

- **TCP Subscription Server**: Accepts client connections on TCP port 8090 (default)
- **UDP Quote Streaming**: Sends stock quotes to subscribed clients via UDP
- **Concurrent Clients**: Handles multiple simultaneous client subscriptions
- **Health Monitoring**: Responds to client ping messages with pongs
- **Graceful Shutdown**: Clean resource cleanup on Ctrl+C

## Usage

### Basic Usage

Start the server with default settings:
```bash
cargo run --package server
```

Or using Make:
```bash
make run-server
```

### Command-Line Options

```bash
cargo run --package server -- [OPTIONS]

Options:
  -p, --port <PORT>              TCP port to listen on [default: 8090]
  -t, --tickers-file <PATH>      Path to tickers file [default: src/data/tickers.txt]
  -h, --help                     Print help
```

### Examples

Run on custom port with specific tickers file:
```bash
cargo run --package server -- -p 9000 -t /path/to/tickers.txt
```

With debug logging:
```bash
RUST_LOG=debug cargo run --package server
```

## Architecture

### Components

1. **Processor** ([processor.rs](./src/processor.rs))
   - TCP server accepting client subscriptions
   - Manages client registry and message routing
   - Handles graceful shutdown

2. **Generator** ([generator.rs](./src/generator.rs))
   - Generates random stock quotes at regular intervals
   - Updates prices with realistic random walks
   - Broadcasts quotes to subscribed clients

3. **Protocol** ([types.rs](./src/types.rs))
   - Message framing with length-prefixed protocol
   - Serialization using custom text format
   - TCP messages: Subscribe, Ack, Error
   - UDP messages: Quote, Ping, Pong

### Message Flow

1. Client connects via TCP
2. Client sends Subscribe command with ticker list and UDP endpoint
3. Server validates subscription and responds with Ack or Error
4. Server broadcasts matching quotes to client's UDP endpoint
5. Client sends periodic Ping messages, server responds with Pong
6. Connection maintained until client disconnects or ping timeout

## Configuration

### Tickers File Format

One ticker symbol per line:
```
AAPL
GOOGL
MSFT
TSLA
```

Default location: `server/src/data/tickers.txt`

### Environment Variables

- `RUST_LOG`: Set log level (error, warn, info, debug, trace)

## Development

### Building

```bash
cargo build --package server
```

### Running Tests

```bash
cargo test --package server
```

### Project Structure

```
server/
├── src/
│   ├── main.rs           # Entry point
│   ├── processor.rs      # TCP server and client management
│   ├── generator.rs      # Quote generation engine
│   ├── types.rs          # Protocol types and serialization
│   ├── errors.rs         # Error types
│   └── data/
│       └── tickers.txt   # Default ticker list
└── Cargo.toml
```