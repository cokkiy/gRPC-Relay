use crate::{DeviceSdkError, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceSdkConfig {
    pub relay: RelayEndpointConfig,
    pub device_id: String,
    pub token: String,

    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,

    #[serde(default = "default_recovery_window_seconds")]
    pub session_recovery_window_seconds: u64,

    #[serde(default = "default_heartbeat_interval_seconds")]
    pub heartbeat_interval_seconds: u64,

    #[serde(default = "default_backoff_initial_seconds")]
    pub backoff_initial_seconds: u64,

    #[serde(default = "default_backoff_max_seconds")]
    pub backoff_max_seconds: u64,

    #[serde(default)]
    pub transport: TransportConfig,
}

impl DeviceSdkConfig {
    /// Load SDK configuration from a YAML/TOML/JSON file and `STATION_SERVICE__*`
    /// environment variables.
    ///
    /// Environment variables use `__` as a nesting separator, for example:
    /// `STATION_SERVICE__RELAY__TCP_ADDR=127.0.0.1:50051`.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let builder = config::Config::builder()
            .add_source(config::File::from(path).required(true))
            .add_source(
                config::Environment::with_prefix("STATION_SERVICE")
                    .separator("__")
                    .try_parsing(true),
            );

        let config = builder
            .build()
            .map_err(|err| DeviceSdkError::InvalidConfig(err.to_string()))?
            .try_deserialize::<Self>()
            .map_err(|err| DeviceSdkError::InvalidConfig(err.to_string()))?;

        config.validate()?;
        Ok(config)
    }

    /// Build a minimal configuration directly from environment variables.
    ///
    /// This is intended for simple deployments and examples. Use [`Self::load`]
    /// when stationService needs a versioned config file.
    pub fn from_env() -> Result<Self> {
        let relay_tcp_addr = std::env::var("RELAY_TCP_ADDR")
            .map_err(|_| DeviceSdkError::InvalidConfig("RELAY_TCP_ADDR is required".into()))?;
        let device_id = std::env::var("DEVICE_ID")
            .map_err(|_| DeviceSdkError::InvalidConfig("DEVICE_ID is required".into()))?;
        let token = std::env::var("DEVICE_TOKEN")
            .map_err(|_| DeviceSdkError::InvalidConfig("DEVICE_TOKEN is required".into()))?;

        let metadata = metadata_from_env("DEVICE_METADATA_");
        let config = Self {
            relay: RelayEndpointConfig {
                tcp_addr: relay_tcp_addr,
                quic_addr: std::env::var("RELAY_QUIC_ADDR").ok(),
            },
            device_id,
            token,
            metadata,
            session_recovery_window_seconds: read_u64_env(
                "SESSION_RECOVERY_WINDOW_SECONDS",
                default_recovery_window_seconds(),
            )?,
            heartbeat_interval_seconds: read_u64_env(
                "HEARTBEAT_INTERVAL_SECONDS",
                default_heartbeat_interval_seconds(),
            )?,
            backoff_initial_seconds: read_u64_env(
                "BACKOFF_INITIAL_SECONDS",
                default_backoff_initial_seconds(),
            )?,
            backoff_max_seconds: read_u64_env(
                "BACKOFF_MAX_SECONDS",
                default_backoff_max_seconds(),
            )?,
            transport: TransportConfig {
                max_payload_bytes: read_usize_env(
                    "MAX_PAYLOAD_BYTES",
                    default_max_payload_bytes(),
                )?,
                enable_tcp_fallback: read_bool_env(
                    "ENABLE_TCP_FALLBACK",
                    default_enable_tcp_fallback(),
                )?,
            },
        };

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.relay.tcp_addr.trim().is_empty() {
            return Err(DeviceSdkError::InvalidConfig(
                "relay.tcp_addr must not be empty".into(),
            ));
        }
        if self.device_id.trim().is_empty() {
            return Err(DeviceSdkError::InvalidConfig(
                "device_id must not be empty".into(),
            ));
        }
        if self.token.trim().is_empty() {
            return Err(DeviceSdkError::MissingToken);
        }
        if self.heartbeat_interval_seconds == 0 {
            return Err(DeviceSdkError::InvalidConfig(
                "heartbeat_interval_seconds must be greater than 0".into(),
            ));
        }
        if self.backoff_initial_seconds == 0 || self.backoff_max_seconds == 0 {
            return Err(DeviceSdkError::InvalidConfig(
                "backoff settings must be greater than 0".into(),
            ));
        }
        if self.transport.max_payload_bytes == 0 {
            return Err(DeviceSdkError::InvalidConfig(
                "transport.max_payload_bytes must be greater than 0".into(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelayEndpointConfig {
    /// DeviceConnect 客户端连接入口（HTTP/TLS fallback）
    pub tcp_addr: String,

    /// QUIC 连接地址（如果后续在该 repo 落地 QUIC transport，则优先使用）
    #[serde(default)]
    pub quic_addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransportConfig {
    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: usize,

    /// 是否允许在 QUIC 不可用时 fallback 到 TCP/TLS
    #[serde(default = "default_enable_tcp_fallback")]
    pub enable_tcp_fallback: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: default_max_payload_bytes(),
            enable_tcp_fallback: default_enable_tcp_fallback(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceSdkTlsConfig {
    #[serde(default)]
    pub ca_file: Option<String>,
    #[serde(default)]
    pub cert_file: Option<String>,
    #[serde(default)]
    pub key_file: Option<String>,
}

fn default_recovery_window_seconds() -> u64 {
    300
}

fn default_heartbeat_interval_seconds() -> u64 {
    30
}

fn default_backoff_initial_seconds() -> u64 {
    1
}

fn default_backoff_max_seconds() -> u64 {
    60
}

fn default_max_payload_bytes() -> usize {
    10 * 1024 * 1024 // 10MB
}

fn default_enable_tcp_fallback() -> bool {
    true
}

fn metadata_from_env(prefix: &str) -> HashMap<String, String> {
    std::env::vars()
        .filter_map(|(key, value)| {
            key.strip_prefix(prefix)
                .map(|stripped| (stripped.to_ascii_lowercase().replace("__", "."), value))
        })
        .collect()
}

fn read_u64_env(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|err| DeviceSdkError::InvalidConfig(format!("{name} must be u64: {err}"))),
        Err(_) => Ok(default),
    }
}

fn read_usize_env(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|err| DeviceSdkError::InvalidConfig(format!("{name} must be usize: {err}"))),
        Err(_) => Ok(default),
    }
}

fn read_bool_env(name: &str, default: bool) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<bool>()
            .map_err(|err| DeviceSdkError::InvalidConfig(format!("{name} must be bool: {err}"))),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validate_ok() {
        let cfg = DeviceSdkConfig {
            relay: RelayEndpointConfig {
                tcp_addr: "127.0.0.1:50051".into(),
                quic_addr: None,
            },
            device_id: "dev-1".into(),
            token: "tok-1".into(),
            metadata: HashMap::new(),
            session_recovery_window_seconds: 300,
            heartbeat_interval_seconds: 30,
            backoff_initial_seconds: 1,
            backoff_max_seconds: 60,
            transport: TransportConfig::default(),
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn config_validate_fails_empty_device_id() {
        let cfg = DeviceSdkConfig {
            device_id: "".into(),
            token: "tok-1".into(),
            backoff_initial_seconds: 1,
            backoff_max_seconds: 60,
            heartbeat_interval_seconds: 30,
            ..build_valid()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_fails_empty_token() {
        let cfg = DeviceSdkConfig {
            device_id: "dev-1".into(),
            token: "".into(),
            backoff_initial_seconds: 1,
            backoff_max_seconds: 60,
            heartbeat_interval_seconds: 30,
            ..build_valid()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_fails_zero_heartbeat() {
        let cfg = DeviceSdkConfig {
            heartbeat_interval_seconds: 0,
            ..build_valid()
        };
        assert!(cfg.validate().is_err());
    }

    fn build_valid() -> DeviceSdkConfig {
        DeviceSdkConfig {
            relay: RelayEndpointConfig {
                tcp_addr: "127.0.0.1:50051".into(),
                quic_addr: None,
            },
            device_id: "dev-1".into(),
            token: "tok-1".into(),
            metadata: HashMap::new(),
            session_recovery_window_seconds: 300,
            heartbeat_interval_seconds: 30,
            backoff_initial_seconds: 1,
            backoff_max_seconds: 60,
            transport: TransportConfig::default(),
        }
    }
}
