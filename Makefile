.PHONY: all client server clean run-client run-server

all: client server

client:
	cargo build --package client

server:
	cargo build --package server

run-client:
	cargo run --package client

run-server:
	cargo run --package server -- -t=server/src/data/tickers.txt

clean:
	cargo clean
