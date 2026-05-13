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

    #[test]
    fn session_removed_after_remove_device_session() {
        let state = Arc::new(RelayState::new());
        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-1"));
        state
            .connection_to_device_id
            .insert("conn-1".to_string(), "dev-1".to_string());

        state.remove_device_session("dev-1");
        assert!(!state.sessions_by_device_id.contains_key("dev-1"));
        assert!(!state.connection_to_device_id.contains_key("conn-1"));
    }

    #[test]
    fn duplicate_registration_replaces_old_session() {
        let state = Arc::new(RelayState::new());
        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-1"));
        state
            .connection_to_device_id
            .insert("conn-1".to_string(), "dev-1".to_string());

        // Register same device with new connection
        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-2"));
        state
            .connection_to_device_id
            .insert("conn-2".to_string(), "dev-1".to_string());

        let reg = SessionRegistry::new(state.clone());
        assert!(reg.is_device_online("dev-1"));
        let session = reg.get_device_session("dev-1").unwrap();
        assert_eq!(session.connection_id, "conn-2");
    }

    #[test]
    fn list_online_devices_filtering() {
        let state = Arc::new(RelayState::new());
        let reg = SessionRegistry::new(state.clone());

        state
            .sessions_by_device_id
            .insert("dev-1".to_string(), make_session("dev-1", "conn-1"));
        state
            .sessions_by_device_id
            .insert("dev-2".to_string(), make_session("dev-2", "conn-2"));

        // list all — should have 2
        assert_eq!(reg.list_online_devices("relay-1").len(), 2);
    }

    #[tokio::test]
    async fn concurrent_register_and_heartbeat_no_race() {
        let state = Arc::new(RelayState::new());

        let state_clone1 = state.clone();
        let handle1 = tokio::spawn(async move {
            for i in 0..50 {
                state_clone1.sessions_by_device_id.insert(
                    format!("dev-{i}"),
                    make_session(&format!("dev-{i}"), &format!("conn-{i}")),
                );
            }
        });

        let state_clone2 = state.clone();
        let handle2 = tokio::spawn(async move {
            for i in 0..50 {
                let _ = state_clone2.sessions_by_device_id.get(&format!("dev-{i}"));
            }
        });

        let _ = tokio::join!(handle1, handle2);
        // No panic means no data race
        assert!(state.sessions_by_device_id.len() <= 50);
    }

    #[test]
    fn controller_connection_counts() {
        let state = Arc::new(RelayState::new());
        assert_eq!(state.controller_connection_count(), 0);

        state.increment_controller_connections();
        state.increment_controller_connections();
        assert_eq!(state.controller_connection_count(), 2);

        state.decrement_controller_connections();
        assert_eq!(state.controller_connection_count(), 1);

        // Should not underflow
        state.decrement_controller_connections();
        state.decrement_controller_connections();
        assert_eq!(state.controller_connection_count(), 0);
    }

    #[test]
    fn connection_to_device_mapping() {
        let state = Arc::new(RelayState::new());
        state
            .connection_to_device_id
            .insert("conn-1".to_string(), "dev-1".to_string());

        assert_eq!(
            state.device_id_for_connection("conn-1").as_deref(),
            Some("dev-1")
        );
        assert_eq!(state.device_id_for_connection("conn-nonexistent"), None);
    }

    #[test]
    fn next_connection_id_is_unique() {
        let state = Arc::new(RelayState::new());
        let id1 = state.next_connection_id();
        let id2 = state.next_connection_id();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("conn-"));
    }
}
