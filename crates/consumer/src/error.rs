//! Consumer error type. `anyhow` reserved for the binary entry point.

use scylla::errors::{ExecutionError, NewSessionError, PrepareError};

#[derive(Debug, thiserror::Error)]
pub enum ConsumerError {
    #[error("kafka client error: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("scylla connect failed: {0}")]
    ScyllaConnect(#[from] NewSessionError),

    #[error("scylla prepare failed: {0}")]
    ScyllaPrepare(#[from] PrepareError),

    #[error("scylla execute failed: {0}")]
    ScyllaExecute(#[from] ExecutionError),

    #[error("json (de)serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unknown channel in message headers: {0}")]
    UnknownChannel(String),

    #[error("missing required record field: {0}")]
    MissingField(&'static str),

    #[error("payload parse failed for channel {channel}: {source}")]
    PayloadParse {
        channel: String,
        #[source]
        source: anyhow::Error,
    },
}

pub type Result<T> = std::result::Result<T, ConsumerError>;
