use crate::config::MqttConfig;
use crate::resource_monitor::ResourceMonitor;
use crate::state::RelayState;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::json;
use std::cmp::min;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tracing::{info, warn};

const MQTT_PUBLISH_QUEUE_CAPACITY: usize = 256;

#[derive(Debug)]
pub enum MqttPublishRequest {
    DeviceOnline {
        device_id: String,
        connection_id: String,
        relay_address: String,
        metadata: std::collections::HashMap<String, String>,
    },
    DeviceOffline {
        device_id: String,
        connection_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct MqttRuntimeState {
    enabled: bool,
    connected: Arc<AtomicBool>,
    reconnect_count: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
    queue_pending: Arc<AtomicU64>,
}

impl MqttRuntimeState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            connected: Arc::new(AtomicBool::new(false)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            dropped_total: Arc::new(AtomicU64::new(0)),
            queue_pending: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn reconnect_count(&self) -> u64 {
        self.reconnect_count.load(Ordering::Relaxed)
    }

    pub fn dropped_total(&self) -> u64 {
        self.dropped_total.load(Ordering::Relaxed)
    }

    pub fn queue_pending(&self) -> u64 {
        self.queue_pending.load(Ordering::Relaxed)
    }

    fn set_connected(&self, value: bool) {
        self.connected.store(value, Ordering::Relaxed);
    }

    fn increment_reconnect_count(&self) {
        self.reconnect_count.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_dropped_total(&self) {
        self.dropped_total.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_queue_pending(&self) {
        self.queue_pending.fetch_add(1, Ordering::Relaxed);
    }

    fn decrement_queue_pending(&self) {
        self.queue_pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            })
            .ok();
    }
}

/// Lightweight handle that can be cloned and used from gRPC service code.
/// Publishing never blocks the main request path: it best-effort enqueues messages
/// into a bounded channel and drops them when the channel is full.
#[derive(Clone)]
pub struct MqttPublisher {
    tx: mpsc::Sender<MqttPublishRequest>,
    runtime: MqttRuntimeState,
}

impl MqttPublisher {
    pub fn is_connected(&self) -> bool {
        self.runtime.is_connected()
    }

    pub fn publish_device_online(
        &self,
        device_id: String,
        connection_id: String,
        relay_address: String,
        metadata: std::collections::HashMap<String, String>,
    ) {
        let req = MqttPublishRequest::DeviceOnline {
            device_id,
            connection_id,
            relay_address,
            metadata,
        };
        match self.tx.try_send(req) {
            Ok(()) => self.runtime.increment_queue_pending(),
            Err(_) => self.runtime.increment_dropped_total(),
        }
    }

    pub fn publish_device_offline(&self, device_id: String, connection_id: String, reason: String) {
        let req = MqttPublishRequest::DeviceOffline {
            device_id,
            connection_id,
            reason,
        };
        match self.tx.try_send(req) {
            Ok(()) => self.runtime.increment_queue_pending(),
            Err(_) => self.runtime.increment_dropped_total(),
        }
    }

    pub fn mqtt_dropped_total(&self) -> u64 {
        self.runtime.dropped_total()
    }
}

pub struct MqttHandles {
    pub publisher: MqttPublisher,
    pub runtime: MqttRuntimeState,
}

/// Spawn MQTT background worker.
///
/// - Publishes periodic `telemetry/relay/{relay_id}`
/// - Best-effort publishes device online/offline events
/// - Reconnects with exponential backoff on connection failure
pub fn spawn_mqtt_publisher(
    cfg: MqttConfig,
    relay_id: String,
    relay_address: String,
    relay_state: Arc<RelayState>,
    resource_monitor: ResourceMonitor,
    runtime: MqttRuntimeState,
) -> MqttHandles {
    let runtime_for_task = runtime.clone();
    // Bounded queue to avoid unbounded memory growth when MQTT is down.
    let (tx, mut rx) = mpsc::channel::<MqttPublishRequest>(MQTT_PUBLISH_QUEUE_CAPACITY);

    let publisher = MqttPublisher {
        tx,
        runtime: runtime.clone(),
    };

    tokio::spawn(async move {
        let initial = std::cmp::max(1, cfg.reconnect_initial_seconds);
        let max = std::cmp::max(initial, cfg.reconnect_max_seconds);

        let mut attempt: u32 = 0;
        let mut has_connected_once = false;
        loop {
            if rx.is_closed() {
                runtime_for_task.set_connected(false);
                return;
            }

            let session_res = run_mqtt_session(
                &cfg,
                &relay_id,
                &relay_address,
                &relay_state,
                &resource_monitor,
                &mut rx,
                &runtime_for_task,
                &mut has_connected_once,
            )
            .await;

            match session_res {
                Ok(()) => {
                    attempt = 0;
                }
                Err(err) => {
                    warn!(
                        event = "mqtt_session_error",
                        relay_id = %relay_id,
                        broker = %cfg.broker_address,
                        error = %err,
                        "mqtt session failed; will retry"
                    );

                    // exp_pow = 2^attempt (capped)
                    let exp_pow = checked_pow2(attempt.min(20));
                    // backoff = min(max, initial * exp_pow)
                    let backoff_secs = min(max, initial.saturating_mul(exp_pow));
                    attempt = attempt.saturating_add(1);

                    runtime_for_task.set_connected(false);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    });

    MqttHandles {
        publisher,
        runtime,
    }
}

async fn run_mqtt_session(
    cfg: &MqttConfig,
    relay_id: &str,
    relay_address: &str,
    relay_state: &Arc<RelayState>,
    resource_monitor: &ResourceMonitor,
    rx: &mut mpsc::Receiver<MqttPublishRequest>,
    runtime: &MqttRuntimeState,
    has_connected_once: &mut bool,
) -> Result<(), String> {
    let (host, port) = parse_host_port(&cfg.broker_address)?;

    let client_id = cfg
        .client_id
        .clone()
        .unwrap_or_else(|| format!("relay-{}", relay_id));

    let mut options = MqttOptions::new(client_id, host, port);
    options.set_keep_alive(std::time::Duration::from_secs(30));

    if let (Some(username), Some(password)) = (cfg.username.clone(), cfg.password.clone()) {
        options.set_credentials(username, password);
    }

    // rumqttc: AsyncClient::new returns (AsyncClient, EventLoop) directly.
    let (client, mut eventloop) = AsyncClient::new(options, 10);

    info!(
        event = "mqtt_connecting",
        relay_id = %relay_id,
        broker = %cfg.broker_address
    );

    let mut telemetry_interval = tokio::time::interval(std::time::Duration::from_secs(
        std::cmp::max(1, cfg.telemetry_interval_seconds),
    ));
    telemetry_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut pending_requests: VecDeque<MqttPublishRequest> = VecDeque::new();

    loop {
        if runtime.is_connected() && !pending_requests.is_empty() {
            let req = pending_requests
                .pop_front()
                .expect("pending_requests was checked non-empty before pop_front; logic error");
            if let Err(e) = publish_device_event(client.clone(), req, relay_address, relay_id).await {
                runtime.decrement_queue_pending();
                return Err(e);
            }
            runtime.decrement_queue_pending();
            continue;
        }

        tokio::select! {
            ev = eventloop.poll() => {
                match ev {
                    Ok(Event::Incoming(Packet::ConnAck(_ack))) => {
                        if *has_connected_once {
                            runtime.increment_reconnect_count();
                        } else {
                            *has_connected_once = true;
                        }
                        runtime.set_connected(true);
                        info!(
                            event = "mqtt_connected",
                            relay_id = %relay_id,
                            broker = %cfg.broker_address
                        );
                        publish_online_session_snapshot(client.clone(), relay_state, relay_address).await?;
                    }
                    Ok(Event::Incoming(_)) => {}
                    Ok(Event::Outgoing(_)) => {}
                    Err(e) => {
                        runtime.set_connected(false);
                        return Err(format!("mqtt eventloop poll error: {e}"));
                    }
                }
            }

            maybe_req = rx.recv() => {
                let Some(req) = maybe_req else {
                    while pending_requests.pop_front().is_some() {
                        runtime.increment_dropped_total();
                        runtime.decrement_queue_pending();
                    }
                    runtime.set_connected(false);
                    return Ok(());
                };

                if runtime.is_connected() && pending_requests.is_empty() {
                    if let Err(e) = publish_device_event(client.clone(), req, relay_address, relay_id).await {
                        runtime.decrement_queue_pending();
                        return Err(e);
                    }
                    runtime.decrement_queue_pending();
                } else {
                    if pending_requests.len() >= MQTT_PUBLISH_QUEUE_CAPACITY {
                        pending_requests.pop_front();
                        runtime.increment_dropped_total();
                        runtime.decrement_queue_pending();
                    }
                    pending_requests.push_back(req);
                }
            }

            _ = telemetry_interval.tick() => {
                if !runtime.is_connected() {
                    continue;
                }

                let payload = build_relay_telemetry_payload(
                    relay_id,
                    relay_address,
                    relay_state,
                    resource_monitor,
                    runtime,
                    0,
                    cfg.telemetry_interval_seconds,
                );

                let topic = format!("telemetry/relay/{relay_id}");

                if let Err(e) = client
                    .publish(topic, QoS::AtMostOnce, false, payload.to_string())
                    .await
                {
                    runtime.set_connected(false);
                    return Err(format!("mqtt telemetry publish failed: {e}"));
                }
            }
        }
    }
}

fn checked_pow2(attempt: u32) -> u64 {
    // capped: attempt <= 20
    1u64.checked_shl(attempt).unwrap_or(u64::MAX)
}

fn parse_host_port(broker_address: &str) -> Result<(String, u16), String> {
    let authority = broker_address
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(broker_address)
        .split('/')
        .next()
        .unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);

    if authority.is_empty() {
        return Err("broker_address missing host".to_string());
    }

    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| "broker_address has invalid IPv6 format".to_string())?;
        let port_str = suffix
            .strip_prefix(':')
            .ok_or_else(|| "broker_address missing port".to_string())?;
        let port: u16 = port_str
            .parse()
            .map_err(|_| "broker_address port must be a number".to_string())?;
        return Ok((host.to_string(), port));
    }

    let (host, port_str) = authority
        .rsplit_once(':')
        .ok_or_else(|| "broker_address missing port".to_string())?;
    if host.is_empty() {
        return Err("broker_address missing host".to_string());
    }
    if host.contains(':') {
        return Err("IPv6 broker_address must use [host]:port format".to_string());
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| "broker_address port must be a number".to_string())?;
    Ok((host.to_string(), port))
}

async fn publish_device_event(
    client: AsyncClient,
    req: MqttPublishRequest,
    _relay_address: &str,
    _relay_id: &str,
) -> Result<(), String> {
    match req {
        MqttPublishRequest::DeviceOnline {
            device_id,
            connection_id,
            relay_address,
            metadata,
        } => {
            let topic = "relay/device/online";
            let timestamp = now_rfc3339();

            let payload = json!({
                "device_id": device_id,
                "connection_id": connection_id,
                "relay_address": relay_address,
                "timestamp": timestamp,
                "metadata": metadata,
            });

            client
                .publish(topic, QoS::AtLeastOnce, true, payload.to_string())
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        MqttPublishRequest::DeviceOffline {
            device_id,
            connection_id,
            reason,
        } => {
            let topic = "relay/device/offline";
            let timestamp = now_rfc3339();

            let payload = json!({
                "device_id": device_id,
                "connection_id": connection_id,
                "timestamp": timestamp,
                "reason": reason,
            });

            client
                .publish(topic, QoS::AtLeastOnce, false, payload.to_string())
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

async fn publish_online_session_snapshot(
    client: AsyncClient,
    relay_state: &Arc<RelayState>,
    relay_address: &str,
) -> Result<(), String> {
    for req in online_session_snapshot(relay_state, relay_address) {
        publish_device_event(client.clone(), req, relay_address, "").await?;
    }
    Ok(())
}

fn online_session_snapshot(
    relay_state: &Arc<RelayState>,
    relay_address: &str,
) -> Vec<MqttPublishRequest> {
    relay_state
        .sessions_by_device_id
        .iter()
        .map(|entry| MqttPublishRequest::DeviceOnline {
            device_id: entry.value().device_id.clone(),
            connection_id: entry.value().connection_id.clone(),
            relay_address: relay_address.to_string(),
            metadata: entry.value().metadata.clone(),
        })
        .collect()
}

/// Build Relay telemetry payload (minimum required fields).
fn build_relay_telemetry_payload(
    relay_id: &str,
    relay_address: &str,
    relay_state: &Arc<RelayState>,
    resource_monitor: &ResourceMonitor,
    runtime: &MqttRuntimeState,
    mqtt_publish_failures_total: u64,
    telemetry_interval_seconds: u64,
) -> serde_json::Value {
    let timestamp = now_rfc3339();

    let cpu_usage_percent = resource_monitor.cpu_usage_percent();
    let memory_usage_percent = resource_monitor.memory_usage_percent();
    let used_memory_mb = resource_monitor.used_memory_mb();

    let resource_healthy = resource_monitor.is_healthy();
    let health_status = if resource_healthy {
        "healthy"
    } else {
        "unhealthy"
    };

    let active_device_connections = relay_state.sessions_by_device_id.len();

    json!({
        "relay_id": relay_id,
        "relay_address": relay_address,
        "timestamp": timestamp,
        "system_metrics": {
            "cpu_usage_percent": cpu_usage_percent,
            "memory_usage_percent": memory_usage_percent,
            "used_memory_mb": used_memory_mb
        },
        "connection_metrics": {
            "active_device_connections": active_device_connections,
            "active_controller_connections": 0
        },
        "stream_metrics": {
            "active_streams": 0
        },
        "performance_metrics": {
            "resource_healthy": resource_healthy
        },
        "error_metrics": {
            "mqtt_publish_failures_total": mqtt_publish_failures_total
        },
        "queue_metrics": {
            "mqtt_queue_pending": runtime.queue_pending()
        },
        "mqtt_metrics": {
            "telemetry_interval_seconds": telemetry_interval_seconds,
            "mqtt_connected": runtime.is_connected(),
            "mqtt_reconnect_count": runtime.reconnect_count(),
            "mqtt_dropped_total": runtime.dropped_total()
        },
        "health_status": health_status
    })
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
pub(crate) fn test_publisher() -> (MqttPublisher, mpsc::Receiver<MqttPublishRequest>) {
    let runtime = MqttRuntimeState::new(true);
    let (tx, rx) = mpsc::channel(16);
    (MqttPublisher { tx, runtime }, rx)
}

#[cfg(test)]
mod tests {
    use super::{build_relay_telemetry_payload, online_session_snapshot, parse_host_port, MqttRuntimeState};
    use crate::resource_monitor::ResourceMonitor;
    use crate::state::{DeviceSession, RelayState};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn test_resource_monitor() -> ResourceMonitor {
        ResourceMonitor::new(&crate::config::RateLimitConfig::default())
    }

    #[test]
    fn online_session_snapshot_contains_all_sessions() {
        let state = Arc::new(RelayState::new());
        let (tx, _rx) = mpsc::channel(1);

        state.sessions_by_device_id.insert(
            "dev-1".into(),
            DeviceSession {
                device_id: "dev-1".into(),
                connection_id: "conn-1".into(),
                metadata: HashMap::from([("region".into(), "test".into())]),
                outbound_tx: tx,
            },
        );

        let snapshot = online_session_snapshot(&state, "relay-test:50051");
        assert_eq!(snapshot.len(), 1);
        match &snapshot[0] {
            super::MqttPublishRequest::DeviceOnline {
                device_id,
                connection_id,
                relay_address,
                metadata,
            } => {
                assert_eq!(device_id, "dev-1");
                assert_eq!(connection_id, "conn-1");
                assert_eq!(relay_address, "relay-test:50051");
                assert_eq!(metadata.get("region").map(String::as_str), Some("test"));
            }
            other => panic!("unexpected snapshot event: {other:?}"),
        }
    }

    #[test]
    fn relay_telemetry_contains_runtime_mqtt_fields() {
        let state = Arc::new(RelayState::new());
        let runtime = MqttRuntimeState::new(true);
        runtime.set_connected(true);
        runtime.increment_reconnect_count();
        runtime.increment_queue_pending();

        let payload = build_relay_telemetry_payload(
            "relay-test",
            "127.0.0.1:50051",
            &state,
            &test_resource_monitor(),
            &runtime,
            3,
            10,
        );

        assert_eq!(payload["mqtt_metrics"]["mqtt_connected"], true);
        assert_eq!(payload["mqtt_metrics"]["mqtt_reconnect_count"], 1);
        assert_eq!(payload["mqtt_metrics"]["telemetry_interval_seconds"], 10);
        assert_eq!(payload["queue_metrics"]["mqtt_queue_pending"], 1);
    }

    #[test]
    fn parse_host_port_supports_mqtt_url_and_ipv6() {
        let with_scheme = parse_host_port("mqtt://broker.example.com:1883").unwrap();
        assert_eq!(with_scheme, ("broker.example.com".to_string(), 1883));

        let ipv6 = parse_host_port("[::1]:1883").unwrap();
        assert_eq!(ipv6, ("::1".to_string(), 1883));
    }
}
