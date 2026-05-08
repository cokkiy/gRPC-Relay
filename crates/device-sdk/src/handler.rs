use bytes::Bytes;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct DataRequestContext {
    pub device_id: String,
    pub connection_id: String,
    pub sequence_number: i64,
    pub received_at: SystemTime,
}

pub type EncryptedPayload = Bytes;

#[async_trait::async_trait]
pub trait DeviceDataHandler: Send + Sync + 'static {
    async fn on_data_request(
        &self,
        ctx: DataRequestContext,
        encrypted_payload: EncryptedPayload,
    ) -> anyhow::Result<EncryptedPayload>;
}
