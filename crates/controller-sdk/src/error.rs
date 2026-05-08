use thiserror::Error;

pub type Result<T> = std::result::Result<T, ControllerSdkError>;

#[derive(Debug, Error)]
pub enum ControllerSdkError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("grpc error: {0}")]
    Grpc(String),

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

impl From<tonic::transport::Error> for ControllerSdkError {
    fn from(value: tonic::transport::Error) -> Self {
        ControllerSdkError::Transport(value.to_string())
    }
}

impl From<tonic::Status> for ControllerSdkError {
    fn from(value: tonic::Status) -> Self {
        ControllerSdkError::Grpc(value.to_string())
    }
}
