use crate::auth::{ControllerPrincipal, DevicePrincipal};
use crate::config::AuthConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationError {
    Unauthorized,
    MethodNotAllowed,
    DeviceProjectForbidden,
}

#[derive(Debug, Clone)]
pub struct RbacPolicyEngine {
    enabled: bool,
    method_whitelist: Vec<String>,
}

impl RbacPolicyEngine {
    pub fn new(config: &AuthConfig) -> Self {
        Self {
            enabled: config.enabled,
            method_whitelist: config.method_whitelist.clone(),
        }
    }

    pub fn is_method_allowed(&self, method_name: &str) -> bool {
        if !self.enabled {
            return true;
        }
        if self.method_whitelist.is_empty() {
            return true;
        }
        self.method_whitelist.iter().any(|m| m == method_name)
    }

    /// RBAC + device归属授权（MVP：admin/role 仅用于放行；viewer/operator 仅按 project_id 授权）
    pub fn authorize_controller_to_device(
        &self,
        controller: &ControllerPrincipal,
        device: &DevicePrincipal,
        method_name: &str,
    ) -> Result<(), AuthorizationError> {
        if !self.enabled {
            return Ok(());
        }

        if !self.is_method_allowed(method_name) {
            return Err(AuthorizationError::MethodNotAllowed);
        }

        if controller.role == "admin" {
            return Ok(());
        }

        // MVP：根据 controller.allowed_project_ids 与 device.project_id 做归属匹配
        if controller
            .allowed_project_ids
            .iter()
            .any(|pid| pid == &device.project_id)
        {
            Ok(())
        } else {
            Err(AuthorizationError::DeviceProjectForbidden)
        }
    }
}
