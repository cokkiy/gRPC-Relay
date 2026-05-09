use lru_time_cache::LruCache;
use relay_proto::relay::v1::DeviceResponse;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Bounded LRU cache keyed by sequence_number.
///
/// Uses `with_expiry_duration_and_capacity` to enforce both TTL and
/// capacity limits, preventing unbounded memory growth under sustained load.
#[derive(Clone)]
pub struct IdempotencyCache {
    inner: Arc<Mutex<LruCache<i64, DeviceResponse>>>,
    capacity: usize,
    _ttl: Duration,
}

impl IdempotencyCache {
    pub fn new(capacity: usize, ttl_seconds: u64) -> Self {
        let ttl = Duration::from_secs(ttl_seconds);
        let cache = LruCache::with_expiry_duration_and_capacity(ttl, capacity);
        Self {
            inner: Arc::new(Mutex::new(cache)),
            capacity,
            _ttl: ttl,
        }
    }

    pub async fn get(&self, sequence_number: i64) -> Option<DeviceResponse> {
        let mut guard = self.inner.lock().await;
        guard.get(&sequence_number).cloned()
    }

    pub async fn insert(&self, sequence_number: i64, response: DeviceResponse) {
        let mut guard = self.inner.lock().await;
        guard.insert(sequence_number, response);
    }

    pub async fn remove(&self, sequence_number: i64) {
        let mut guard = self.inner.lock().await;
        guard.remove(&sequence_number);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_proto::relay::v1::ErrorCode;

    fn make_response(seq: i64) -> DeviceResponse {
        DeviceResponse {
            device_id: "dev-1".into(),
            sequence_number: seq,
            encrypted_payload: vec![],
            error: ErrorCode::Ok as i32,
        }
    }

    #[tokio::test]
    async fn get_missing() {
        let cache = IdempotencyCache::new(100, 3600);
        assert!(cache.get(42).await.is_none());
    }

    #[tokio::test]
    async fn insert_and_get() {
        let cache = IdempotencyCache::new(100, 3600);
        cache.insert(1, make_response(1)).await;
        let resp = cache.get(1).await;
        assert!(resp.is_some());
        assert_eq!(resp.unwrap().sequence_number, 1);
    }

    #[tokio::test]
    async fn evicts_on_capacity() {
        let cache = IdempotencyCache::new(2, 3600);
        cache.insert(1, make_response(1)).await;
        cache.insert(2, make_response(2)).await;
        // This should evict seq=1
        cache.insert(3, make_response(3)).await;

        assert!(cache.get(1).await.is_none());
        assert!(cache.get(2).await.is_some());
        assert!(cache.get(3).await.is_some());
    }

    #[test]
    fn capacity_is_bounded() {
        let cache = IdempotencyCache::new(10_000, 3600);
        assert_eq!(cache.capacity(), 10_000);
    }
}
