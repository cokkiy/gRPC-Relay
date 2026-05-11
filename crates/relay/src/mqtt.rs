use crate::config::MqttConfig;
use crate::resource_monitor::ResourceMonitor;
use crate::state::RelayState;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::json;
use std::cmp::min;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tracing::{info, warn};

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

/// Lightweight handle that can be cloned and used from gRPC service code.
/// Publishing never blocks the main request path: it best-effort enqueues messages
/// into a bounded channel and drops them when the channel is full.
#[derive(Clone)]
pub struct MqttPublisher {
    tx: mpsc::Sender<MqttPublishRequest>,
    connected: Arc<AtomicBool>,
    mqtt_dropped_total: Arc<AtomicU64>,
}

impl MqttPublisher {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
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
        let _ = self.tx.try_send(req).map_err(|_| {
            self.mqtt_dropped_total.fetch_add(1, Ordering::Relaxed);
        });
    }

    pub fn publish_device_offline(&self, device_id: String, connection_id: String, reason: String) {
        let req = MqttPublishRequest::DeviceOffline {
            device_id,
            connection_id,
            reason,
        };
        let _ = self
            .tx
            .try_send(req)
            .map_err(|_| self.mqtt_dropped_total.fetch_add(1, Ordering::Relaxed));
    }

    pub fn mqtt_dropped_total(&self) -> u64 {
        self.mqtt_dropped_total.load(Ordering::Relaxed)
    }
}

pub struct MqttHandles {
    pub publisher: MqttPublisher,
    pub connected: Arc<AtomicBool>,
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
) -> MqttHandles {
    let connected = Arc::new(AtomicBool::new(false));
    let connected_for_task = connected.clone();
    let dropped_total = Arc::new(AtomicU64::new(0));

    // Bounded queue to avoid unbounded memory growth when MQTT is down.
    let (tx, mut rx) = mpsc::channel::<MqttPublishRequest>(256);

    let publisher = MqttPublisher {
        tx,
        connected: connected.clone(),
        mqtt_dropped_total: dropped_total.clone(),
    };

    tokio::spawn(async move {
        let initial = std::cmp::max(1, cfg.reconnect_initial_seconds);
        let max = std::cmp::max(initial, cfg.reconnect_max_seconds);

        let mut attempt: u32 = 0;
        loop {
            if rx.is_closed() {
                connected_for_task.store(false, Ordering::Relaxed);
                return;
            }

            let session_res = run_mqtt_session(
                &cfg,
                &relay_id,
                &relay_address,
                &relay_state,
                &resource_monitor,
                &mut rx,
                &connected_for_task,
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
                    let backoff_secs = min(max as u64, (initial as u64).saturating_mul(exp_pow));
                    attempt = attempt.saturating_add(1);

                    connected_for_task.store(false, Ordering::Relaxed);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    });

    MqttHandles {
        publisher,
        connected,
    }
}

async fn run_mqtt_session(
    cfg: &MqttConfig,
    relay_id: &str,
    relay_address: &str,
    relay_state: &Arc<RelayState>,
    resource_monitor: &ResourceMonitor,
    rx: &mut mpsc::Receiver<MqttPublishRequest>,
    connected: &Arc<AtomicBool>,
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

    connected.store(true, Ordering::Relaxed);
    info!(
        event = "mqtt_connected",
        relay_id = %relay_id,
        broker = %cfg.broker_address
    );

    let mut telemetry_interval = tokio::time::interval(std::time::Duration::from_secs(
        std::cmp::max(1, cfg.telemetry_interval_seconds),
    ));
    telemetry_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut mqtt_publish_failures_total: u64 = 0;

    loop {
        tokio::select! {
            ev = eventloop.poll() => {
                match ev {
                    Ok(Event::Incoming(Packet::ConnAck(_ack))) => {
                        connected.store(true, Ordering::Relaxed);
                    }
                    Ok(Event::Incoming(_)) => {}
                    Ok(Event::Outgoing(_)) => {}
                    Err(e) => {
                        connected.store(false, Ordering::Relaxed);
                        return Err(format!("mqtt eventloop poll error: {e}"));
                    }
                }
            }

            maybe_req = rx.recv() => {
                let Some(req) = maybe_req else {
                    connected.store(false, Ordering::Relaxed);
                    return Ok(());
                };

                if !connected.load(Ordering::Relaxed) {
                    // MQTT is down; drop requests.
                    continue;
                }

                publish_device_event(client.clone(), req, relay_address, relay_id)
                    .await
                    .map_err(|e| {
                        mqtt_publish_failures_total += 1;
                        e
                    })?;
            }

            _ = telemetry_interval.tick() => {
                if !connected.load(Ordering::Relaxed) {
                    continue;
                }

                let payload = build_relay_telemetry_payload(
                    relay_id,
                    relay_address,
                    relay_state,
                    resource_monitor,
                    mqtt_publish_failures_total,
                    cfg.telemetry_interval_seconds,
                );

                let topic = format!("telemetry/relay/{relay_id}");

                if let Err(e) = client
                    .publish(topic, QoS::AtMostOnce, false, payload.to_string())
                    .await
                {
                    mqtt_publish_failures_total += 1;
                    connected.store(false, Ordering::Relaxed);
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
    let mut parts = broker_address.split(':');
    let host = parts
        .next()
        .ok_or_else(|| "broker_address missing host".to_string())?
        .to_string();
    let port_str = parts
        .next()
        .ok_or_else(|| "broker_address missing port".to_string())?;
    if parts.next().is_some() {
        return Err("broker_address must be host:port".to_string());
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| "broker_address port must be a number".to_string())?;
    Ok((host, port))
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

/// Build Relay telemetry payload (minimum required fields).
fn build_relay_telemetry_payload(
    relay_id: &str,
    relay_address: &str,
    relay_state: &Arc<RelayState>,
    resource_monitor: &ResourceMonitor,
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
            "mqtt_queue_pending": 0
        },
        "mqtt_metrics": {
            "telemetry_interval_seconds": telemetry_interval_seconds
        },
        "health_status": health_status
    })
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
