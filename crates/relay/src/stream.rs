use crate::config::StreamConfig;
use dashmap::{mapref::entry::Entry, DashMap};
use relay_proto::relay::v1::DeviceResponse;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tonic::Status;

/// Channel for sending responses back to a controller stream.
pub type ControllerTx = mpsc::Sender<Result<DeviceResponse, Status>>;

#[derive(Debug, Clone)]
pub struct StreamMapping {
    pub stream_id: String,
    pub device_id: String,
    pub controller_id: String,
    pub method_name: String,
    pub created_at: Instant,
    pub last_activity: Instant,
    pub active_requests: usize,
    pub controller_tx: ControllerTx,
}

#[derive(Debug, Clone)]
pub struct StreamRouterError {
    pub kind: StreamRouterErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamRouterErrorKind {
    MaxStreamsExceeded,
    DeviceOffline,
    Internal,
}

impl std::fmt::Display for StreamRouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::fmt::Display for StreamRouterErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamRouterErrorKind::MaxStreamsExceeded => write!(f, "max streams exceeded"),
            StreamRouterErrorKind::DeviceOffline => write!(f, "device offline"),
            StreamRouterErrorKind::Internal => write!(f, "internal error"),
        }
    }
}

/// Manages Controller ↔ Device stream mappings.
///
/// Maintains three DashMaps for O(1) lookups in all directions:
/// - `mappings`: stream_id → StreamMapping
/// - `device_to_streams`: device_id → set of stream_ids
/// - `controller_to_streams`: controller_id → set of stream_ids
#[derive(Debug, Clone)]
pub struct StreamRouter {
    counter: Arc<AtomicU64>,
    mappings: Arc<DashMap<String, StreamMapping>>,
    device_to_streams: Arc<DashMap<String, HashSet<String>>>,
    controller_to_streams: Arc<DashMap<String, HashSet<String>>>,
    max_active_streams: u32,
    max_concurrent_streams_per_controller: u32,
    idle_timeout: Duration,
}

impl StreamRouter {
    pub fn new(config: &StreamConfig) -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(1)),
            mappings: Arc::new(DashMap::new()),
            device_to_streams: Arc::new(DashMap::new()),
            controller_to_streams: Arc::new(DashMap::new()),
            max_active_streams: config.max_active_streams,
            max_concurrent_streams_per_controller: config.max_concurrent_streams_per_controller,
            idle_timeout: Duration::from_secs(config.idle_timeout_seconds),
        }
    }

    pub fn cleanup_interval(&self) -> Duration {
        self.idle_timeout.max(Duration::from_secs(1))
    }

    fn generate_stream_id(&self) -> String {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("strm-{id}")
    }

    /// Create a mapping for a new controller stream targeting a device.
    /// Returns the stream_id on success, or an error if limits are exceeded.
    pub fn create_mapping(
        &self,
        device_id: String,
        controller_id: String,
        method_name: String,
        controller_tx: ControllerTx,
    ) -> Result<String, StreamRouterError> {
        let stream_id = self.generate_stream_id();
        let now = Instant::now();

        let mapping = StreamMapping {
            stream_id: stream_id.clone(),
            device_id: device_id.clone(),
            controller_id: controller_id.clone(),
            method_name,
            created_at: now,
            last_activity: now,
            active_requests: 0,
            controller_tx,
        };
        let stream_count = match self.device_to_streams.entry(device_id) {
            Entry::Occupied(mut o) => {
                let current = o.get().len();
                if current >= self.max_active_streams as usize {
                    return Err(StreamRouterError {
                        kind: StreamRouterErrorKind::MaxStreamsExceeded,
                        message: format!(
                            "device {} has {current} active streams (max: {})",
                            mapping.device_id, self.max_active_streams
                        ),
                    });
                }
                o.get_mut().insert(stream_id.clone());
                current + 1
            }
            Entry::Vacant(v) => {
                let mut set = HashSet::new();
                set.insert(stream_id.clone());
                v.insert(set);
                1
            }
        };

        // Track by controller — enforce per-controller concurrent stream cap
        match self.controller_to_streams.entry(controller_id) {
            Entry::Occupied(mut o) => {
                let current = o.get().len();
                if current >= self.max_concurrent_streams_per_controller as usize {
                    // Roll back the device tracking we just did
                    if let Entry::Occupied(mut dev_entry) =
                        self.device_to_streams.entry(mapping.device_id.clone())
                    {
                        dev_entry.get_mut().remove(&stream_id);
                        if dev_entry.get().is_empty() {
                            dev_entry.remove();
                        }
                    }
                    return Err(StreamRouterError {
                        kind: StreamRouterErrorKind::MaxStreamsExceeded,
                        message: format!(
                            "controller {} has {current} active streams (max: {})",
                            mapping.controller_id, self.max_concurrent_streams_per_controller
                        ),
                    });
                }
                o.get_mut().insert(stream_id.clone());
            }
            Entry::Vacant(v) => {
                let mut set = HashSet::new();
                set.insert(stream_id.clone());
                v.insert(set);
            }
        }

        self.mappings.insert(stream_id.clone(), mapping);

        debug_assert!(stream_count <= self.max_active_streams as usize);

        Ok(stream_id)
    }

    /// Remove a specific stream mapping by stream_id.
    pub fn remove_mapping(&self, stream_id: &str) -> Option<StreamMapping> {
        let mapping = self.mappings.remove(stream_id).map(|(_, v)| v)?;

        // Clean up device tracking
        if let Entry::Occupied(mut o) = self.device_to_streams.entry(mapping.device_id.clone()) {
            o.get_mut().remove(stream_id);
            if o.get().is_empty() {
                o.remove();
            }
        }

        // Clean up controller tracking
        if let Entry::Occupied(mut o) = self
            .controller_to_streams
            .entry(mapping.controller_id.clone())
        {
            o.get_mut().remove(stream_id);
            if o.get().is_empty() {
                o.remove();
            }
        }

        Some(mapping)
    }

    /// Remove all stream mappings for a device. Used when device disconnects.
    /// Returns the removed mappings so the caller can notify controllers.
    pub fn remove_all_for_device(&self, device_id: &str) -> Vec<StreamMapping> {
        let stream_ids = match self.device_to_streams.entry(device_id.to_string()) {
            Entry::Occupied(o) => o.remove(),
            Entry::Vacant(_) => return Vec::new(),
        };

        let mut removed = Vec::with_capacity(stream_ids.len());
        for sid in &stream_ids {
            if let Some((_, mapping)) = self.mappings.remove(sid) {
                // Clean up controller tracking
                if let Entry::Occupied(mut o) = self
                    .controller_to_streams
                    .entry(mapping.controller_id.clone())
                {
                    o.get_mut().remove(sid);
                    if o.get().is_empty() {
                        o.remove();
                    }
                }
                removed.push(mapping);
            }
        }
        removed
    }

    /// Get all active stream mappings for a device.
    pub fn get_mappings_for_device(&self, device_id: &str) -> Vec<StreamMapping> {
        let stream_ids = match self.device_to_streams.get(device_id) {
            Some(ids) => ids.clone(),
            None => return Vec::new(),
        };

        stream_ids
            .iter()
            .filter_map(|sid| self.mappings.get(sid).map(|e| e.clone()))
            .collect()
    }

    /// Number of active streams for a given device.
    pub fn device_stream_count(&self, device_id: &str) -> usize {
        self.device_to_streams
            .get(device_id)
            .map(|e| e.len())
            .unwrap_or(0)
    }

    /// Total active streams across all devices.
    pub fn total_active_streams(&self) -> usize {
        self.mappings.len()
    }

    /// Check if a device has any active controller streams.
    pub fn has_active_streams(&self, device_id: &str) -> bool {
        self.device_stream_count(device_id) > 0
    }

    /// Update the last_activity timestamp for a stream.
    pub fn touch_stream(&self, stream_id: &str) {
        if let Some(mut m) = self.mappings.get_mut(stream_id) {
            m.last_activity = Instant::now();
        }
    }

    pub fn begin_request(&self, stream_id: &str) {
        if let Some(mut m) = self.mappings.get_mut(stream_id) {
            m.last_activity = Instant::now();
            m.active_requests += 1;
        }
    }

    pub fn finish_request(&self, stream_id: &str) {
        if let Some(mut m) = self.mappings.get_mut(stream_id) {
            m.last_activity = Instant::now();
            m.active_requests = m.active_requests.saturating_sub(1);
        }
    }

    /// Remove streams that have been idle beyond the configured timeout.
    /// Returns the removed mappings so callers can notify controllers.
    pub fn cleanup_stale(&self) -> Vec<StreamMapping> {
        let now = Instant::now();
        let mut stale = Vec::new();

        for entry in self.mappings.iter() {
            if entry.active_requests == 0
                && now.saturating_duration_since(entry.last_activity) >= self.idle_timeout
            {
                stale.push(entry.stream_id.clone());
            }
        }

        stale
            .into_iter()
            .filter_map(|sid| self.remove_mapping(&sid))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn test_config() -> StreamConfig {
        StreamConfig {
            idle_timeout_seconds: 300,
            max_active_streams: 10,
            max_concurrent_streams_per_controller: 100,
        }
    }

    fn dummy_tx() -> ControllerTx {
        let (tx, _) = mpsc::channel(64);
        tx
    }

    #[test]
    fn create_and_remove_mapping() {
        let router = StreamRouter::new(&test_config());
        let sid = router
            .create_mapping(
                "dev-1".into(),
                "ctrl-1".into(),
                "DoSomething".into(),
                dummy_tx(),
            )
            .unwrap();
        assert_eq!(router.device_stream_count("dev-1"), 1);
        assert_eq!(router.total_active_streams(), 1);

        let removed = router.remove_mapping(&sid);
        assert!(removed.is_some());
        assert_eq!(router.device_stream_count("dev-1"), 0);
        assert_eq!(router.total_active_streams(), 0);
    }

    #[test]
    fn max_streams_enforced() {
        let config = StreamConfig {
            idle_timeout_seconds: 300,
            max_active_streams: 2,
            max_concurrent_streams_per_controller: 100,
        };
        let router = StreamRouter::new(&config);

        router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();
        router
            .create_mapping("dev-1".into(), "ctrl-2".into(), "m2".into(), dummy_tx())
            .unwrap();

        let err = router
            .create_mapping("dev-1".into(), "ctrl-3".into(), "m3".into(), dummy_tx())
            .unwrap_err();
        assert_eq!(err.kind, StreamRouterErrorKind::MaxStreamsExceeded);
    }

    #[test]
    fn remove_all_for_device() {
        let router = StreamRouter::new(&test_config());
        router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();
        router
            .create_mapping("dev-1".into(), "ctrl-2".into(), "m2".into(), dummy_tx())
            .unwrap();

        let removed = router.remove_all_for_device("dev-1");
        assert_eq!(removed.len(), 2);
        assert_eq!(router.device_stream_count("dev-1"), 0);
        assert_eq!(router.total_active_streams(), 0);
    }

    #[test]
    fn cleanup_stale_removes_idle() {
        // Set very short timeout so idle streams get cleaned
        let config = StreamConfig {
            idle_timeout_seconds: 0, // immediate expiration
            max_active_streams: 10,
            max_concurrent_streams_per_controller: 100,
        };
        let router = StreamRouter::new(&config);

        router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();
        // Small delay to ensure the idle timeout triggers
        std::thread::sleep(Duration::from_millis(10));

        let stale = router.cleanup_stale();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].device_id, "dev-1");
        assert_eq!(router.total_active_streams(), 0);
    }

    #[test]
    fn touch_stream_updates_activity() {
        let config = StreamConfig {
            idle_timeout_seconds: 3600,
            max_active_streams: 10,
            max_concurrent_streams_per_controller: 100,
        };
        let router = StreamRouter::new(&config);
        let sid = router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();

        // Touch should prevent cleanup
        router.touch_stream(&sid);
        let stale = router.cleanup_stale();
        assert!(stale.is_empty());
    }

    #[test]
    fn cleanup_stale_skips_active_request() {
        let config = StreamConfig {
            idle_timeout_seconds: 0,
            max_active_streams: 10,
            max_concurrent_streams_per_controller: 100,
        };
        let router = StreamRouter::new(&config);
        let sid = router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();

        router.begin_request(&sid);
        std::thread::sleep(Duration::from_millis(10));

        assert!(router.cleanup_stale().is_empty());

        router.finish_request(&sid);
        assert_eq!(router.cleanup_stale().len(), 1);
    }

    #[test]
    fn max_concurrent_streams_per_controller_enforced() {
        let config = StreamConfig {
            idle_timeout_seconds: 300,
            max_active_streams: 100,
            max_concurrent_streams_per_controller: 2,
        };
        let router = StreamRouter::new(&config);

        router
            .create_mapping("dev-1".into(), "ctrl-1".into(), "m1".into(), dummy_tx())
            .unwrap();
        router
            .create_mapping("dev-2".into(), "ctrl-1".into(), "m2".into(), dummy_tx())
            .unwrap();

        let err = router
            .create_mapping("dev-3".into(), "ctrl-1".into(), "m3".into(), dummy_tx())
            .unwrap_err();
        assert_eq!(err.kind, StreamRouterErrorKind::MaxStreamsExceeded);

        // Different controller should still work
        let sid = router
            .create_mapping("dev-3".into(), "ctrl-2".into(), "m3".into(), dummy_tx())
            .unwrap();
        assert!(router.remove_mapping(&sid).is_some());
    }
}
