use crate::{AppError, Result};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub relay: RelayConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelayConfig {
    pub id: String,
    pub address: String,
    #[serde(default = "default_quic_address")]
    pub quic_address: String,
    #[serde(default = "default_max_device_connections")]
    pub max_device_connections: u32,
    #[serde(default = "default_heartbeat_interval_seconds")]
    pub heartbeat_interval_seconds: u64,
    #[serde(default)]
    pub stream: StreamConfig,
    #[serde(default)]
    pub rate_limiting: RateLimitConfig,
    #[serde(default)]
    pub idempotency: IdempotencyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamConfig {
    #[serde(default = "default_stream_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_max_active_streams")]
    pub max_active_streams: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default = "default_device_rps")]
    pub device_requests_per_second: u64,
    #[serde(default = "default_controller_rps")]
    pub controller_requests_per_second: u64,
    #[serde(default = "default_global_rps")]
    pub global_requests_per_second: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdempotencyConfig {
    #[serde(default = "default_cache_capacity")]
    pub cache_capacity: usize,
    #[serde(default = "default_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: default_stream_idle_timeout(),
            max_active_streams: default_max_active_streams(),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            device_requests_per_second: default_device_rps(),
            controller_requests_per_second: default_controller_rps(),
            global_requests_per_second: default_global_rps(),
        }
    }
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            cache_capacity: default_cache_capacity(),
            cache_ttl_seconds: default_cache_ttl_seconds(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub health: HealthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default = "default_health_address")]
    pub address: String,
    #[serde(default = "default_health_path")]
    pub path: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            address: default_health_address(),
            path: default_health_path(),
        }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).map_err(|source| AppError::Io {
            path: path_ref.display().to_string(),
            source,
        })?;

        let config = config::Config::builder()
            .add_source(config::File::from_str(&content, config::FileFormat::Yaml))
            .add_source(config::Environment::default().separator("__"))
            .build()?;

        let mut app_config: Self = config.try_deserialize()?;
        app_config.apply_legacy_env_overrides();

        Ok(app_config)
    }

    fn apply_legacy_env_overrides(&mut self) {
        if let Ok(relay_id) = std::env::var("RELAY_ID") {
            self.relay.id = relay_id;
        }

        if let Ok(log_level) = std::env::var("RUST_LOG") {
            self.observability.logging.level = log_level;
        }
    }
}

fn default_quic_address() -> String {
    "0.0.0.0:50052".to_string()
}

fn default_max_device_connections() -> u32 {
    10_000
}

fn default_heartbeat_interval_seconds() -> u64 {
    30
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

fn default_health_enabled() -> bool {
    true
}

fn default_health_address() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_health_path() -> String {
    "/health".to_string()
}

fn default_stream_idle_timeout() -> u64 {
    300
}

fn default_max_active_streams() -> u32 {
    1000
}

fn default_device_rps() -> u64 {
    100
}

fn default_controller_rps() -> u64 {
    1000
}

fn default_global_rps() -> u64 {
    100_000
}

fn default_cache_capacity() -> usize {
    10_000
}

fn default_cache_ttl_seconds() -> u64 {
    3600
}
