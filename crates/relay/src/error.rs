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
        source: hyper::Error,
    },

    #[error("health server failed: {0}")]
    HealthServer(#[from] hyper::Error),

    #[error("failed to wait for shutdown signal: {0}")]
    ShutdownSignal(#[source] std::io::Error),

    #[error("health server task failed: {0}")]
    HealthTask(#[source] tokio::task::JoinError),
}
