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
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamConfig {
    #[serde(default = "default_stream_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_max_active_streams")]
    pub max_active_streams: u32,
    #[serde(default = "default_max_concurrent_streams_per_controller")]
    pub max_concurrent_streams_per_controller: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    // Request rate limits
    #[serde(default = "default_device_rps")]
    pub device_requests_per_second: u64,
    #[serde(default = "default_controller_rpm")]
    pub controller_requests_per_minute: u64,
    #[serde(default = "default_global_rps")]
    pub global_requests_per_second: u64,

    // Connection rate limits
    #[serde(default = "default_device_conn_per_minute")]
    pub device_connection_per_minute: u32,
    #[serde(default = "default_global_conn_per_second")]
    pub global_connections_per_second: u32,

    // Bandwidth limits (bytes per second)
    #[serde(default = "default_device_bw")]
    pub device_bandwidth_bytes_per_sec: u64,
    #[serde(default = "default_controller_bw")]
    pub controller_bandwidth_bytes_per_sec: u64,
    #[serde(default = "default_global_bw")]
    pub global_bandwidth_bytes_per_sec: u64,

    // Resource thresholds
    #[serde(default = "default_cpu_threshold")]
    pub cpu_threshold_percent: f64,
    #[serde(default = "default_memory_threshold_mb")]
    pub memory_threshold_mb: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdempotencyConfig {
    #[serde(default = "default_cache_capacity")]
    pub cache_capacity: usize,
    #[serde(default = "default_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// Whether Relay should enforce authentication/authorization.
    ///
    /// For MVP security requirements, set to true in `config/relay.yaml`.
    #[serde(default = "default_auth_enabled")]
    pub enabled: bool,

    /// Map token -> controller auth entry.
    ///
    /// MVP: static token verification (JWT/mTLS can be added in P1/P2).
    #[serde(default)]
    pub controller_tokens: std::collections::HashMap<String, ControllerAuthEntry>,

    /// Map token -> device auth entry.
    #[serde(default)]
    pub device_tokens: std::collections::HashMap<String, DeviceAuthEntry>,

    /// Allowed method list. If empty, all methods are allowed (MVP default).
    #[serde(default)]
    pub method_whitelist: Vec<String>,

    #[serde(default)]
    pub jwt: JwtConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: default_auth_enabled(),
            controller_tokens: Default::default(),
            device_tokens: Default::default(),
            method_whitelist: Vec::new(),
            jwt: JwtConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub hs256_secret: String,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default = "default_jwt_clock_skew_seconds")]
    pub clock_skew_seconds: u64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hs256_secret: String::new(),
            issuer: None,
            audience: None,
            clock_skew_seconds: default_jwt_clock_skew_seconds(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControllerAuthEntry {
    pub controller_id: String,
    /// admin / operator / viewer
    pub role: String,
    #[serde(default)]
    pub allowed_project_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthEntry {
    pub device_id: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MqttConfig {
    /// Whether Relay maintains an MQTT connection and publishes discovery/telemetry.
    #[serde(default = "default_mqtt_enabled")]
    pub enabled: bool,

    /// MQTT broker address (e.g. localhost:1883).
    #[serde(default = "default_mqtt_broker_address")]
    pub broker_address: String,

    /// MQTT client id. If empty, Relay will derive one from `relay.id`.
    #[serde(default)]
    pub client_id: Option<String>,

    /// Username/password for broker authentication (optional).
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,

    /// How often Relay publishes `telemetry/relay/{relay_id}`.
    #[serde(default = "default_mqtt_telemetry_interval_seconds")]
    pub telemetry_interval_seconds: u64,

    /// Initial reconnect delay (seconds) after MQTT disconnect/failure.
    #[serde(default = "default_mqtt_reconnect_initial_seconds")]
    pub reconnect_initial_seconds: u64,

    /// Max reconnect delay (seconds) after repeated failures.
    #[serde(default = "default_mqtt_reconnect_max_seconds")]
    pub reconnect_max_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TlsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cert_path: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
    #[serde(default)]
    pub client_ca_path: Option<String>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: default_stream_idle_timeout(),
            max_active_streams: default_max_active_streams(),
            max_concurrent_streams_per_controller: default_max_concurrent_streams_per_controller(),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            device_requests_per_second: default_device_rps(),
            controller_requests_per_minute: default_controller_rpm(),
            global_requests_per_second: default_global_rps(),
            device_connection_per_minute: default_device_conn_per_minute(),
            global_connections_per_second: default_global_conn_per_second(),
            device_bandwidth_bytes_per_sec: default_device_bw(),
            controller_bandwidth_bytes_per_sec: default_controller_bw(),
            global_bandwidth_bytes_per_sec: default_global_bw(),
            cpu_threshold_percent: default_cpu_threshold(),
            memory_threshold_mb: default_memory_threshold_mb(),
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
    #[serde(default)]
    pub audit: AuditConfig,
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

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: default_mqtt_enabled(),
            broker_address: default_mqtt_broker_address(),
            client_id: None,
            username: None,
            password: None,
            telemetry_interval_seconds: default_mqtt_telemetry_interval_seconds(),
            reconnect_initial_seconds: default_mqtt_reconnect_initial_seconds(),
            reconnect_max_seconds: default_mqtt_reconnect_max_seconds(),
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
    10
}

fn default_max_concurrent_streams_per_controller() -> u32 {
    100
}

fn default_device_rps() -> u64 {
    1000
}

fn default_controller_rpm() -> u64 {
    1000
}

fn default_global_rps() -> u64 {
    100_000
}

fn default_device_conn_per_minute() -> u32 {
    10
}

fn default_global_conn_per_second() -> u32 {
    100
}

fn default_device_bw() -> u64 {
    10 * 1024 * 1024 // 10 MB/s
}

fn default_controller_bw() -> u64 {
    100 * 1024 * 1024 // 100 MB/s
}

fn default_global_bw() -> u64 {
    100 * 1024 * 1024 // 100 MB/s (800 Mbps)
}

fn default_cpu_threshold() -> f64 {
    80.0
}

fn default_memory_threshold_mb() -> u64 {
    12 * 1024 // 12 GB
}

fn default_cache_capacity() -> usize {
    10_000
}

fn default_cache_ttl_seconds() -> u64 {
    3600
}

fn default_auth_enabled() -> bool {
    false
}

fn default_jwt_clock_skew_seconds() -> u64 {
    30
}

fn default_mqtt_enabled() -> bool {
    false
}

fn default_mqtt_broker_address() -> String {
    "localhost:1883".to_string()
}

fn default_mqtt_telemetry_interval_seconds() -> u64 {
    30
}

fn default_mqtt_reconnect_initial_seconds() -> u64 {
    1
}

fn default_mqtt_reconnect_max_seconds() -> u64 {
    30
}

// ── Audit config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct AuditConfig {
    #[serde(default = "default_audit_enabled")]
    pub enabled: bool,
    #[serde(default = "default_audit_output")]
    pub output: String,
    #[serde(default = "default_audit_file_path")]
    pub file_path: String,
    #[serde(default = "default_audit_max_size_mb")]
    pub max_size_mb: u64,
    #[serde(default = "default_audit_max_backups")]
    pub max_backups: usize,
    #[serde(default = "default_audit_retention_days")]
    pub retention_days: u32,
    #[serde(default)]
    pub events: Vec<String>,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_audit_enabled(),
            output: default_audit_output(),
            file_path: default_audit_file_path(),
            max_size_mb: default_audit_max_size_mb(),
            max_backups: default_audit_max_backups(),
            retention_days: default_audit_retention_days(),
            events: Vec::new(),
        }
    }
}

fn default_audit_enabled() -> bool {
    true
}

fn default_audit_output() -> String {
    "stdout".to_string()
}

fn default_audit_file_path() -> String {
    "/var/log/relay/audit.log".to_string()
}

fn default_audit_max_size_mb() -> u64 {
    100
}

fn default_audit_max_backups() -> usize {
    10
}

fn default_audit_retention_days() -> u32 {
    30
}
