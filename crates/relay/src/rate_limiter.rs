use crate::config::RateLimitConfig;
use dashmap::{mapref::entry::Entry, DashMap};
use std::time::Instant;

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token-bucket rate limiter per device, per controller, and globally.
///
/// Uses DashMap without inner Mutex — the entry API provides exclusive
/// access to each bucket, making it safe in async contexts.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    device_rate: f64,
    controller_rate: f64,
    global_rate: f64,
    buckets: DashMap<String, Bucket>,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            device_rate: config.device_requests_per_second as f64,
            controller_rate: config.controller_requests_per_second as f64,
            global_rate: config.global_requests_per_second as f64,
            buckets: DashMap::new(),
        }
    }

    pub fn allow(&self, device_id: &str, controller_id: &str) -> bool {
        if !self.check_key("global", self.global_rate) {
            return false;
        }
        if !self.check_key(&format!("device:{device_id}"), self.device_rate) {
            return false;
        }
        self.check_key(&format!("controller:{controller_id}"), self.controller_rate)
    }

    fn check_key(&self, key: &str, rate: f64) -> bool {
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
                    tokens: rate - 1.0,
                    last: now,
                });
                true
            }
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            device_requests_per_second: 100,
            controller_requests_per_second: 1000,
            global_requests_per_second: 100_000,
        }
    }

    #[test]
    fn allows_first_request() {
        let rl = RateLimiter::new(&test_config());
        assert!(rl.allow("dev-1", "ctrl-1"));
    }

    #[test]
    fn denies_after_exhaustion() {
        let config = RateLimitConfig {
            device_requests_per_second: 2,
            controller_requests_per_second: 1000,
            global_requests_per_second: 100_000,
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(rl.allow("dev-1", "ctrl-1"));
        // third request should be denied (2 tokens, per-device limit)
        assert!(!rl.allow("dev-1", "ctrl-1"));
    }

    #[test]
    fn different_devices_independent() {
        let config = RateLimitConfig {
            device_requests_per_second: 1,
            controller_requests_per_second: 1000,
            global_requests_per_second: 100_000,
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-1", "ctrl-1"));
        // different device should still be allowed
        assert!(rl.allow("dev-2", "ctrl-1"));
    }

    #[test]
    fn global_limit_applies() {
        let config = RateLimitConfig {
            device_requests_per_second: 1000,
            controller_requests_per_second: 1000,
            global_requests_per_second: 1,
        };
        let rl = RateLimiter::new(&config);
        assert!(rl.allow("dev-1", "ctrl-1"));
        assert!(!rl.allow("dev-2", "ctrl-2"));
    }
}
