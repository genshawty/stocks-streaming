LOG_LEVEL ?= info

.PHONY: all client server clean run-client run-server

all: client server

client:
	cargo build --package client

server:
	cargo build --package server

run-client-file:
	RUST_LOG=$(LOG_LEVEL) cargo run --package client -- -f server/src/data/tickers.txt

run-client:
	RUST_LOG=$(LOG_LEVEL) cargo run --package client -- -t AAPL

run-server:
	RUST_LOG=$(LOG_LEVEL) cargo run --package server -- -t=server/src/data/tickers.txt
