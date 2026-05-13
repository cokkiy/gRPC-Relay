use prometheus::{
    Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    Opts, Registry, TextEncoder,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RelayMetrics {
    registry: Arc<Registry>,
    pub auth_success_total: IntCounter,
    pub auth_failure_total: IntCounter,
    pub authorization_denied_total: IntCounter,
    pub rate_limit_hits_total: IntCounter,
    pub revoked_tokens_total: IntCounter,
    pub active_device_connections: IntGauge,
    pub active_controller_connections: IntGauge,
    pub active_streams: IntGauge,
    pub cpu_usage_percent: Gauge,
    pub memory_usage_percent: Gauge,
    pub memory_used_bytes: Gauge,
    pub mqtt_connected: IntGauge,
    pub mqtt_reconnect_total: IntGauge,
    pub mqtt_dropped_total: IntGauge,
    pub mqtt_queue_pending: IntGauge,
    pub health_status: IntGauge,
    pub component_health: GaugeVec,
    pub request_latency_seconds: HistogramVec,
    pub requests_total: IntCounterVec,
    pub bytes_transferred_total: IntCounterVec,
}

impl RelayMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Arc::new(Registry::new());

        let auth_success_total = IntCounter::with_opts(Opts::new(
            "relay_auth_success_total",
            "Total successful authentication events",
        ))?;
        let auth_failure_total = IntCounter::with_opts(Opts::new(
            "relay_auth_failures_total",
            "Total failed authentication events",
        ))?;
        let authorization_denied_total = IntCounter::with_opts(Opts::new(
            "relay_authorization_denied_total",
            "Total authorization denied events",
        ))?;
        let rate_limit_hits_total = IntCounter::with_opts(Opts::new(
            "relay_rate_limit_hits_total",
            "Total rate limit events",
        ))?;
        let revoked_tokens_total = IntCounter::with_opts(Opts::new(
            "relay_revoked_tokens_total",
            "Total revoked tokens",
        ))?;
        let active_device_connections = IntGauge::with_opts(Opts::new(
            "relay_active_device_connections",
            "Current active device connections",
        ))?;
        let active_controller_connections = IntGauge::with_opts(Opts::new(
            "relay_active_controller_connections",
            "Current active controller connections",
        ))?;
        let active_streams = IntGauge::with_opts(Opts::new(
            "relay_active_streams",
            "Current active controller-device streams",
        ))?;
        let cpu_usage_percent = Gauge::with_opts(Opts::new(
            "relay_cpu_usage_percent",
            "Current CPU usage percent",
        ))?;
        let memory_usage_percent = Gauge::with_opts(Opts::new(
            "relay_memory_usage_percent",
            "Current memory usage percent",
        ))?;
        let memory_used_bytes = Gauge::with_opts(Opts::new(
            "relay_memory_used_bytes",
            "Current memory usage in bytes",
        ))?;
        let mqtt_connected = IntGauge::with_opts(Opts::new(
            "relay_mqtt_connected",
            "Current MQTT connected state",
        ))?;
        let mqtt_reconnect_total = IntGauge::with_opts(Opts::new(
            "relay_mqtt_reconnect_total",
            "Observed MQTT reconnect count",
        ))?;
        let mqtt_dropped_total = IntGauge::with_opts(Opts::new(
            "relay_mqtt_dropped_total",
            "Observed MQTT dropped publish count",
        ))?;
        let mqtt_queue_pending = IntGauge::with_opts(Opts::new(
            "relay_mqtt_queue_pending",
            "Current MQTT queue depth",
        ))?;
        let health_status = IntGauge::with_opts(Opts::new(
            "relay_health_status",
            "Overall health status (0=unhealthy, 1=degraded, 2=healthy)",
        ))?;
        let component_health = GaugeVec::new(
            Opts::new(
                "relay_component_health",
                "Health status by component (0=unhealthy, 1=degraded, 2=healthy)",
            ),
            &["component"],
        )?;
        let request_latency_seconds = HistogramVec::new(
            HistogramOpts::new(
                "relay_request_latency_seconds",
                "Relay request latency by method and status",
            ),
            &["method_name", "status"],
        )?;
        let requests_total = IntCounterVec::new(
            Opts::new("relay_requests_total", "Total relay requests by method and status"),
            &["method_name", "status"],
        )?;
        let bytes_transferred_total = IntCounterVec::new(
            Opts::new(
                "relay_bytes_transferred_total",
                "Total bytes transferred by direction",
            ),
            &["direction"],
        )?;
        registry.register(Box::new(auth_success_total.clone()))?;
        registry.register(Box::new(auth_failure_total.clone()))?;
        registry.register(Box::new(authorization_denied_total.clone()))?;
        registry.register(Box::new(rate_limit_hits_total.clone()))?;
        registry.register(Box::new(revoked_tokens_total.clone()))?;
        registry.register(Box::new(active_device_connections.clone()))?;
        registry.register(Box::new(active_controller_connections.clone()))?;
        registry.register(Box::new(active_streams.clone()))?;
        registry.register(Box::new(cpu_usage_percent.clone()))?;
        registry.register(Box::new(memory_usage_percent.clone()))?;
        registry.register(Box::new(memory_used_bytes.clone()))?;
        registry.register(Box::new(mqtt_connected.clone()))?;
        registry.register(Box::new(mqtt_reconnect_total.clone()))?;
        registry.register(Box::new(mqtt_dropped_total.clone()))?;
        registry.register(Box::new(mqtt_queue_pending.clone()))?;
        registry.register(Box::new(health_status.clone()))?;
        registry.register(Box::new(component_health.clone()))?;
        registry.register(Box::new(request_latency_seconds.clone()))?;
        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(bytes_transferred_total.clone()))?;

        Ok(Self {
            registry,
            auth_success_total,
            auth_failure_total,
            authorization_denied_total,
            rate_limit_hits_total,
            revoked_tokens_total,
            active_device_connections,
            active_controller_connections,
            active_streams,
            cpu_usage_percent,
            memory_usage_percent,
            memory_used_bytes,
            mqtt_connected,
            mqtt_reconnect_total,
            mqtt_dropped_total,
            mqtt_queue_pending,
            health_status,
            component_health,
            request_latency_seconds,
            requests_total,
            bytes_transferred_total,
        })
    }

    pub fn encode(&self) -> Result<String, prometheus::Error> {
        let encoder = TextEncoder::new();
        let metrics = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metrics, &mut buffer)?;
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }
}
