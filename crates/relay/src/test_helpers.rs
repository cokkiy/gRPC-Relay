use crate::auth::{AuthService, ControllerPrincipal};
use crate::config::{AuthConfig, ControllerAuthEntry, JwtConfig, RateLimitConfig};
use crate::idempotency::IdempotencyCache;
use crate::rate_limiter::{ConnectionRateLimiter, RateLimiter};
use crate::session::SessionRegistry;
use crate::state::RelayState;
use jsonwebtoken::{encode, EncodingKey, Header};
use std::collections::HashMap;
use std::sync::Arc;

/// Create an AuthService with a JWT secret configured for testing.
pub fn make_auth_service(controller_tokens: Vec<(&str, &str, &str, &[&str])>) -> AuthService {
    let mut ct_map = HashMap::new();
    for (token, ctrl_id, role, projects) in controller_tokens {
        ct_map.insert(
            token.to_string(),
            ControllerAuthEntry {
                controller_id: ctrl_id.to_string(),
                role: role.to_string(),
                allowed_project_ids: projects.iter().map(|s| s.to_string()).collect(),
            },
        );
    }
    let config = AuthConfig {
        enabled: true,
        controller_tokens: ct_map,
        device_tokens: HashMap::new(),
        method_whitelist: vec!["ExecuteCommand".to_string(), "QueryStatus".to_string()],
        jwt: JwtConfig {
            enabled: true,
            hs256_secret: "test-secret-key".to_string(),
            issuer: None,
            audience: None,
            clock_skew_seconds: 30,
        },
    };
    AuthService::new(&config)
}

/// Create a ControllerPrincipal for test purposes.
pub fn make_controller_principal(
    controller_id: &str,
    role: &str,
    allowed_project_ids: &[&str],
) -> ControllerPrincipal {
    ControllerPrincipal {
        controller_id: controller_id.to_string(),
        role: role.to_string(),
        allowed_project_ids: allowed_project_ids.iter().map(|s| s.to_string()).collect(),
    }
}

/// Generate a valid HS256 JWT for testing.
pub fn make_controller_jwt(
    controller_id: &str,
    role: &str,
    allowed_project_ids: &[&str],
) -> String {
    let claims = crate::auth::ControllerClaims {
        sub: controller_id.to_string(),
        controller_id: controller_id.to_string(),
        role: role.to_string(),
        allowed_project_ids: allowed_project_ids.iter().map(|s| s.to_string()).collect(),
        exp: 2_000_000_000, // far future and portable across 32/64-bit usize
        iss: None,
        aud: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret("test-secret-key".as_bytes()),
    )
    .expect("JWT encoding should succeed")
}

/// Generate an expired JWT for testing expiry rejection.
pub fn make_expired_controller_jwt(
    controller_id: &str,
    role: &str,
    allowed_project_ids: &[&str],
) -> String {
    let claims = crate::auth::ControllerClaims {
        sub: controller_id.to_string(),
        controller_id: controller_id.to_string(),
        role: role.to_string(),
        allowed_project_ids: allowed_project_ids.iter().map(|s| s.to_string()).collect(),
        exp: 100, // expired long ago
        iss: None,
        aud: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret("test-secret-key".as_bytes()),
    )
    .expect("JWT encoding should succeed")
}

/// Generate a JWT signed with a wrong key.
pub fn make_forged_jwt(controller_id: &str) -> String {
    let claims = crate::auth::ControllerClaims {
        sub: controller_id.to_string(),
        controller_id: controller_id.to_string(),
        role: "admin".to_string(),
        allowed_project_ids: vec![],
        exp: 2_000_000_000,
        iss: None,
        aud: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret("wrong-secret-key".as_bytes()),
    )
    .expect("JWT encoding should succeed")
}

/// Build a test RelayState with some pre-registered devices.
pub fn build_test_state() -> Arc<RelayState> {
    Arc::new(RelayState::new())
}

/// Build a SessionRegistry backed by a fresh RelayState.
pub fn build_test_session_registry() -> SessionRegistry {
    SessionRegistry::new(build_test_state())
}

/// Build IdempotencyCache with small capacity for testing.
pub fn build_test_idempotency_cache() -> IdempotencyCache {
    IdempotencyCache::new(100, 3600)
}

/// Build a RateLimiter with test defaults.
pub fn build_test_rate_limiter() -> RateLimiter {
    let config = RateLimitConfig {
        device_requests_per_second: 1000,
        controller_requests_per_minute: 60000,
        global_requests_per_second: 100000,
        device_connection_per_minute: 10,
        global_connections_per_second: 100,
        device_bandwidth_bytes_per_sec: 10 * 1024 * 1024,
        controller_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
        global_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
        cpu_threshold_percent: 80.0,
        memory_threshold_mb: 12 * 1024,
    };
    RateLimiter::new(&config)
}

/// Build a ConnectionRateLimiter for testing.
pub fn build_test_connection_limiter() -> ConnectionRateLimiter {
    let config = RateLimitConfig {
        device_connection_per_minute: 10,
        global_connections_per_second: 100,
        ..Default::default()
    };
    ConnectionRateLimiter::new(&config)
}
