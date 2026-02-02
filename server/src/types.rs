use crate::errors::ParseCommandErr;
use bincode;
use bincode::Error;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;
use strum_macros::EnumString;

/// Represents a stock market quote with price, volume, and timestamp information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockQuote {
    /// Stock ticker symbol (e.g., "AAPL", "GOOGL")
    pub ticker: String,
    /// Current price of the stock
    pub price: f64,
    /// Trading volume
    pub volume: u32,
    /// Unix timestamp of the quote
    pub timestamp: u64,
}

impl StockQuote {
    /// Serializes the quote to a pipe-delimited string format.
    ///
    /// Format: `ticker|price|volume|timestamp`
    pub fn to_string(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.ticker, self.price, self.volume, self.timestamp
        )
    }

    /// Deserializes a quote from a pipe-delimited string.
    ///
    /// Returns `None` if the string format is invalid or parsing fails.
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() == 4 {
            Some(StockQuote {
                ticker: parts[0].to_string(),
                price: parts[1].parse().ok()?,
                volume: parts[2].parse().ok()?,
                timestamp: parts[3].parse().ok()?,
            })
        } else {
            None
        }
    }

    /// Serializes the quote to binary format using bincode.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(bincode::serialize(&self)?)
    }

    /// Deserializes a quote from binary format using bincode.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, Box<bincode::ErrorKind>> {
        bincode::deserialize(&bytes.as_slice())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UdpMessage {
    Quote(StockQuote),
    Ping { timestamp: u64 },
    Pong { timestamp: u64 },
}

impl UdpMessage {
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(bincode::serialize(&self)?)
    }

    /// Deserializes a quote from binary format using bincode.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, Box<bincode::ErrorKind>> {
        bincode::deserialize(&bytes.as_slice())
    }
}

/// Available commands for client requests.
#[derive(Debug, Clone, Copy, PartialEq, EnumString, strum_macros::Display)]
pub enum Commands {
    /// Request to stream stock quotes
    #[strum(ascii_case_insensitive, to_string = "STREAM")]
    Stream,
}

/// Network protocols supported for streaming.
#[derive(Debug, Clone, Copy, PartialEq, EnumString, strum_macros::Display)]
pub enum Protocol {
    /// User Datagram Protocol
    #[strum(ascii_case_insensitive, to_string = "UDP")]
    Udp,
}

/// Client subscription command containing connection details and requested tickers.
///
/// Format when serialized: `COMMAND|PROTOCOL|IP|PORT|TICKER1,TICKER2,...`
#[derive(Debug)]
pub struct SubscribeCommand {
    /// The command type
    pub cmd: Commands,
    /// Network protocol to use for streaming
    pub protocol: Protocol,
    /// IPv4 address where quotes should be sent
    pub ip: Ipv4Addr,
    /// Port number for the connection
    pub port: u16,
    /// List of stock ticker symbols to stream
    pub tickers_list: Vec<String>,
}

impl FromStr for SubscribeCommand {
    type Err = ParseCommandErr;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() < 5 {
            return Err(ParseCommandErr::NotEnoughArguments);
        }

        let cmd = Commands::from_str(parts[0])?;
        let protocol = Protocol::from_str(parts[1])?;
        let ip = Ipv4Addr::from_str(parts[2])?;
        let port = parts[3]
            .parse::<u16>()
            .map_err(|_| ParseCommandErr::InvalidPort)?;
        let tickers_list = parts[4].split(',').map(|s| s.to_string()).collect();

        Ok(SubscribeCommand {
            cmd,
            protocol,
            ip,
            port,
            tickers_list,
        })
    }
}

impl fmt::Display for SubscribeCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}|{}|{}|{}|{}",
            self.cmd,
            self.protocol,
            self.ip.clone(),
            self.port,
            self.tickers_list.clone().join(",")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stock_quote_to_from_string() {
        let quote = StockQuote {
            ticker: "AAPL".to_string(),
            price: 150.25,
            volume: 1000000,
            timestamp: 1234567890,
        };

        let s = quote.to_string();
        assert_eq!(s, "AAPL|150.25|1000000|1234567890");

        let parsed = StockQuote::from_string(&s).unwrap();
        assert_eq!(parsed.ticker, "AAPL");
        assert_eq!(parsed.price, 150.25);
        assert_eq!(parsed.volume, 1000000);
        assert_eq!(parsed.timestamp, 1234567890);
    }

    #[test]
    fn test_stock_quote_to_from_bytes() {
        let quote = StockQuote {
            ticker: "GOOGL".to_string(),
            price: 2800.50,
            volume: 500000,
            timestamp: 9876543210,
        };

        let bytes = quote.to_bytes().unwrap();
        let parsed = StockQuote::from_bytes(bytes).unwrap();

        assert_eq!(parsed.ticker, "GOOGL");
        assert_eq!(parsed.price, 2800.50);
        assert_eq!(parsed.volume, 500000);
        assert_eq!(parsed.timestamp, 9876543210);
    }

    #[test]
    fn test_subscribe_command_from_str_valid() {
        let input = "STREAM|UDP|192.168.1.1|8080|AAPL,GOOGL,MSFT";
        let cmd = SubscribeCommand::from_str(input).unwrap();

        assert_eq!(cmd.cmd, Commands::Stream);
        assert_eq!(cmd.protocol, Protocol::Udp);
        assert_eq!(cmd.ip, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(cmd.port, 8080);
        assert_eq!(cmd.tickers_list, vec!["AAPL", "GOOGL", "MSFT"]);
    }

    #[test]
    fn test_subscribe_command_display() {
        let cmd = SubscribeCommand {
            cmd: Commands::Stream,
            protocol: Protocol::Udp,
            ip: Ipv4Addr::new(10, 0, 0, 1),
            port: 9000,
            tickers_list: vec!["TSLA".to_string(), "NVDA".to_string()],
        };

        let output = format!("{}", cmd);
        assert_eq!(output, "STREAM|UDP|10.0.0.1|9000|TSLA,NVDA");
    }

    #[test]
    fn test_subscribe_command_invalid_ip() {
        let input = "STREAM|UDP|999.999.999.999|8080|AAPL";
        let result = SubscribeCommand::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_subscribe_command_not_enough_args() {
        let input = "STREAM|UDP|192.168.1.1";
        let result = SubscribeCommand::from_str(input);
        assert!(matches!(result, Err(ParseCommandErr::NotEnoughArguments)));
    }

    #[test]
    fn test_subscribe_command_invalid_port() {
        let input = "STREAM|UDP|192.168.1.1|invalid|AAPL";
        let result = SubscribeCommand::from_str(input);
        assert!(matches!(result, Err(ParseCommandErr::InvalidPort)));
    }

    // -- UdpMessage serialization tests --

    #[test]
    fn test_udp_message_quote_round_trip() {
        let quote = StockQuote {
            ticker: "TSLA".to_string(),
            price: 250.75,
            volume: 15000,
            timestamp: 1234567890,
        };
        let msg = UdpMessage::Quote(quote.clone());

        // Serialize
        let bytes = msg.to_bytes().unwrap();

        // Deserialize
        let decoded = UdpMessage::from_bytes(bytes).unwrap();

        // Verify it matches
        match decoded {
            UdpMessage::Quote(q) => {
                assert_eq!(q.ticker, "TSLA");
                assert_eq!(q.price, 250.75);
                assert_eq!(q.volume, 15000);
                assert_eq!(q.timestamp, 1234567890);
            }
            _ => panic!("Expected Quote variant"),
        }
    }

    #[test]
    fn test_udp_message_ping_pong_round_trip() {
        // Test Ping
        let ping = UdpMessage::Ping { timestamp: 9876543210 };
        let ping_bytes = ping.to_bytes().unwrap();
        let decoded_ping = UdpMessage::from_bytes(ping_bytes).unwrap();

        match decoded_ping {
            UdpMessage::Ping { timestamp } => {
                assert_eq!(timestamp, 9876543210);
            }
            _ => panic!("Expected Ping variant"),
        }

        // Test Pong
        let pong = UdpMessage::Pong { timestamp: 1111111111 };
        let pong_bytes = pong.to_bytes().unwrap();
        let decoded_pong = UdpMessage::from_bytes(pong_bytes).unwrap();

        match decoded_pong {
            UdpMessage::Pong { timestamp } => {
                assert_eq!(timestamp, 1111111111);
            }
            _ => panic!("Expected Pong variant"),
        }
    }
}
