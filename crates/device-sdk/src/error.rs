use thiserror::Error;

pub type Result<T> = std::result::Result<T, DeviceSdkError>;

#[derive(Debug, Error)]
pub enum DeviceSdkError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("tonic transport error: {0}")]
    Tonic(#[from] tonic::transport::Error),

    #[error("grpc status: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("time error: {0}")]
    Time(String),

    #[error("connection closed")]
    ConnectionClosed,

    #[error("device token/auth required but not set")]
    MissingToken,

    #[error("previous_connection_id provided but session recovery not enabled")]
    RecoveryDisabled,
}
