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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ControllerPrincipal;

    fn make_ctrl(controller_id: &str, role: &str, projects: &[&str]) -> ControllerPrincipal {
        ControllerPrincipal {
            controller_id: controller_id.to_string(),
            role: role.to_string(),
            allowed_project_ids: projects.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_dev(device_id: &str, project_id: &str) -> DevicePrincipal {
        DevicePrincipal {
            device_id: device_id.to_string(),
            project_id: project_id.to_string(),
        }
    }

    fn make_engine() -> RbacPolicyEngine {
        RbacPolicyEngine {
            enabled: true,
            method_whitelist: vec!["ExecuteCommand".into(), "QueryStatus".into()],
        }
    }

    #[test]
    fn test_admin_can_access_any_device() {
        let engine = make_engine();
        let ctrl = make_ctrl("admin-1", "admin", &[]);
        let dev = make_dev("dev-1", "proj-a");
        assert!(engine
            .authorize_controller_to_device(&ctrl, &dev, "ExecuteCommand")
            .is_ok());
    }

    #[test]
    fn test_operator_can_access_own_project_device() {
        let engine = make_engine();
        let ctrl = make_ctrl("op-1", "operator", &["proj-a"]);
        let dev = make_dev("dev-1", "proj-a");
        assert!(engine
            .authorize_controller_to_device(&ctrl, &dev, "ExecuteCommand")
            .is_ok());
    }

    #[test]
    fn test_operator_cannot_access_other_project_device() {
        let engine = make_engine();
        let ctrl = make_ctrl("op-1", "operator", &["proj-a"]);
        let dev = make_dev("dev-2", "proj-b");
        let result = engine.authorize_controller_to_device(&ctrl, &dev, "ExecuteCommand");
        assert!(matches!(
            result,
            Err(AuthorizationError::DeviceProjectForbidden)
        ));
    }

    #[test]
    fn test_viewer_cannot_execute_control_command() {
        let mut engine = make_engine();
        engine.method_whitelist = vec!["ExecuteCommand".into()];
        let ctrl = make_ctrl("viewer-1", "viewer", &["proj-a"]);
        let dev = make_dev("dev-1", "proj-a");
        // QueryStatus is not in whitelist
        let result = engine.authorize_controller_to_device(&ctrl, &dev, "QueryStatus");
        assert!(matches!(result, Err(AuthorizationError::MethodNotAllowed)));
    }

    #[test]
    fn test_method_not_in_whitelist_rejected() {
        let engine = make_engine();
        let ctrl = make_ctrl("admin-1", "admin", &[]);
        let dev = make_dev("dev-1", "proj-a");
        let result = engine.authorize_controller_to_device(&ctrl, &dev, "DeleteAll");
        assert!(matches!(result, Err(AuthorizationError::MethodNotAllowed)));
    }

    #[test]
    fn test_rbac_disabled_allows_all() {
        let engine = RbacPolicyEngine {
            enabled: false,
            method_whitelist: vec![],
        };
        let ctrl = make_ctrl("anyone", "viewer", &[]);
        let dev = make_dev("dev-1", "some-other-project");
        assert!(engine
            .authorize_controller_to_device(&ctrl, &dev, "DeleteAll")
            .is_ok());
    }

    #[test]
    fn test_authorization_denied_returns_correct_error() {
        let engine = make_engine();
        let ctrl = make_ctrl("op-1", "operator", &["proj-a"]);
        let dev = make_dev("dev-2", "proj-b");
        let result = engine.authorize_controller_to_device(&ctrl, &dev, "ExecuteCommand");
        assert!(result.is_err());
        match result {
            Err(AuthorizationError::DeviceProjectForbidden) => {} // expected
            _ => panic!("expected DeviceProjectForbidden"),
        }
    }

    #[test]
    fn test_is_method_allowed_with_empty_whitelist() {
        let engine = RbacPolicyEngine {
            enabled: true,
            method_whitelist: vec![],
        };
        assert!(engine.is_method_allowed("anything"));
    }

    #[test]
    fn test_is_method_disabled_when_not_in_whitelist() {
        let engine = RbacPolicyEngine {
            enabled: true,
            method_whitelist: vec!["OnlyMethod".into()],
        };
        assert!(engine.is_method_allowed("OnlyMethod"));
        assert!(!engine.is_method_allowed("OtherMethod"));
    }
}
