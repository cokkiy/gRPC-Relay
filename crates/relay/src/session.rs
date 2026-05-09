use crate::state::{DeviceSession, RelayState};
use relay_proto::relay::v1::{DeviceInfo, DeviceResponse, ErrorCode};
use std::sync::Arc;

/// Thin facade over RelayState for session lookups.
#[derive(Debug, Clone)]
pub struct SessionRegistry {
    state: Arc<RelayState>,
}

impl SessionRegistry {
    pub fn new(state: Arc<RelayState>) -> Self {
        Self { state }
    }

    /// Look up a device session by device_id.
    pub fn get_device_session(&self, device_id: &str) -> Option<DeviceSession> {
        self.state
            .sessions_by_device_id
            .get(device_id)
            .map(|e| e.clone())
    }

    /// Check if a device is currently online.
    pub fn is_device_online(&self, device_id: &str) -> bool {
        self.state.sessions_by_device_id.contains_key(device_id)
    }

    /// Get the number of currently connected devices.
    pub fn online_device_count(&self) -> usize {
        self.state.sessions_by_device_id.len()
    }

    /// List all online devices as DeviceInfo.
    pub fn list_online_devices(&self, relay_address: &str) -> Vec<DeviceInfo> {
        self.state
            .sessions_by_device_id
            .iter()
            .map(|entry| DeviceInfo {
                device_id: entry.value().device_id.clone(),
                connection_id: entry.value().connection_id.clone(),
                relay_address: relay_address.to_string(),
                connected_at: 0,
                metadata: entry.value().metadata.clone(),
            })
            .collect()
    }

    /// Build a DEVICE_OFFLINE error response for a specific sequence.
    pub fn device_offline_response(device_id: &str, seq: i64) -> DeviceResponse {
        DeviceResponse {
            device_id: device_id.to_string(),
            sequence_number: seq,
            encrypted_payload: Vec::new(),
            error: ErrorCode::DeviceOffline as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    fn make_session(device_id: &str, conn_id: &str) -> DeviceSession {
        let (tx, _rx) = mpsc::channel(64);
        DeviceSession {
            device_id: device_id.to_string(),
            connection_id: conn_id.to_string(),
            metadata: HashMap::new(),
            outbound_tx: tx,
        }
    }

    #[test]
    fn is_device_online_after_registration() {
        let state = Arc::new(RelayState::new());
        let registry = SessionRegistry::new(state.clone());

        // Register a device directly via state
        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-1"));

        assert!(registry.is_device_online("dev-1"));
        assert_eq!(registry.online_device_count(), 1);
    }

    #[test]
    fn offline_device_not_found() {
        let state = Arc::new(RelayState::new());
        let registry = SessionRegistry::new(state);

        assert!(!registry.is_device_online("dev-nonexistent"));
        assert!(registry.get_device_session("dev-nonexistent").is_none());
    }

    #[test]
    fn list_online_devices_returns_all() {
        let state = Arc::new(RelayState::new());
        let registry = SessionRegistry::new(state.clone());

        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-1"));
        state
            .sessions_by_device_id
            .insert("dev-2".to_string(), make_session("dev-2", "conn-2"));

        let devices = registry.list_online_devices("relay-1:50051");
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].relay_address, "relay-1:50051");
    }

    #[test]
    fn device_offline_response_builds() {
        let resp = SessionRegistry::device_offline_response("dev-1", 42);
        assert_eq!(resp.device_id, "dev-1");
        assert_eq!(resp.sequence_number, 42);
        assert_eq!(resp.error, ErrorCode::DeviceOffline as i32);
    }
}
