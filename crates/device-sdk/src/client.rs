use crate::{
    backoff::RetryBackoff,
    config::DeviceSdkConfig,
    error::{DeviceSdkError, Result},
    handler::{DataRequestContext, DeviceDataHandler, EncryptedPayload},
};
use bytes::Bytes;
use relay_proto::relay::v1::{
    device_message, relay_message, relay_service_client::RelayServiceClient, DataRequest,
    DataResponse, DeviceMessage, ErrorCode, HeartbeatRequest, HeartbeatResponse, RegisterRequest,
    RegisterResponse,
};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;
use tonic::Request;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct DeviceConnectClient<H> {
    config: DeviceSdkConfig,
    handler: std::sync::Arc<H>,
}

impl<H> DeviceConnectClient<H>
where
    H: DeviceDataHandler,
{
    pub fn new(config: DeviceSdkConfig, handler: H) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            handler: std::sync::Arc::new(handler),
        })
    }

    /// 设备侧 stationService 永久运行：断线重连 + 会话恢复
    pub async fn run(&self) -> Result<()> {
        let backoff = RetryBackoff::new(
            self.config.backoff_initial_seconds,
            self.config.backoff_max_seconds,
        );

        // recovery state
        let mut last_connection_id: Option<String> = None;
        let mut last_disconnect_at_millis: Option<i64> = None;

        let mut attempt: u32 = 0;

        loop {
            match self
                .connect_once(last_connection_id.clone(), last_disconnect_at_millis)
                .await
            {
                Ok(new_connection_id) => {
                    last_connection_id = Some(new_connection_id);
                    last_disconnect_at_millis = Some(now_epoch_millis());
                    attempt = 0;

                    let sleep_seconds = backoff.next_sleep_seconds(attempt);
                    let retry_attempt = attempt.saturating_add(1);
                    tracing::warn!(
                        attempt = retry_attempt,
                        sleep_seconds,
                        "device connect closed after registration; retrying"
                    );

                    attempt = retry_attempt;
                    tokio::time::sleep(Duration::from_secs(sleep_seconds)).await;
                }
                Err(DeviceSdkError::ConnectionClosed) | Err(DeviceSdkError::Grpc(_)) => {
                    let sleep_seconds = backoff.next_sleep_seconds(attempt);
                    let retry_attempt = attempt.saturating_add(1);
                    tracing::warn!(
                        attempt = retry_attempt,
                        sleep_seconds,
                        "device connect closed; retrying"
                    );

                    attempt = retry_attempt;
                    last_disconnect_at_millis = Some(now_epoch_millis());
                    tokio::time::sleep(Duration::from_secs(sleep_seconds)).await;
                }
                Err(err) => {
                    tracing::error!(error=?err, "fatal error in device connect client; stop");
                    return Err(err);
                }
            }
        }
    }

    async fn connect_once(
        &self,
        previous_connection_id: Option<String>,
        last_disconnect_at_millis: Option<i64>,
    ) -> Result<String> {
        let endpoint = Endpoint::from_shared(normalize_http_uri(&self.config.relay.tcp_addr))
            .map_err(DeviceSdkError::Tonic)?;
        let channel = endpoint.connect().await?;
        let max_message_size = self.config.transport.max_payload_bytes;
        let mut client = RelayServiceClient::new(channel)
            .max_decoding_message_size(max_message_size)
            .max_encoding_message_size(max_message_size);

        let device_id = self.config.device_id.clone();
        let token = self.config.token.clone();

        // writer channel (outbound DeviceMessage stream)
        let (tx, rx) = mpsc::channel::<DeviceMessage>(64);
        let outbound = ReceiverStream::new(rx);
        let request = Request::new(outbound);

        let mut response_stream = client.device_connect(request).await?.into_inner();

        // recovery window decision
        if previous_connection_id.is_some() && self.config.session_recovery_window_seconds == 0 {
            return Err(DeviceSdkError::RecoveryDisabled);
        }

        let now_ms = now_epoch_millis();
        let previous_connection_id_for_register = previous_connection_id.and_then(|cid| {
            let disconnect_ms = last_disconnect_at_millis?;
            let elapsed = now_ms.saturating_sub(disconnect_ms);
            let window_ms: i64 = self
                .config
                .session_recovery_window_seconds
                .saturating_mul(1000)
                .min(i64::MAX as u64) as i64;
            if elapsed <= window_ms {
                Some(cid)
            } else {
                None
            }
        });

        let register_req = RegisterRequest {
            device_id: device_id.clone(),
            metadata: self.config.metadata.clone(),
            previous_connection_id: previous_connection_id_for_register.unwrap_or_default(),
        };

        // send Register
        tx.send(DeviceMessage {
            device_id: device_id.clone(),
            token: token.clone(),
            payload: Some(device_message::Payload::Register(register_req)),
        })
        .await
        .map_err(|_| DeviceSdkError::ConnectionClosed)?;

        // start heartbeat after first RegisterResponse (connection_id becomes known)
        let mut heartbeat_task: Option<JoinHandle<()>> = None;
        let mut registered_connection_id: Option<String> = None;
        let data_handler_limit = std::sync::Arc::new(Semaphore::new(64));

        loop {
            let relay_msg = match response_stream.message().await {
                Ok(Some(msg)) => msg,
                Ok(None) => break,
                Err(status) => {
                    if registered_connection_id.is_some() {
                        tracing::warn!(error = ?status, "response stream closed after registration");
                        break;
                    }
                    return Err(DeviceSdkError::Grpc(status));
                }
            };

            match relay_msg.payload {
                Some(relay_message::Payload::RegisterResponse(RegisterResponse {
                    connection_id,
                    session_resumed: _,
                    timestamp: _,
                })) => {
                    tracing::info!(
                        device_id=%device_id,
                        connection_id=%connection_id,
                        "registered (and possibly resumed)"
                    );

                    registered_connection_id = Some(connection_id.clone());

                    if heartbeat_task.is_none() {
                        let hb_tx = tx.clone();
                        let hb_device_id = device_id.clone();
                        let hb_token = token.clone();
                        let heartbeat_interval_seconds = self.config.heartbeat_interval_seconds;

                        heartbeat_task = Some(tokio::spawn(async move {
                            let mut interval = tokio::time::interval(Duration::from_secs(
                                heartbeat_interval_seconds,
                            ));
                            loop {
                                interval.tick().await;

                                let hb = HeartbeatRequest {
                                    connection_id: connection_id.clone(),
                                    timestamp: now_epoch_millis(),
                                };

                                // token/device_id 必须与会话一致
                                if hb_tx
                                    .send(DeviceMessage {
                                        device_id: hb_device_id.clone(),
                                        token: hb_token.clone(),
                                        payload: Some(device_message::Payload::Heartbeat(hb)),
                                    })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }));
                    }
                }

                Some(relay_message::Payload::HeartbeatResponse(HeartbeatResponse {
                    timestamp: _,
                })) => {
                    // MVP: ignore
                }

                Some(relay_message::Payload::DataRequest(DataRequest {
                    connection_id,
                    sequence_number,
                    encrypted_payload,
                })) => {
                    if encrypted_payload.len() > self.config.transport.max_payload_bytes {
                        tracing::warn!(
                            payload_bytes = encrypted_payload.len(),
                            max_payload_bytes = self.config.transport.max_payload_bytes,
                            "rejecting oversized device data request"
                        );

                        let resp = DataResponse {
                            connection_id,
                            sequence_number,
                            encrypted_payload: Vec::new(),
                            error: ErrorCode::InternalError as i32,
                        };

                        tx.send(DeviceMessage {
                            device_id: device_id.clone(),
                            token: token.clone(),
                            payload: Some(device_message::Payload::Data(resp)),
                        })
                        .await
                        .map_err(|_| DeviceSdkError::ConnectionClosed)?;
                        continue;
                    }

                    let ctx = DataRequestContext {
                        device_id: device_id.clone(),
                        connection_id,
                        sequence_number,
                        received_at: SystemTime::now(),
                    };

                    // 并发处理，避免阻塞 reader
                    let handler = self.handler.clone();
                    let tx_for_response = tx.clone();
                    let token_for_response = token.clone();
                    let permit = data_handler_limit
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| DeviceSdkError::ConnectionClosed)?;

                    tokio::spawn(async move {
                        let _permit = permit;
                        let encrypted_payload_bytes = Bytes::from(encrypted_payload);

                        let handler_result: anyhow::Result<EncryptedPayload> = handler
                            .on_data_request(ctx.clone(), encrypted_payload_bytes)
                            .await;

                        let (error_code, resp_bytes) = match handler_result {
                            Ok(bytes) => (ErrorCode::Ok, bytes),
                            Err(err) => {
                                tracing::error!(error=?err, "handler on_data_request failed");
                                (ErrorCode::InternalError, Bytes::new())
                            }
                        };

                        let resp = DataResponse {
                            connection_id: ctx.connection_id.clone(),
                            sequence_number: ctx.sequence_number,
                            encrypted_payload: resp_bytes.to_vec(),
                            error: error_code as i32,
                        };

                        // 回写 DeviceMessage.payload=Data
                        let _ = tx_for_response
                            .send(DeviceMessage {
                                device_id: ctx.device_id.clone(),
                                token: token_for_response,
                                payload: Some(device_message::Payload::Data(resp)),
                            })
                            .await;
                    });
                }
                None => {}
            }
        }

        if let Some(handle) = heartbeat_task {
            handle.abort();
        }

        if let Some(connection_id) = registered_connection_id {
            return Ok(connection_id);
        }

        Err(DeviceSdkError::ConnectionClosed)
    }
}

fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn normalize_http_uri(addr: &str) -> String {
    // tonic Endpoint 需要 scheme：http/https
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}
