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
