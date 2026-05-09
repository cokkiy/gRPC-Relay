use thiserror::Error;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("failed to load config: {0}")]
    Config(#[from] config::ConfigError),

    #[error("failed to read file `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TLS config: {0}")]
    InvalidTlsConfig(String),

    #[error("invalid socket address `{address}`: {source}")]
    InvalidSocketAddress {
        address: String,
        #[source]
        source: std::net::AddrParseError,
    },

    #[error("failed to bind health server to `{address}`: {source}")]
    HealthBind {
        address: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },

    #[error("health server failed: {0}")]
    HealthServer(#[from] std::io::Error),

    #[error("failed to wait for shutdown signal: {0}")]
    ShutdownSignal(#[source] std::io::Error),

    #[error("health server task failed: {0}")]
    HealthTask(#[source] tokio::task::JoinError),

    #[error("gRPC server failed: {0}")]
    GrpcServer(#[from] tonic::transport::Error),

    #[error("gRPC server task failed: {0}")]
    GrpcTask(#[source] tokio::task::JoinError),

    #[error("stream router: {0}")]
    StreamRouter(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("rate limit exceeded: {entity_type} {entity_id}")]
    RateLimited {
        entity_type: &'static str,
        entity_id: String,
    },

    #[error("max streams exceeded for device {device_id} (max: {max})")]
    MaxStreamsExceeded { device_id: String, max: u32 },
}
