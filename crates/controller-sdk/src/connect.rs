use crate::error::ControllerSdkError;
use crate::session::{PendingRequests, SequenceResult};
use bytes::Bytes;
use relay_proto::relay::v1::relay_service_client::RelayServiceClient;
use relay_proto::relay::v1::{ControllerMessage, DeviceResponse, ErrorCode};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;
use tonic::Request;

use crate::error::Result;

pub type RequestPayload = Bytes;

#[derive(Debug, Clone)]
pub struct RequestTimeout {
    pub send_sequence_timeout: std::time::Duration,
}

impl Default for RequestTimeout {
    fn default() -> Self {
        Self {
            send_sequence_timeout: std::time::Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectToDeviceOptions {
    pub relay_endpoint: String,
    pub controller_id: String,
    pub token: String,
    pub target_device_id: String,
    pub max_payload_bytes: usize,
    pub request_timeout: RequestTimeout,
}

impl ConnectToDeviceOptions {
    pub fn new(
        relay_endpoint: String,
        controller_id: String,
        token: String,
        target_device_id: String,
        max_payload_bytes: usize,
    ) -> Self {
        Self {
            relay_endpoint,
            controller_id,
            token,
            target_device_id,
            max_payload_bytes,
            request_timeout: RequestTimeout::default(),
        }
    }
}

#[derive(Debug)]
pub struct ControllerConnectSession {
    target_device_id: String,
    controller_id: String,
    token: String,

    pending: Arc<PendingRequests>,

    // Keep the sender alive for the session lifetime.
    outbound_tx: Mutex<mpsc::Sender<ControllerMessage>>,
}

impl ControllerConnectSession {
    pub async fn connect(opts: ConnectToDeviceOptions) -> Result<Self> {
        let endpoint = Endpoint::from_shared(opts.relay_endpoint.clone())
            .map_err(|e| ControllerSdkError::Transport(e.to_string()))?;
        let channel = endpoint
            .connect()
            .await
            .map_err(|e| ControllerSdkError::Transport(e.to_string()))?;

        let mut client = RelayServiceClient::new(channel);

        // writer channel (outbound ControllerMessage stream)
        let (tx, rx) = mpsc::channel::<ControllerMessage>(64);
        let outbound = ReceiverStream::new(rx);
        let request = Request::new(outbound);

        let response_stream = client
            .connect_to_device(request)
            .await
            .map_err(|e| ControllerSdkError::Grpc(e.to_string()))?
            .into_inner();

        let pending = Arc::new(PendingRequests::new());
        let pending_reader = pending.clone();

        let token = opts.token.clone();
        let controller_id = opts.controller_id.clone();

        // Spawn background task to dispatch DeviceResponse by sequence_number.
        tokio::spawn(async move {
            let mut response_stream = response_stream;
            while let Some(resp) = response_stream.message().await.transpose() {
                match resp {
                    Ok(device_resp) => {
                        let seq = device_resp.sequence_number;
                        let result = map_device_response_to_sequence_result(device_resp);
                        pending_reader.complete(seq, result).await;
                    }
                    Err(_status) => {
                        // Stream error: fail remaining pending.
                        break;
                    }
                }
            }

            // Best-effort: notify remaining waiters as stream closed.
            // (PendingRequests currently doesn't have an API to iterate remaining keys, so
            // we rely on timeout in send_request.)
            let _ = (token, controller_id);
        });

        Ok(Self {
            target_device_id: opts.target_device_id,
            controller_id: opts.controller_id,
            token: opts.token,
            pending,
            outbound_tx: Mutex::new(tx),
        })
    }

    pub async fn send_request(
        &self,
        method_name: String,
        sequence_number: i64,
        encrypted_payload: RequestPayload,
        request_timeout: std::time::Duration,
    ) -> Result<Bytes> {
        if encrypted_payload.is_empty() {
            // Allow empty payload; don't reject.
        }

        let rx = self.pending.insert(sequence_number).await?;

        let msg = ControllerMessage {
            controller_id: self.controller_id.clone(),
            token: self.token.clone(),
            target_device_id: self.target_device_id.clone(),
            method_name,
            sequence_number,
            encrypted_payload: encrypted_payload.to_vec(),
        };

        {
            let tx_guard = self.outbound_tx.lock().await;
            if tx_guard.send(msg).await.is_err() {
                self.pending.remove(sequence_number).await;
                return Err(ControllerSdkError::StreamClosed);
            }
        }

        match tokio::time::timeout(request_timeout, rx).await {
            Ok(Ok(seq_result)) => seq_result,
            Ok(Err(_recv_error)) => {
                self.pending.remove(sequence_number).await;
                Err(ControllerSdkError::StreamClosed)
            }
            Err(_) => {
                self.pending.remove(sequence_number).await;
                Err(ControllerSdkError::SequenceResponseNotFound)
            }
        }
    }

    pub fn pending(&self) -> Arc<PendingRequests> {
        self.pending.clone()
    }
}

fn map_device_response_to_sequence_result(device_resp: DeviceResponse) -> SequenceResult {
    let err_code = device_resp.error;

    let result = match err_code {
        x if x == ErrorCode::Ok as i32 => Ok(Bytes::from(device_resp.encrypted_payload)),
        x if x == ErrorCode::DeviceOffline as i32 => Err(ControllerSdkError::DeviceOffline),
        x if x == ErrorCode::Unauthorized as i32 => Err(ControllerSdkError::Unauthorized),
        x if x == ErrorCode::DeviceNotFound as i32 => Err(ControllerSdkError::DeviceNotFound),
        x if x == ErrorCode::RateLimited as i32 => Err(ControllerSdkError::RateLimited),
        _ => Err(ControllerSdkError::DeviceOffline),
    };

    result
}
