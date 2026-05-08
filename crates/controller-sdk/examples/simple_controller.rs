use bytes::Bytes;
use controller_sdk::{ControllerClient, ControllerSdkConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置来自环境变量（参见 ControllerSdkConfig::from_env）
    let config = ControllerSdkConfig::from_env()?;
    let client = ControllerClient::new(config)?;

    // 1) 拉取在线设备列表（可选按 region_filter 过滤）
    let devices = client.list_online_devices(None).await?;
    println!("online devices: {}", devices.len());

    // 2) 选择一个目标设备
    let device = devices
        .first()
        .expect("no online device found; start relay+device first");

    println!(
        "pick device_id={} connection_id={}",
        device.device_id, device.connection_id
    );

    // 3) 建立 ConnectToDevice 流会话
    let session = client.connect_to_device(device.device_id.clone()).await?;

    // 4) 发送一条 opaque encrypted_payload（此示例不加密/不解密，只演示字节直通）
    // 注意：真实 Controller 应该把业务 payload 进行端到端加密后放入 encrypted_payload。
    let sequence_number: i64 = 1;
    let method_name = "ExecuteCommand".to_string();
    let encrypted_payload = Bytes::from_static(b"opaque_encrypted_payload");

    let response = session
        .send_request(
            method_name,
            sequence_number,
            encrypted_payload,
            std::time::Duration::from_secs(30),
        )
        .await?;

    println!("got response payload bytes={}", response.len());
    Ok(())
}
