pub mod command;
pub mod frame;
pub mod handshake;
pub mod resultset;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
