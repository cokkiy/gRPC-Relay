use crate::config::ControllerSdkConfig;
use crate::connect::{ConnectToDeviceOptions, ControllerConnectSession};
use crate::error::ControllerSdkError;
use crate::list::DeviceInfoExt;
use crate::Result;
use relay_proto::relay::v1::{
    relay_service_client::RelayServiceClient, DeviceInfo, ListOnlineDevicesRequest,
};
use tonic::transport::Endpoint;
use tonic::Request;

#[derive(Debug, Clone)]
pub struct ControllerClient {
    config: ControllerSdkConfig,
}

impl ControllerClient {
    pub fn new(config: ControllerSdkConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &ControllerSdkConfig {
        &self.config
    }

    pub async fn list_online_devices(
        &self,
        region_filter: Option<&str>,
    ) -> Result<Vec<DeviceInfoExt>> {
        let endpoint = Endpoint::from_shared(self.config.normalized_endpoint()?)
            .map_err(|e| ControllerSdkError::Transport(e.into()))?;
        let channel = endpoint
            .connect()
            .await
            .map_err(ControllerSdkError::Transport)?;

        let mut client = RelayServiceClient::new(channel);

        let token = self.config.token_provider().token()?;
        let region = region_filter.unwrap_or_default().to_string();

        // Note: relay-proto ListOnlineDevicesRequest has `region_filter` as string.
        let req = ListOnlineDevicesRequest {
            controller_id: self.config.controller_id.clone(),
            token,
            region_filter: region,
        };

        let resp = client
            .list_online_devices(Request::new(req))
            .await
            .map_err(ControllerSdkError::Grpc)?;

        let ListOnlineDevicesResponse { devices } = resp.into_inner();
        Ok(devices.into_iter().map(|d: DeviceInfo| d.into()).collect())
    }

    pub async fn connect_to_device(
        &self,
        target_device_id: impl Into<String>,
    ) -> Result<ControllerConnectSession> {
        let opts = ConnectToDeviceOptions {
            relay_endpoint: self.config.normalized_endpoint()?,
            controller_id: self.config.controller_id.clone(),
            token: self.config.token_provider().token()?,
            target_device_id: target_device_id.into(),
            max_payload_bytes: self.config.max_payload_bytes,
            request_timeout: crate::connect::RequestTimeout::default(),
        };

        ControllerConnectSession::connect(opts).await
    }
}

use relay_proto::relay::v1::ListOnlineDevicesResponse;
