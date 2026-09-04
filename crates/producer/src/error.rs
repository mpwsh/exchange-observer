//! Producer error type.
//!
//! Uses `thiserror` for a proper error hierarchy. `anyhow` is reserved for the
//! binary entry point (`main`), where erasing typed context is acceptable.

use tokio_tungstenite::tungstenite;

/// Errors that can occur inside the producer.
#[derive(Debug, thiserror::Error)]
pub enum ProducerError {
    #[error("websocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),

    #[error("invalid websocket url: {0}")]
    Url(#[from] url::ParseError),

    #[error("tls setup failed: {0}")]
    Tls(#[from] native_tls::Error),

    #[error("json (de)serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("kafka client error: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("kafka producer error: {0}")]
    KafkaProduce(#[from] rskafka::client::producer::Error),

    #[error("exchange symbol lookup failed: {0}")]
    Symbols(#[from] crypto_markets::Error),

    #[error("task join failed: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("unknown channel: {0}")]
    UnknownChannel(String),

    #[error("missing field in message: {0}")]
    MissingField(&'static str),

    #[error("websocket closed by peer")]
    Disconnected,
}

pub type Result<T> = std::result::Result<T, ProducerError>;
