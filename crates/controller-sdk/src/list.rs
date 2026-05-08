use crate::error::ControllerSdkError;
use crate::Result;
use relay_proto::relay::v1::DeviceInfo;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfoExt {
    pub device_id: String,
    pub connection_id: String,
    pub relay_address: String,
    pub connected_at: i64,
    pub metadata: HashMap<String, String>,
}

impl From<DeviceInfo> for DeviceInfoExt {
    fn from(value: DeviceInfo) -> Self {
        Self {
            device_id: value.device_id,
            connection_id: value.connection_id,
            relay_address: value.relay_address,
            connected_at: value.connected_at,
            metadata: value.metadata,
        }
    }
}

impl DeviceInfoExt {
    pub fn metadata_get(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }

    pub fn require_region(&self) -> Result<&str> {
        self.metadata_get("region").ok_or_else(|| {
            ControllerSdkError::InvalidConfig("missing region in device metadata".into())
        })
    }
}
