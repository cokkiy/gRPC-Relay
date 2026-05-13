use crate::config::{AuthConfig, ControllerAuthEntry, DeviceAuthEntry};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct ControllerPrincipal {
    pub controller_id: String,
    pub role: String,
    pub allowed_project_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DevicePrincipal {
    pub device_id: String,
    pub project_id: String,
}

#[derive(Debug, Clone)]
pub enum AuthError {
    InvalidToken,
    ControllerIdMismatch,
    DeviceIdMismatch,
    UnknownDevice,
    RevokedToken,
    JwtNotConfigured,
}

#[derive(Debug, Clone)]
pub struct AuthService {
    enabled: bool,
    jwt: JwtAuth,
    controller_tokens: HashMap<String, ControllerAuthEntry>, // token -> entry
    device_tokens: HashMap<String, DeviceAuthEntry>,         // token -> entry
    device_by_id: HashMap<String, DeviceAuthEntry>,          // device_id -> entry
    revoked_controller_tokens: Arc<RwLock<HashSet<String>>>,
    revoked_device_tokens: Arc<RwLock<HashSet<String>>>,
}

#[derive(Debug, Clone)]
struct JwtAuth {
    enabled: bool,
    secret: String,
    issuer: Option<String>,
    audience: Option<String>,
    clock_skew_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerClaims {
    pub sub: String,
    pub controller_id: String,
    pub role: String,
    #[serde(default)]
    pub allowed_project_ids: Vec<String>,
    pub exp: usize,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<String>,
}

impl AuthService {
    pub fn new(config: &AuthConfig) -> Self {
        let mut device_by_id = HashMap::new();
        for entry in config.device_tokens.values() {
            device_by_id.insert(entry.device_id.clone(), entry.clone());
        }

        Self {
            enabled: config.enabled,
            jwt: JwtAuth {
                enabled: config.jwt.enabled,
                secret: config.jwt.hs256_secret.clone(),
                issuer: config.jwt.issuer.clone(),
                audience: config.jwt.audience.clone(),
                clock_skew_seconds: config.jwt.clock_skew_seconds,
            },
            controller_tokens: config.controller_tokens.clone(),
            device_tokens: config.device_tokens.clone(),
            device_by_id,
            revoked_controller_tokens: Arc::new(RwLock::new(HashSet::new())),
            revoked_device_tokens: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Authenticate controller by (controller_id, token).
    pub fn authenticate_controller(
        &self,
        controller_id: &str,
        token: &str,
    ) -> Result<ControllerPrincipal, AuthError> {
        if !self.enabled {
            return Ok(ControllerPrincipal {
                controller_id: controller_id.to_string(),
                role: "admin".to_string(),
                allowed_project_ids: Vec::new(),
            });
        }

        if self.is_controller_token_revoked(token) {
            return Err(AuthError::RevokedToken);
        }

        if self.jwt.enabled {
            return self.authenticate_controller_jwt(controller_id, token);
        }

        let entry = self
            .controller_tokens
            .get(token)
            .ok_or(AuthError::InvalidToken)?;

        if entry.controller_id != controller_id {
            return Err(AuthError::ControllerIdMismatch);
        }

        Ok(ControllerPrincipal {
            controller_id: entry.controller_id.clone(),
            role: entry.role.clone(),
            allowed_project_ids: entry.allowed_project_ids.clone(),
        })
    }

    fn authenticate_controller_jwt(
        &self,
        controller_id: &str,
        token: &str,
    ) -> Result<ControllerPrincipal, AuthError> {
        if self.jwt.secret.is_empty() {
            return Err(AuthError::JwtNotConfigured);
        }

        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = self.jwt.clock_skew_seconds;
        if let Some(issuer) = &self.jwt.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        if let Some(audience) = &self.jwt.audience {
            validation.set_audience(&[audience.as_str()]);
        } else {
            validation.validate_aud = false;
        }

        let data = decode::<ControllerClaims>(
            token,
            &DecodingKey::from_secret(self.jwt.secret.as_bytes()),
            &validation,
        )
        .map_err(|_| AuthError::InvalidToken)?;

        if data.claims.controller_id != controller_id || data.claims.sub != controller_id {
            return Err(AuthError::ControllerIdMismatch);
        }

        Ok(ControllerPrincipal {
            controller_id: data.claims.controller_id,
            role: data.claims.role,
            allowed_project_ids: data.claims.allowed_project_ids,
        })
    }

    /// Authenticate device by device token.
    pub fn authenticate_device_by_token(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<DevicePrincipal, AuthError> {
        if !self.enabled {
            return Ok(DevicePrincipal {
                device_id: device_id.to_string(),
                project_id: "".to_string(),
            });
        }

        if self.is_device_token_revoked(token) {
            return Err(AuthError::RevokedToken);
        }

        let entry = self
            .device_tokens
            .get(token)
            .ok_or(AuthError::InvalidToken)?;

        if entry.device_id != device_id {
            return Err(AuthError::DeviceIdMismatch);
        }

        Ok(DevicePrincipal {
            device_id: entry.device_id.clone(),
            project_id: entry.project_id.clone(),
        })
    }

    /// Lookup device authorization context by device_id (even if offline).
    pub fn get_device_principal_by_id(
        &self,
        device_id: &str,
    ) -> Result<DevicePrincipal, AuthError> {
        if !self.enabled {
            return Ok(DevicePrincipal {
                device_id: device_id.to_string(),
                project_id: "".to_string(),
            });
        }

        let entry = self
            .device_by_id
            .get(device_id)
            .ok_or(AuthError::UnknownDevice)?;

        Ok(DevicePrincipal {
            device_id: entry.device_id.clone(),
            project_id: entry.project_id.clone(),
        })
    }

    pub fn token_prefix(token: &str) -> String {
        token.chars().take(8).collect()
    }

    pub fn revoke_controller_token(&self, token_hash_or_prefix: &str) {
        if let Ok(mut tokens) = self.revoked_controller_tokens.write() {
            tokens.insert(token_hash_or_prefix.to_string());
        }
    }

    pub fn revoke_device_token(&self, token_hash_or_prefix: &str) {
        if let Ok(mut tokens) = self.revoked_device_tokens.write() {
            tokens.insert(token_hash_or_prefix.to_string());
        }
    }

    fn is_controller_token_revoked(&self, token: &str) -> bool {
        token_is_revoked(&self.revoked_controller_tokens, token)
    }

    fn is_device_token_revoked(&self, token: &str) -> bool {
        token_is_revoked(&self.revoked_device_tokens, token)
    }
}

fn token_is_revoked(tokens: &RwLock<HashSet<String>>, token: &str) -> bool {
    let Ok(tokens) = tokens.read() else {
        return true;
    };
    tokens
        .iter()
        .any(|revoked| token == revoked || token.starts_with(revoked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, ControllerAuthEntry, DeviceAuthEntry, JwtConfig};

    fn make_test_auth_service() -> AuthService {
        let mut ct_tokens = HashMap::new();
        ct_tokens.insert(
            "ctrl-token-1".to_string(),
            ControllerAuthEntry {
                controller_id: "ctrl-1".to_string(),
                role: "admin".to_string(),
                allowed_project_ids: vec!["proj-a".to_string()],
            },
        );
        let mut dt_tokens = HashMap::new();
        dt_tokens.insert(
            "dev-token-1".to_string(),
            DeviceAuthEntry {
                device_id: "dev-1".to_string(),
                project_id: "proj-a".to_string(),
            },
        );
        dt_tokens.insert(
            "dev-token-2".to_string(),
            DeviceAuthEntry {
                device_id: "dev-2".to_string(),
                project_id: "proj-b".to_string(),
            },
        );
        let config = AuthConfig {
            enabled: true,
            controller_tokens: ct_tokens,
            device_tokens: dt_tokens,
            method_whitelist: vec![],
            jwt: JwtConfig {
                enabled: false,
                hs256_secret: String::new(),
                issuer: None,
                audience: None,
                clock_skew_seconds: 30,
            },
        };
        AuthService::new(&config)
    }

    fn make_jwt_auth_service() -> AuthService {
        let config = AuthConfig {
            enabled: true,
            controller_tokens: HashMap::new(),
            device_tokens: HashMap::new(),
            method_whitelist: vec![],
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

    #[test]
    fn test_authenticate_controller_valid_jwt() {
        let auth = make_jwt_auth_service();
        let jwt = crate::test_helpers::make_controller_jwt("ctrl-1", "admin", &[]);
        let principal = auth.authenticate_controller("ctrl-1", &jwt).unwrap();
        assert_eq!(principal.controller_id, "ctrl-1");
        assert_eq!(principal.role, "admin");
    }

    #[test]
    fn test_authenticate_controller_expired_jwt() {
        let auth = make_jwt_auth_service();
        let jwt = crate::test_helpers::make_expired_controller_jwt("ctrl-1", "admin", &[]);
        let result = auth.authenticate_controller("ctrl-1", &jwt);
        assert!(matches!(result, Err(AuthError::InvalidToken)));
    }

    #[test]
    fn test_authenticate_controller_wrong_signature() {
        let auth = make_jwt_auth_service();
        let jwt = crate::test_helpers::make_forged_jwt("ctrl-1");
        let result = auth.authenticate_controller("ctrl-1", &jwt);
        assert!(matches!(result, Err(AuthError::InvalidToken)));
    }

    #[test]
    fn test_authenticate_controller_id_mismatch() {
        let auth = make_jwt_auth_service();
        let jwt = crate::test_helpers::make_controller_jwt("ctrl-1", "admin", &[]);
        let result = auth.authenticate_controller("ctrl-2", &jwt);
        assert!(matches!(result, Err(AuthError::ControllerIdMismatch)));
    }

    #[test]
    fn test_authenticate_controller_revoked_token() {
        let auth = make_jwt_auth_service();
        let jwt = crate::test_helpers::make_controller_jwt("ctrl-1", "admin", &[]);
        // Revoke the token
        auth.revoke_controller_token(&jwt);
        let result = auth.authenticate_controller("ctrl-1", &jwt);
        assert!(matches!(result, Err(AuthError::RevokedToken)));
    }

    #[test]
    fn test_authenticate_device_valid_token() {
        let auth = make_test_auth_service();
        let principal = auth
            .authenticate_device_by_token("dev-1", "dev-token-1")
            .unwrap();
        assert_eq!(principal.device_id, "dev-1");
        assert_eq!(principal.project_id, "proj-a");
    }

    #[test]
    fn test_authenticate_device_unknown_token() {
        let auth = make_test_auth_service();
        let result = auth.authenticate_device_by_token("dev-unknown", "invalid-token");
        assert!(matches!(result, Err(AuthError::InvalidToken)));
    }

    #[test]
    fn test_revoke_token_takes_effect() {
        let auth = make_test_auth_service();
        // Authenticate first
        assert!(auth
            .authenticate_controller("ctrl-1", "ctrl-token-1")
            .is_ok());
        // Revoke
        auth.revoke_controller_token("ctrl-token-1");
        // Should fail now
        let result = auth.authenticate_controller("ctrl-1", "ctrl-token-1");
        assert!(matches!(result, Err(AuthError::RevokedToken)));
    }

    #[test]
    fn test_auth_disabled_allows_all() {
        let config = AuthConfig {
            enabled: false,
            ..Default::default()
        };
        let auth = AuthService::new(&config);
        let result = auth.authenticate_controller("anyone", "any-token").unwrap();
        assert_eq!(result.role, "admin");
        let device = auth
            .authenticate_device_by_token("dev-unknown", "")
            .unwrap();
        assert_eq!(device.device_id, "dev-unknown");
    }

    #[test]
    fn test_token_prefix_truncation() {
        let token = "abcdefghijklmnop";
        assert_eq!(AuthService::token_prefix(token), "abcdefgh");
        assert_eq!(AuthService::token_prefix("short"), "short");
        assert_eq!(AuthService::token_prefix(""), "");
    }

    #[test]
    fn test_jwt_not_configured_returns_error() {
        let config = AuthConfig {
            enabled: true,
            jwt: JwtConfig {
                enabled: true,
                hs256_secret: String::new(), // empty secret
                issuer: None,
                audience: None,
                clock_skew_seconds: 30,
            },
            ..Default::default()
        };
        let auth = AuthService::new(&config);
        let result = auth.authenticate_controller("ctrl-1", "some-jwt");
        assert!(matches!(result, Err(AuthError::JwtNotConfigured)));
    }

    #[test]
    fn test_get_device_principal_by_id() {
        let auth = make_test_auth_service();
        let device = auth.get_device_principal_by_id("dev-1").unwrap();
        assert_eq!(device.device_id, "dev-1");
        assert_eq!(device.project_id, "proj-a");

        let result = auth.get_device_principal_by_id("dev-nonexistent");
        assert!(matches!(result, Err(AuthError::UnknownDevice)));
    }

    #[test]
    fn test_authenticate_controller_valid_token() {
        let auth = make_test_auth_service();
        let principal = auth
            .authenticate_controller("ctrl-1", "ctrl-token-1")
            .unwrap();
        assert_eq!(principal.controller_id, "ctrl-1");
        assert_eq!(principal.role, "admin");
        assert_eq!(principal.allowed_project_ids, vec!["proj-a"]);
    }
}
