use device_sdk::{
    handler::{DataRequestContext, DeviceDataHandler, EncryptedPayload},
    DeviceConnectClient, DeviceSdkConfig,
};
use tracing_subscriber::EnvFilter;

#[derive(Clone, Default)]
struct EchoDeviceHandler;

#[async_trait::async_trait]
impl DeviceDataHandler for EchoDeviceHandler {
    async fn on_data_request(
        &self,
        _ctx: DataRequestContext,
        encrypted_payload: EncryptedPayload,
    ) -> anyhow::Result<EncryptedPayload> {
        // MVP 示例：不解密、不做业务语义，只回显 opaque payload（用于验证链路闭环）
        Ok(encrypted_payload)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = match std::env::var("STATION_SERVICE_CONFIG") {
        Ok(path) => DeviceSdkConfig::load(path)?,
        Err(_) => DeviceSdkConfig::from_env()?,
    };

    let client = DeviceConnectClient::new(config, EchoDeviceHandler)?;
    client.run().await?;

    Ok(())
}
