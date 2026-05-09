use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct SecurityMetrics {
    inner: Arc<SecurityMetricsInner>,
}

#[derive(Debug, Default)]
struct SecurityMetricsInner {
    auth_success_total: AtomicU64,
    auth_failure_total: AtomicU64,
    authorization_denied_total: AtomicU64,
    rate_limit_total: AtomicU64,
    revoked_tokens_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecurityMetricsSnapshot {
    pub auth_success_total: u64,
    pub auth_failure_total: u64,
    pub authorization_denied_total: u64,
    pub rate_limit_total: u64,
    pub revoked_tokens_total: u64,
    pub auth_failure_ratio: f64,
    pub authorization_denied_ratio: f64,
    pub rate_limit_ratio: f64,
}

impl SecurityMetrics {
    pub fn record_auth_success(&self) {
        self.inner
            .auth_success_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_auth_failure(&self) {
        self.inner
            .auth_failure_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_authorization_denied(&self) {
        self.inner
            .authorization_denied_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rate_limit(&self) {
        self.inner.rate_limit_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_revoked_token(&self) {
        self.inner
            .revoked_tokens_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> SecurityMetricsSnapshot {
        let auth_success_total = self.inner.auth_success_total.load(Ordering::Relaxed);
        let auth_failure_total = self.inner.auth_failure_total.load(Ordering::Relaxed);
        let authorization_denied_total = self
            .inner
            .authorization_denied_total
            .load(Ordering::Relaxed);
        let rate_limit_total = self.inner.rate_limit_total.load(Ordering::Relaxed);
        let revoked_tokens_total = self.inner.revoked_tokens_total.load(Ordering::Relaxed);
        let auth_total = auth_success_total + auth_failure_total;

        SecurityMetricsSnapshot {
            auth_success_total,
            auth_failure_total,
            authorization_denied_total,
            rate_limit_total,
            revoked_tokens_total,
            auth_failure_ratio: ratio(auth_failure_total, auth_total),
            authorization_denied_ratio: ratio(authorization_denied_total, auth_total),
            rate_limit_ratio: ratio(rate_limit_total, auth_total),
        }
    }
}

fn ratio(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64
    }
}
