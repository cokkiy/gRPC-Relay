use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

use relay_proto::relay::v1::{DeviceInfo, DeviceResponse, ErrorCode, RelayMessage};
use tonic::Status;

#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub device_id: String,
    pub connection_id: String,
    pub metadata: HashMap<String, String>,
    pub outbound_tx: mpsc::Sender<std::result::Result<RelayMessage, Status>>,
}

#[derive(Debug)]
pub struct InFlight {
    device_id: String,
    pub waiters: tokio::sync::Mutex<Vec<oneshot::Sender<DeviceResponse>>>,
}

impl Default for InFlight {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl InFlight {
    pub fn new(device_id: String) -> Self {
        Self {
            device_id,
            waiters: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub async fn push_waiter(&self, tx: oneshot::Sender<DeviceResponse>) {
        let mut guard = self.waiters.lock().await;
        guard.push(tx);
    }

    pub async fn complete(&self, resp: DeviceResponse) {
        let waiters = {
            let mut guard = self.waiters.lock().await;
            std::mem::take(&mut *guard)
        };
        for waiter in waiters {
            let _ = waiter.send(resp.clone());
        }
    }
}

/// Relay in-memory state for MVP
#[derive(Debug)]
pub struct RelayState {
    pub sessions_by_device_id: DashMap<String, DeviceSession>,
    pub connection_to_device_id: DashMap<String, String>,
    pub inflight_by_sequence: DashMap<(String, i64), std::sync::Arc<InFlight>>,
    pub device_last_seen: DashMap<String, Instant>,
    connection_id_counter: AtomicU64,
    controller_connection_count: AtomicU64,
}

impl Default for RelayState {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayState {
    pub fn new() -> Self {
        Self {
            sessions_by_device_id: DashMap::new(),
            connection_to_device_id: DashMap::new(),
            inflight_by_sequence: DashMap::new(),
            device_last_seen: DashMap::new(),
            connection_id_counter: AtomicU64::new(1),
            controller_connection_count: AtomicU64::new(0),
        }
    }

    pub fn next_connection_id(&self) -> String {
        let id = self.connection_id_counter.fetch_add(1, Ordering::Relaxed);
        format!("conn-{id}")
    }

    pub fn list_online_devices(&self) -> Vec<DeviceInfo> {
        self.sessions_by_device_id
            .iter()
            .map(|entry| DeviceInfo {
                device_id: entry.value().device_id.clone(),
                connection_id: entry.value().connection_id.clone(),
                relay_address: "".to_string(), // MVP: fill later when we wire relay_address
                connected_at: 0,               // MVP: fill later when we add timestamps
                metadata: entry.value().metadata.clone(),
            })
            .collect()
    }

    /// Returns (receiver, is_new_forwarder)
    /// - is_new_forwarder=true means the caller should forward the request to the device.
    /// - is_new_forwarder=false means an in-flight request already exists; just wait.
    pub async fn ensure_inflight_waiter(
        &self,
        sequence_number: i64,
        device_id: &str,
    ) -> (oneshot::Receiver<DeviceResponse>, bool) {
        use dashmap::mapref::entry::Entry;

        let key = (device_id.to_string(), sequence_number);

        match self.inflight_by_sequence.entry(key) {
            Entry::Occupied(o) => {
                let inflight = o.get();
                let (tx, rx) = oneshot::channel::<DeviceResponse>();
                inflight.push_waiter(tx).await;
                (rx, false)
            }
            Entry::Vacant(v) => {
                let inflight = std::sync::Arc::new(InFlight::new(device_id.to_string()));
                let (tx, rx) = oneshot::channel::<DeviceResponse>();
                inflight.push_waiter(tx).await;
                v.insert(inflight);
                (rx, true)
            }
        }
    }

    pub fn take_inflight(
        &self,
        device_id: &str,
        sequence_number: i64,
    ) -> Option<std::sync::Arc<InFlight>> {
        self.inflight_by_sequence
            .remove(&(device_id.to_string(), sequence_number))
            .map(|(_, v)| v)
    }

    pub fn device_id_for_connection(&self, connection_id: &str) -> Option<String> {
        self.connection_to_device_id
            .get(connection_id)
            .map(|entry| entry.clone())
    }

    pub fn has_active_device_connection(&self, device_id: &str, connection_id: &str) -> bool {
        self.sessions_by_device_id
            .get(device_id)
            .map(|session| session.connection_id == connection_id)
            .unwrap_or(false)
    }

    pub fn touch_device(&self, device_id: &str) {
        self.device_last_seen
            .insert(device_id.to_string(), Instant::now());
    }

    pub fn device_last_seen_seconds(&self, device_id: &str) -> Option<u64> {
        self.device_last_seen
            .get(device_id)
            .map(|entry| entry.value().elapsed().as_secs())
    }

    pub fn remove_device_session(&self, device_id: &str) -> Option<DeviceSession> {
        let session = self
            .sessions_by_device_id
            .remove(device_id)
            .map(|(_, session)| session)?;
        self.connection_to_device_id.remove(&session.connection_id);
        self.device_last_seen.remove(device_id);
        Some(session)
    }

    pub fn take_inflight_for_device(
        &self,
        device_id: &str,
    ) -> Vec<(i64, std::sync::Arc<InFlight>)> {
        let keys: Vec<(String, i64)> = self
            .inflight_by_sequence
            .iter()
            .filter_map(|entry| {
                if entry.value().device_id() == device_id {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        keys.into_iter()
            .filter_map(|(device_id, seq)| {
                self.take_inflight(&device_id, seq)
                    .map(|inflight| (seq, inflight))
            })
            .collect()
    }

    pub fn increment_controller_connections(&self) {
        self.controller_connection_count
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_controller_connections(&self) {
        self.controller_connection_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }

    pub fn controller_connection_count(&self) -> u64 {
        self.controller_connection_count.load(Ordering::Relaxed)
    }
}

/// Shared helpers to map device message -> controller response
pub fn device_response_from_device_data(
    device_id: String,
    seq: i64,
    encrypted_payload: Vec<u8>,
    error: i32,
) -> DeviceResponse {
    DeviceResponse {
        device_id,
        sequence_number: seq,
        encrypted_payload,
        error,
    }
}

/// Convenience error response
pub fn make_error_response(device_id: &str, seq: i64, err: ErrorCode) -> DeviceResponse {
    DeviceResponse {
        device_id: device_id.to_string(),
        sequence_number: seq,
        encrypted_payload: Vec::new(),
        error: err as i32,
    }
}

/// Helper to build RelayMessage DataRequest
pub fn relay_message_data_request(
    connection_id: String,
    sequence_number: i64,
    encrypted_payload: Vec<u8>,
) -> RelayMessage {
    RelayMessage {
        payload: Some(relay_proto::relay::v1::relay_message::Payload::DataRequest(
            relay_proto::relay::v1::DataRequest {
                connection_id,
                sequence_number,
                encrypted_payload,
            },
        )),
    }
}

/// Helper to build RelayMessage RegisterResponse
pub fn relay_message_register_response(
    connection_id: String,
    session_resumed: bool,
) -> RelayMessage {
    RelayMessage {
        payload: Some(
            relay_proto::relay::v1::relay_message::Payload::RegisterResponse(
                relay_proto::relay::v1::RegisterResponse {
                    connection_id,
                    session_resumed,
                    timestamp: 0,
                },
            ),
        ),
    }
}

/// Helper to build RelayMessage HeartbeatResponse
pub fn relay_message_heartbeat_response() -> RelayMessage {
    RelayMessage {
        payload: Some(
            relay_proto::relay::v1::relay_message::Payload::HeartbeatResponse(
                relay_proto::relay::v1::HeartbeatResponse { timestamp: 0 },
            ),
        ),
    }
}
