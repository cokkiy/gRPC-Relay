use thiserror::Error;

pub type Result<T> = std::result::Result<T, ControllerSdkError>;

#[derive(Debug, Error)]
pub enum ControllerSdkError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("tonic transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("grpc status: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("unauthorized")]
    Unauthorized,

    #[error("device offline")]
    DeviceOffline,

    #[error("device not found")]
    DeviceNotFound,

    #[error("rate limited")]
    RateLimited,

    #[error("payload too large")]
    PayloadTooLarge,

    #[error("internal error")]
    InternalError,

    #[error("stream closed")]
    StreamClosed,

    #[error("sequence number response not found (request timed out)")]
    SequenceResponseNotFound,
}
