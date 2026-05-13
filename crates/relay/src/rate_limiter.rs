use crate::config::RateLimitConfig;
use dashmap::{mapref::entry::Entry, DashMap};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ── Token-bucket request rate limiter ──────────────────────────────────────

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token-bucket rate limiter per device, per controller, and globally.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    device_rate: f64,
    controller_rate: f64,
    global_rate: f64,
    buckets: Arc<DashMap<String, Bucket>>,
    cleanup_counter: Arc<AtomicU64>,
    bucket_ttl: Duration,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            device_rate: config.device_requests_per_second as f64,
            controller_rate: (config.controller_requests_per_minute as f64) / 60.0,
            global_rate: config.global_requests_per_second as f64,
            buckets: Arc::new(DashMap::new()),
            cleanup_counter: Arc::new(AtomicU64::new(0)),
            bucket_ttl: Duration::from_secs(300),
        }
    }

    pub fn allow(&self, device_id: &str, controller_id: &str) -> bool {
        self.maybe_cleanup();

        if !self.check_key("global", self.global_rate) {
            return false;
        }
        if !self.check_key(&format!("device:{device_id}"), self.device_rate) {
            return false;
        }
        self.check_key(&format!("controller:{controller_id}"), self.controller_rate)
    }

    fn check_key(&self, key: &str, rate: f64) -> bool {
        if rate <= 0.0 {
            return false;
        }

        let now = Instant::now();

        match self.buckets.entry(key.to_string()) {
            Entry::Occupied(mut o) => {
                let b = o.get_mut();
                let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
                if elapsed > 0.0 {
                    b.tokens = rate.min(b.tokens + elapsed * rate);
                    b.last = now;
                }
                if b.tokens >= 1.0 {
                    b.tokens -= 1.0;
                    true
                } else {
                    false
                }
            }
            Entry::Vacant(v) => {
                v.insert(Bucket {
                    tokens: (rate - 1.0).max(0.0),
                    last: now,
                });
                true
            }
        }
    }

    fn maybe_cleanup(&self) {
        const CLEANUP_EVERY: usize = 256;

        let count = self.cleanup_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count % CLEANUP_EVERY as u64 != 0 {
            return;
        }

        let now = Instant::now();
        let stale: Vec<String> = self
            .buckets
            .iter()
            .filter_map(|entry| {
                if now.saturating_duration_since(entry.last) >= self.bucket_ttl {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        for key in stale {
            self.buckets.remove(&key);
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

// ── Sliding-window connection rate limiter ────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConnectionRateLimiter {
    device_limit: u32,
    device_window: Duration,
    global_limit: u32,
    global_window: Duration,
    device_windows: Arc<DashMap<String, VecDeque<Instant>>>,
    global_window_data: Arc<tokio::sync::Mutex<VecDeque<Instant>>>,
}

impl ConnectionRateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            device_limit: config.device_connection_per_minute,
            device_window: Duration::from_secs(60),
            global_limit: config.global_connections_per_second,
            global_window: Duration::from_secs(1),
            device_windows: Arc::new(DashMap::new()),
            global_window_data: Arc::new(tokio::sync::Mutex::new(VecDeque::new())),
        }
    }

    /// Check a device connection attempt. Returns true if allowed.
    pub fn allow_device(&self, device_id: &str) -> bool {
        let mut window = self
            .device_windows
            .entry(device_id.to_string())
            .or_insert_with(VecDeque::new);

        Self::prune_and_check(&mut window, self.device_limit, self.device_window)
    }

    /// Check a global connection attempt. Returns true if allowed.
    pub async fn allow_global(&self) -> bool {
        let mut window = self.global_window_data.lock().await;
        Self::prune_and_check_inner(&mut *window, self.global_limit, self.global_window)
    }

    fn prune_and_check(window: &mut VecDeque<Instant>, limit: u32, window_dur: Duration) -> bool {
        let now = Instant::now();
        while window
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= window_dur)
        {
            window.pop_front();
        }
        if window.len() >= limit as usize {
            return false;
        }
        window.push_back(now);
        true
    }

    fn prune_and_check_inner(
        window: &mut VecDeque<Instant>,
        limit: u32,
        window_dur: Duration,
    ) -> bool {
        let now = Instant::now();
        while window
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= window_dur)
        {
            window.pop_front();
        }
        if window.len() >= limit as usize {
            return false;
        }
        window.push_back(now);
        true
    }
}

// ── Bandwidth tracker ─────────────────────────────────────────────────────

/// Tracks bandwidth usage via rotating 1-second windows.
#[derive(Debug, Clone)]
pub struct BandwidthTracker {
    device_limit: u64,
    controller_limit: u64,
    global_limit: u64,
    // (bytes_in_window, window_start_ns)
    windows: Arc<DashMap<String, (u64, u64)>>,
}

impl BandwidthTracker {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            device_limit: config.device_bandwidth_bytes_per_sec,
            controller_limit: config.controller_bandwidth_bytes_per_sec,
            global_limit: config.global_bandwidth_bytes_per_sec,
            windows: Arc::new(DashMap::new()),
        }
    }

    /// Record bytes transferred for a device, controller, and globally.
    /// Returns false if any limit is exceeded.
    pub fn record_and_check(&self, device_id: &str, controller_id: &str, bytes: u64) -> bool {
        let global_ok = self.record_key("global", bytes, self.global_limit);
        let device_ok = self.record_key(&format!("device:{device_id}"), bytes, self.device_limit);
        let controller_ok = self.record_key(
            &format!("controller:{controller_id}"),
            bytes,
            self.controller_limit,
        );
        global_ok && device_ok && controller_ok
    }

    fn record_key(&self, key: &str, bytes: u64, limit: u64) -> bool {
        let now_ns = Self::now_ns();

        let mut entry = self
            .windows
            .entry(key.to_string())
            .or_insert_with(|| (0, now_ns));

        // Rotate window if >= 1 second elapsed.
        if now_ns.saturating_sub(entry.1) >= 1_000_000_000 {
            entry.1 = now_ns;
            entry.0 = 0;
        }

        let Some(new_total) = entry.0.checked_add(bytes) else {
            return false;
        };
        entry.0 = new_total;
        new_total <= limit
    }

    fn now_ns() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            device_requests_per_second: 100,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 100_000,
            device_connection_per_minute: 10,
            global_connections_per_second: 100,
            device_bandwidth_bytes_per_sec: 10 * 1024 * 1024,
            controller_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
            global_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
            cpu_threshold_percent: 80.0,
            memory_threshold_mb: 12 * 1024,
        }
    }

    // ── Token bucket tests ──────────────────────────────────────────────

    #[test]
    fn allows_first_request() {
        let rl = RateLimiter::new(&test_config());
        assert!(rl.allow("dev-1", "ctrl-1"));
    }

    #[test]
    fn denies_after_exhaustion() {
        let config = RateLimitConfig {
            device_requests_per_second: 2,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-1", "ctrl-1"));
    }

    #[test]
    fn different_devices_independent() {
        let config = RateLimitConfig {
            device_requests_per_second: 1,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-1", "ctrl-1"));
        assert!(rl.allow("dev-2", "ctrl-1"));
    }

    #[test]
    fn global_limit_applies() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 1,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-2", "ctrl-2"));
    }

    #[test]
    fn zero_rate_denies_requests() {
        let config = RateLimitConfig {
            device_requests_per_second: 0,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        assert!(!rl.allow("dev-1", "ctrl-1"));
    }

    #[test]
    fn controller_rate_is_per_minute_converted_to_per_second() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 120, // 2 per second
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        // First 2 should be allowed (different device IDs to bypass device limit)
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(rl.allow("dev-2", "ctrl-1"));
        assert!(!rl.allow("dev-3", "ctrl-1"));
    }

    // ── Connection rate limiter tests ───────────────────────────────────

    #[test]
    fn connection_limiter_allows_up_to_limit() {
        let rl = ConnectionRateLimiter::new(&test_config());
        for _ in 0..10 {
            assert!(rl.allow_device("dev-1"));
        }
        assert!(!rl.allow_device("dev-1"));
    }

    #[test]
    fn connection_limiter_independent_per_device() {
        let rl = ConnectionRateLimiter::new(&test_config());
        for _ in 0..10 {
            assert!(rl.allow_device("dev-1"));
        }
        assert!(!rl.allow_device("dev-1"));
        // dev-2 still allowed
        assert!(rl.allow_device("dev-2"));
    }

    #[tokio::test]
    async fn connection_limiter_global() {
        let config = RateLimitConfig {
            global_connections_per_second: 3,
            ..test_config()
        };
        let rl = ConnectionRateLimiter::new(&config);
        assert!(rl.allow_global().await);
        assert!(rl.allow_global().await);
        assert!(rl.allow_global().await);
        assert!(!rl.allow_global().await);
    }

    // ── Bandwidth tracker tests ─────────────────────────────────────────

    #[test]
    fn bandwidth_tracker_allows_within_limit() {
        let bt = BandwidthTracker::new(&test_config());
        assert!(bt.record_and_check("dev-1", "ctrl-1", 1024));
    }

    #[test]
    fn bandwidth_tracker_denies_when_exceeding_device_limit() {
        let config = RateLimitConfig {
            device_bandwidth_bytes_per_sec: 100,
            controller_bandwidth_bytes_per_sec: 100_000_000,
            global_bandwidth_bytes_per_sec: 100_000_000,
            ..test_config()
        };
        let bt = BandwidthTracker::new(&config);
        // 50 bytes — within limit
        assert!(bt.record_and_check("dev-1", "ctrl-1", 50));
        // Another 60 bytes pushes to 110 — exceeds 100
        assert!(!bt.record_and_check("dev-1", "ctrl-1", 60));
    }

    #[test]
    fn bandwidth_tracker_denies_when_exceeding_controller_limit() {
        let config = RateLimitConfig {
            device_bandwidth_bytes_per_sec: 100_000_000,
            controller_bandwidth_bytes_per_sec: 100,
            global_bandwidth_bytes_per_sec: 100_000_000,
            ..test_config()
        };
        let bt = BandwidthTracker::new(&config);
        assert!(bt.record_and_check("dev-1", "ctrl-1", 60));
        assert!(!bt.record_and_check("dev-1", "ctrl-1", 50));
    }

    #[test]
    fn bandwidth_tracker_denies_when_exceeding_global_limit() {
        let config = RateLimitConfig {
            device_bandwidth_bytes_per_sec: 100_000_000,
            controller_bandwidth_bytes_per_sec: 100_000_000,
            global_bandwidth_bytes_per_sec: 100,
            ..test_config()
        };
        let bt = BandwidthTracker::new(&config);
        assert!(bt.record_and_check("dev-1", "ctrl-1", 50));
        assert!(!bt.record_and_check("dev-2", "ctrl-2", 60));
    }

    #[test]
    fn bandwidth_tracker_rotates_window_before_counting_new_bytes() {
        let config = RateLimitConfig {
            device_bandwidth_bytes_per_sec: 100,
            controller_bandwidth_bytes_per_sec: 100_000_000,
            global_bandwidth_bytes_per_sec: 100_000_000,
            ..test_config()
        };
        let bt = BandwidthTracker::new(&config);

        let stale_ns = BandwidthTracker::now_ns().saturating_sub(2_000_000_000);
        bt.windows
            .insert("device:dev-1".to_string(), (90, stale_ns));

        assert!(bt.record_key("device:dev-1", 20, 100));
        assert_eq!(bt.windows.get("device:dev-1").unwrap().0, 20);
    }

    #[test]
    fn controller_rate_limit_blocks_when_exceeded() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 1,
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        // First request allowed
        assert!(rl.allow("dev-1", "ctrl-1"));
        // Second request for same controller (different device) — should be blocked
        // due to controller rate
        assert!(!rl.allow("dev-2", "ctrl-1"));
    }

    #[test]
    fn global_rate_limit_blocks_when_exceeded() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 60_000,
            global_requests_per_second: 1,
            ..test_config()
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-2", "ctrl-2"));
        assert!(!rl.allow("dev-3", "ctrl-3"));
    }

    #[test]
    fn concurrent_requests_share_controller_bucket() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_minute: 1,
            global_requests_per_second: 100_000,
            ..test_config()
        };
        let rl = std::sync::Arc::new(RateLimiter::new(&config));
        let mut handles = Vec::new();
        for _ in 0..20 {
            let limiter = rl.clone();
            handles.push(std::thread::spawn(move || limiter.allow("dev-1", "ctrl-1")));
        }

        let allowed = handles
            .into_iter()
            .map(|h| h.join().expect("thread should not panic"))
            .filter(|allowed| *allowed)
            .count();
        assert_eq!(allowed, 1);
    }
}
