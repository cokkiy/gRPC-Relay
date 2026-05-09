use crate::config::AppConfig;
use crate::idempotency::IdempotencyCache;
use crate::rate_limiter::RateLimiter;
use crate::session::SessionRegistry;
use crate::state::{
    device_response_from_device_data, relay_message_data_request, relay_message_heartbeat_response,
    relay_message_register_response, RelayState,
};
use crate::stream::{StreamRouter, StreamRouterErrorKind};
use crate::validator;
use relay_proto::relay::v1::relay_service_server::RelayService;
use relay_proto::relay::v1::{
    device_message, ControllerMessage, DeviceMessage, DeviceResponse, ErrorCode,
    ListOnlineDevicesRequest, ListOnlineDevicesResponse, RelayMessage,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Clone)]
pub struct RelayGrpcService {
    state: Arc<RelayState>,
    relay_address: String,
    idempotency_cache: IdempotencyCache,
    rate_limiter: RateLimiter,
    stream_router: StreamRouter,
    session_registry: SessionRegistry,
    inflight_timeout: Duration,
}

impl RelayGrpcService {
    pub fn new(state: Arc<RelayState>, config: &AppConfig) -> Self {
        Self {
            idempotency_cache: IdempotencyCache::new(
                config.relay.idempotency.cache_capacity,
                config.relay.idempotency.cache_ttl_seconds,
            ),
            rate_limiter: RateLimiter::new(&config.relay.rate_limiting),
            stream_router: StreamRouter::new(&config.relay.stream),
            session_registry: SessionRegistry::new(state.clone()),
            inflight_timeout: Duration::from_secs(60),
            state,
            relay_address: config.relay.address.clone(),
        }
    }

    pub fn spawn_stale_stream_cleanup(&self) -> JoinHandle<()> {
        let router = self.stream_router.clone();
        tokio::spawn(async move {
            let interval = router.cleanup_interval();
            loop {
                tokio::time::sleep(interval).await;
                let stale = router.cleanup_stale();
                for mapping in stale {
                    let _ = mapping
                        .controller_tx
                        .send(Ok(error_resp(
                            &mapping.device_id,
                            0,
                            ErrorCode::DeviceOffline,
                        )))
                        .await;
                }
            }
        })
    }

    async fn run_device_connect_stream<S>(
        state: Arc<RelayState>,
        session_registry: SessionRegistry,
        stream_router: StreamRouter,
        mut inbound: S,
        out_tx: mpsc::Sender<Result<RelayMessage, Status>>,
    ) where
        S: Stream<Item = Result<DeviceMessage, Status>> + Unpin,
    {
        let mut current_device_id: Option<String> = None;

        while let Some(next_message) = inbound.next().await {
            let Ok(dev_msg) = next_message else {
                break;
            };
            let dev_id = dev_msg.device_id.clone();

            match dev_msg.payload {
                Some(device_message::Payload::Register(register_req)) => {
                    current_device_id = Some(dev_id.clone());

                    let connection_id = state.next_connection_id();
                    let previous_connection_id = register_req.previous_connection_id.clone();
                    let session_resumed = !previous_connection_id.is_empty()
                        && state
                            .device_id_for_connection(&previous_connection_id)
                            .as_deref()
                            == Some(dev_id.as_str());

                    let session = crate::state::DeviceSession {
                        device_id: dev_id.clone(),
                        connection_id: connection_id.clone(),
                        metadata: register_req.metadata.clone(),
                        outbound_tx: out_tx.clone(),
                    };

                    if let Some(previous_session) =
                        state.sessions_by_device_id.insert(dev_id.clone(), session)
                    {
                        state
                            .connection_to_device_id
                            .remove(&previous_session.connection_id);
                    }
                    state
                        .connection_to_device_id
                        .insert(connection_id.clone(), dev_id.clone());

                    info!(
                        device_connect_register = true,
                        device_id = %dev_id,
                        connection_id = %connection_id,
                        "device registered"
                    );

                    let resp = relay_message_register_response(connection_id, session_resumed);
                    let _ = out_tx.send(Ok(resp)).await;
                }
                Some(device_message::Payload::Heartbeat(_hb)) => {
                    let resp = relay_message_heartbeat_response();
                    let _ = out_tx.send(Ok(resp)).await;
                }
                Some(device_message::Payload::Data(data_resp)) => {
                    let response_device_id = session_registry
                        .get_device_session(&dev_id)
                        .map(|session| session.device_id)
                        .or_else(|| state.device_id_for_connection(&data_resp.connection_id))
                        .unwrap_or(dev_id.clone());
                    if let Some(inflight) =
                        state.take_inflight(&response_device_id, data_resp.sequence_number)
                    {
                        let resp = device_response_from_device_data(
                            response_device_id,
                            data_resp.sequence_number,
                            data_resp.encrypted_payload.clone(),
                            data_resp.error,
                        );
                        inflight.complete(resp).await;
                    }
                }
                None => {}
            }
        }

        if let Some(ref did) = current_device_id {
            let inflight = state.take_inflight_for_device(did);
            let removed = stream_router.remove_all_for_device(did);

            for (seq, inflight_entry) in inflight {
                inflight_entry
                    .complete(error_resp(did, seq, ErrorCode::DeviceOffline))
                    .await;
            }

            for mapping in removed {
                if mapping.active_requests == 0 {
                    let _ = mapping
                        .controller_tx
                        .send(Ok(error_resp(did, 0, ErrorCode::DeviceOffline)))
                        .await;
                }
            }

            state.remove_device_session(did);
        }
    }

    async fn run_connect_to_device_stream<S>(
        router: StreamRouter,
        cache: IdempotencyCache,
        rate_limiter: RateLimiter,
        session_registry: SessionRegistry,
        state: Arc<RelayState>,
        inflight_timeout: Duration,
        mut inbound: S,
        out_tx: mpsc::Sender<Result<DeviceResponse, Status>>,
    ) where
        S: Stream<Item = Result<ControllerMessage, Status>> + Unpin,
    {
        let mut stream_id: Option<String> = None;
        let mut stream_binding: Option<(String, String, String)> = None;

        while let Some(next_message) = inbound.next().await {
            let Ok(msg) = next_message else {
                break;
            };
            let device_id = &msg.target_device_id;
            let seq = msg.sequence_number;

            if let Err(err) = validator::validate_controller_message(
                &msg.controller_id,
                &msg.target_device_id,
                &msg.method_name,
                &msg.encrypted_payload,
                msg.sequence_number,
            ) {
                send_error_response(&out_tx, device_id, seq, err.code).await;
                continue;
            }

            if !rate_limiter.allow(device_id, &msg.controller_id) {
                send_error_response(&out_tx, device_id, seq, ErrorCode::RateLimited).await;
                continue;
            }

            if stream_id.is_none() {
                match router.create_mapping(
                    msg.target_device_id.clone(),
                    msg.controller_id.clone(),
                    msg.method_name.clone(),
                    out_tx.clone(),
                ) {
                    Ok(sid) => {
                        stream_binding = Some((
                            msg.target_device_id.clone(),
                            msg.controller_id.clone(),
                            msg.method_name.clone(),
                        ));
                        stream_id = Some(sid);
                    }
                    Err(e) if e.kind == StreamRouterErrorKind::MaxStreamsExceeded => {
                        send_error_response(&out_tx, device_id, seq, ErrorCode::RateLimited).await;
                        continue;
                    }
                    Err(_) => {
                        send_error_response(&out_tx, device_id, seq, ErrorCode::InternalError)
                            .await;
                        continue;
                    }
                }
            } else if stream_binding.as_ref()
                != Some(&(
                    msg.target_device_id.clone(),
                    msg.controller_id.clone(),
                    msg.method_name.clone(),
                ))
            {
                send_error_response(&out_tx, device_id, seq, ErrorCode::InternalError).await;
                continue;
            }

            if let Some(ref sid) = stream_id {
                router.begin_request(sid);
            }

            if let Some(cached) = cache.get(device_id, msg.sequence_number).await {
                if let Some(ref sid) = stream_id {
                    router.finish_request(sid);
                }
                let _ = out_tx.send(Ok(cached)).await;
                continue;
            }

            let (rx, is_new_forwarder) = state
                .ensure_inflight_waiter(msg.sequence_number, device_id)
                .await;

            if is_new_forwarder {
                let state_for_timeout = state.clone();
                let t_seq = msg.sequence_number;
                let t_device_id = msg.target_device_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(inflight_timeout).await;
                    if let Some(inflight) = state_for_timeout.take_inflight(&t_device_id, t_seq) {
                        inflight
                            .complete(error_resp(&t_device_id, t_seq, ErrorCode::DeviceOffline))
                            .await;
                    }
                });

                let Some(device_session) = session_registry.get_device_session(device_id) else {
                    if let Some(inflight) = state.take_inflight(device_id, msg.sequence_number) {
                        inflight
                            .complete(error_resp(device_id, seq, ErrorCode::DeviceOffline))
                            .await;
                    }
                    if let Some(ref sid) = stream_id {
                        router.finish_request(sid);
                    }
                    continue;
                };

                let relay_req = relay_message_data_request(
                    device_session.connection_id.clone(),
                    msg.sequence_number,
                    msg.encrypted_payload.clone(),
                );

                if device_session
                    .outbound_tx
                    .send(Ok(relay_req))
                    .await
                    .is_err()
                {
                    if let Some(inflight) = state.take_inflight(device_id, msg.sequence_number) {
                        inflight
                            .complete(error_resp(device_id, seq, ErrorCode::DeviceOffline))
                            .await;
                    }
                    if let Some(ref sid) = stream_id {
                        router.finish_request(sid);
                    }
                    continue;
                }
            }

            if let Ok(resp) = rx.await {
                if is_new_forwarder {
                    cache
                        .insert(device_id, msg.sequence_number, resp.clone())
                        .await;
                }
                if let Some(ref sid) = stream_id {
                    router.finish_request(sid);
                }
                let _ = out_tx.send(Ok(resp)).await;
            } else {
                if let Some(ref sid) = stream_id {
                    router.finish_request(sid);
                }
                send_error_response(&out_tx, device_id, seq, ErrorCode::InternalError).await;
            }
        }

        if let Some(sid) = stream_id {
            router.remove_mapping(&sid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, HealthConfig, IdempotencyConfig, LoggingConfig, ObservabilityConfig,
        RateLimitConfig, RelayConfig, StreamConfig,
    };
    use relay_proto::relay::v1::relay_message;
    use relay_proto::relay::v1::{DataResponse, RegisterRequest};
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    fn test_config() -> AppConfig {
        AppConfig {
            relay: RelayConfig {
                id: "relay-test".into(),
                address: "127.0.0.1:50051".into(),
                quic_address: "127.0.0.1:50052".into(),
                max_device_connections: 1_000,
                heartbeat_interval_seconds: 30,
                stream: StreamConfig {
                    idle_timeout_seconds: 60,
                    max_active_streams: 100,
                },
                rate_limiting: RateLimitConfig {
                    device_requests_per_second: 100,
                    controller_requests_per_second: 100,
                    global_requests_per_second: 1_000,
                },
                idempotency: IdempotencyConfig {
                    cache_capacity: 10_000,
                    cache_ttl_seconds: 3_600,
                },
            },
            observability: ObservabilityConfig {
                logging: LoggingConfig {
                    level: "info".into(),
                    format: "json".into(),
                },
                health: HealthConfig {
                    enabled: false,
                    address: "127.0.0.1:0".into(),
                    path: "/health".into(),
                },
            },
        }
    }

    fn service_with_state(config: AppConfig) -> (RelayGrpcService, Arc<RelayState>) {
        let state = Arc::new(RelayState::new());
        let service = RelayGrpcService::new(state.clone(), &config);
        (service, state)
    }

    async fn start_device_loop(
        service: &RelayGrpcService,
        device_id: &str,
    ) -> (
        mpsc::Sender<Result<DeviceMessage, Status>>,
        mpsc::Receiver<Result<RelayMessage, Status>>,
        String,
    ) {
        let (device_tx, relay_rx, connection_id, _) =
            start_device_loop_with_previous_connection(service, device_id, "").await;
        (device_tx, relay_rx, connection_id)
    }

    async fn start_device_loop_with_previous_connection(
        service: &RelayGrpcService,
        device_id: &str,
        previous_connection_id: &str,
    ) -> (
        mpsc::Sender<Result<DeviceMessage, Status>>,
        mpsc::Receiver<Result<RelayMessage, Status>>,
        String,
        bool,
    ) {
        let (device_tx, device_rx) = mpsc::channel(16);
        let (relay_tx, relay_rx) = mpsc::channel(16);
        let inbound = ReceiverStream::new(device_rx);
        tokio::spawn(RelayGrpcService::run_device_connect_stream(
            service.state.clone(),
            service.session_registry.clone(),
            service.stream_router.clone(),
            inbound,
            relay_tx,
        ));

        device_tx
            .send(Ok(DeviceMessage {
                device_id: device_id.to_string(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Register(RegisterRequest {
                    device_id: device_id.to_string(),
                    metadata: Default::default(),
                    previous_connection_id: previous_connection_id.to_string(),
                })),
            }))
            .await
            .unwrap();

        let mut relay_rx = relay_rx;
        let register = relay_rx.recv().await.unwrap().unwrap();
        let (connection_id, session_resumed) = match register.payload.unwrap() {
            relay_message::Payload::RegisterResponse(resp) => {
                (resp.connection_id, resp.session_resumed)
            }
            other => panic!("unexpected register response: {other:?}"),
        };

        (device_tx, relay_rx, connection_id, session_resumed)
    }

    async fn start_controller_loop(
        service: &RelayGrpcService,
    ) -> (
        mpsc::Sender<Result<ControllerMessage, Status>>,
        mpsc::Receiver<Result<DeviceResponse, Status>>,
    ) {
        let (controller_tx, controller_rx) = mpsc::channel(16);
        let (response_tx, response_rx) = mpsc::channel(16);
        let inbound = ReceiverStream::new(controller_rx);
        tokio::spawn(RelayGrpcService::run_connect_to_device_stream(
            service.stream_router.clone(),
            service.idempotency_cache.clone(),
            service.rate_limiter.clone(),
            service.session_registry.clone(),
            service.state.clone(),
            service.inflight_timeout,
            inbound,
            response_tx,
        ));
        (controller_tx, response_rx)
    }

    fn controller_message(device_id: &str, seq: i64, payload: &[u8]) -> ControllerMessage {
        ControllerMessage {
            controller_id: "ctrl-1".into(),
            token: "controller-token".into(),
            target_device_id: device_id.into(),
            method_name: "svc.Device/Invoke".into(),
            sequence_number: seq,
            encrypted_payload: payload.to_vec(),
        }
    }

    #[tokio::test]
    async fn connect_to_device_relays_response_with_device_id() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        controller_tx
            .send(Ok(controller_message("dev-1", 42, b"hello")))
            .await
            .unwrap();

        let forwarded = device_stream.recv().await.unwrap().unwrap();
        match forwarded.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => {
                assert_eq!(req.connection_id, connection_id);
                assert_eq!(req.sequence_number, 42);
                assert_eq!(req.encrypted_payload, b"hello");
            }
            other => panic!("unexpected relay payload: {other:?}"),
        }

        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id,
                    sequence_number: 42,
                    encrypted_payload: b"world".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();

        let response = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(response.device_id, "dev-1");
        assert_eq!(response.sequence_number, 42);
        assert_eq!(response.encrypted_payload, b"world");
        assert_eq!(response.error, ErrorCode::Ok as i32);
    }

    #[tokio::test]
    async fn duplicate_sequence_returns_cached_response_without_second_forward() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        controller_tx
            .send(Ok(controller_message("dev-1", 7, b"first")))
            .await
            .unwrap();

        let first_forward = device_stream.recv().await.unwrap().unwrap();
        match first_forward.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => {
                assert_eq!(req.sequence_number, 7);
            }
            other => panic!("unexpected relay payload: {other:?}"),
        }

        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id,
                    sequence_number: 7,
                    encrypted_payload: b"done".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();

        let first_response = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(first_response.encrypted_payload, b"done");

        controller_tx
            .send(Ok(controller_message("dev-1", 7, b"first")))
            .await
            .unwrap();

        let second_response = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(second_response.device_id, "dev-1");
        assert_eq!(second_response.sequence_number, 7);
        assert_eq!(second_response.encrypted_payload, b"done");

        let no_second_forward =
            tokio::time::timeout(Duration::from_millis(100), device_stream.recv()).await;
        assert!(no_second_forward.is_err());
    }

    #[tokio::test]
    async fn same_sequence_on_different_devices_forwards_independently() {
        let (service, _state) = service_with_state(test_config());
        let (device1_tx, mut device1_stream, connection1) =
            start_device_loop(&service, "dev-1").await;
        let (device2_tx, mut device2_stream, connection2) =
            start_device_loop(&service, "dev-2").await;
        let (controller1_tx, mut controller1_stream) = start_controller_loop(&service).await;
        let (controller2_tx, mut controller2_stream) = start_controller_loop(&service).await;

        controller1_tx
            .send(Ok(controller_message("dev-1", 11, b"one")))
            .await
            .unwrap();
        controller2_tx
            .send(Ok(controller_message("dev-2", 11, b"two")))
            .await
            .unwrap();

        let forwarded1 = device1_stream.recv().await.unwrap().unwrap();
        let forwarded2 = device2_stream.recv().await.unwrap().unwrap();
        match forwarded1.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => assert_eq!(req.connection_id, connection1),
            other => panic!("unexpected relay payload: {other:?}"),
        }
        match forwarded2.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => assert_eq!(req.connection_id, connection2),
            other => panic!("unexpected relay payload: {other:?}"),
        }

        device1_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id: connection1,
                    sequence_number: 11,
                    encrypted_payload: b"resp-1".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();
        device2_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-2".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id: connection2,
                    sequence_number: 11,
                    encrypted_payload: b"resp-2".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();

        let response1 = controller1_stream.recv().await.unwrap().unwrap();
        let response2 = controller2_stream.recv().await.unwrap().unwrap();
        assert_eq!(response1.device_id, "dev-1");
        assert_eq!(response1.encrypted_payload, b"resp-1");
        assert_eq!(response2.device_id, "dev-2");
        assert_eq!(response2.encrypted_payload, b"resp-2");
    }

    #[tokio::test]
    async fn list_online_devices_reflects_disconnect_cleanup() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, _device_stream, _connection_id) =
            start_device_loop(&service, "dev-1").await;

        let listed = service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token: "controller-token".into(),
                region_filter: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.devices.len(), 1);
        assert_eq!(listed.devices[0].device_id, "dev-1");

        drop(device_tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let listed_after_disconnect = service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token: "controller-token".into(),
                region_filter: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(listed_after_disconnect.devices.is_empty());
    }

    #[tokio::test]
    async fn active_controller_stream_receives_device_offline_on_disconnect() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, _connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        controller_tx
            .send(Ok(controller_message("dev-1", 99, b"ping")))
            .await
            .unwrap();
        let forwarded = device_stream.recv().await.unwrap().unwrap();
        match forwarded.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => assert_eq!(req.sequence_number, 99),
            other => panic!("unexpected relay payload: {other:?}"),
        }

        drop(device_tx);

        let response = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(response.device_id, "dev-1");
        assert_eq!(response.sequence_number, 99);
        assert_eq!(response.error, ErrorCode::DeviceOffline as i32);
    }

    #[tokio::test]
    async fn device_disconnect_notifies_idle_and_inflight_streams() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (idle_controller_tx, mut idle_controller_stream) =
            start_controller_loop(&service).await;
        let (inflight_controller_tx, mut inflight_controller_stream) =
            start_controller_loop(&service).await;

        idle_controller_tx
            .send(Ok(controller_message("dev-1", 1, b"idle")))
            .await
            .unwrap();
        let _ = device_stream.recv().await.unwrap().unwrap();
        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id: connection_id.clone(),
                    sequence_number: 1,
                    encrypted_payload: b"done".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();
        let idle_response = idle_controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(idle_response.error, ErrorCode::Ok as i32);

        inflight_controller_tx
            .send(Ok(controller_message("dev-1", 2, b"wait")))
            .await
            .unwrap();
        let _ = device_stream.recv().await.unwrap().unwrap();

        drop(device_tx);

        let idle_offline = idle_controller_stream.recv().await.unwrap().unwrap();
        let inflight_offline = inflight_controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(idle_offline.sequence_number, 0);
        assert_eq!(idle_offline.error, ErrorCode::DeviceOffline as i32);
        assert_eq!(inflight_offline.sequence_number, 2);
        assert_eq!(inflight_offline.error, ErrorCode::DeviceOffline as i32);
    }

    #[tokio::test]
    async fn controller_stream_rejects_target_changes_after_mapping_created() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        controller_tx
            .send(Ok(controller_message("dev-1", 21, b"first")))
            .await
            .unwrap();
        let _ = device_stream.recv().await.unwrap().unwrap();
        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id,
                    sequence_number: 21,
                    encrypted_payload: b"ok".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();
        let first_response = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(first_response.error, ErrorCode::Ok as i32);

        let mut changed_method = controller_message("dev-1", 22, b"second");
        changed_method.method_name = "svc.Device/Other".into();
        controller_tx.send(Ok(changed_method)).await.unwrap();

        let error = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(error.sequence_number, 22);
        assert_eq!(error.error, ErrorCode::InternalError as i32);

        let no_second_forward =
            tokio::time::timeout(Duration::from_millis(100), device_stream.recv()).await;
        assert!(no_second_forward.is_err());
    }

    #[tokio::test]
    async fn register_reports_resume_and_replaces_previous_connection_mapping() {
        let (service, state) = service_with_state(test_config());
        let (first_device_tx, _first_stream, first_connection_id, first_resumed) =
            start_device_loop_with_previous_connection(&service, "dev-1", "").await;
        assert!(!first_resumed);
        assert_eq!(
            state
                .device_id_for_connection(&first_connection_id)
                .as_deref(),
            Some("dev-1")
        );

        let (_second_device_tx, _second_stream, second_connection_id, second_resumed) =
            start_device_loop_with_previous_connection(&service, "dev-1", &first_connection_id)
                .await;
        assert!(second_resumed);
        assert!(state
            .device_id_for_connection(&first_connection_id)
            .is_none());
        assert_eq!(
            state
                .device_id_for_connection(&second_connection_id)
                .as_deref(),
            Some("dev-1")
        );

        drop(first_device_tx);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_error_response(
    tx: &mpsc::Sender<Result<DeviceResponse, Status>>,
    device_id: &str,
    seq: i64,
    err: ErrorCode,
) {
    let _ = tx
        .send(Ok(DeviceResponse {
            device_id: device_id.to_string(),
            sequence_number: seq,
            encrypted_payload: Vec::new(),
            error: err as i32,
        }))
        .await;
}

fn error_resp(device_id: &str, seq: i64, err: ErrorCode) -> DeviceResponse {
    DeviceResponse {
        device_id: device_id.to_string(),
        sequence_number: seq,
        encrypted_payload: Vec::new(),
        error: err as i32,
    }
}

// ---------------------------------------------------------------------------
// RelayService implementation
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl RelayService for RelayGrpcService {
    type DeviceConnectStream = ReceiverStream<std::result::Result<RelayMessage, Status>>;
    type ConnectToDeviceStream = ReceiverStream<std::result::Result<DeviceResponse, Status>>;

    async fn device_connect(
        &self,
        request: Request<tonic::Streaming<DeviceMessage>>,
    ) -> std::result::Result<Response<Self::DeviceConnectStream>, Status> {
        let inbound = request.into_inner();

        let (out_tx, out_rx) = mpsc::channel::<std::result::Result<RelayMessage, Status>>(64);
        let out_stream = ReceiverStream::new(out_rx);

        let state = self.state.clone();
        let session_registry = self.session_registry.clone();
        let stream_router = self.stream_router.clone();
        tokio::spawn(async move {
            Self::run_device_connect_stream(
                state,
                session_registry,
                stream_router,
                inbound,
                out_tx,
            )
            .await;
        });

        Ok(Response::new(out_stream))
    }

    async fn list_online_devices(
        &self,
        _request: Request<ListOnlineDevicesRequest>,
    ) -> std::result::Result<Response<ListOnlineDevicesResponse>, Status> {
        let devices = self
            .session_registry
            .list_online_devices(&self.relay_address);

        info!(
            list_online_devices_called = true,
            count = devices.len(),
            "online devices list built"
        );

        Ok(Response::new(ListOnlineDevicesResponse { devices }))
    }

    async fn connect_to_device(
        &self,
        request: Request<tonic::Streaming<ControllerMessage>>,
    ) -> std::result::Result<Response<Self::ConnectToDeviceStream>, Status> {
        let inbound = request.into_inner();

        let (out_tx, out_rx) = mpsc::channel::<std::result::Result<DeviceResponse, Status>>(64);
        let out_stream = ReceiverStream::new(out_rx);

        let router = self.stream_router.clone();
        let cache = self.idempotency_cache.clone();
        let rate_limiter = self.rate_limiter.clone();
        let session_registry = self.session_registry.clone();
        let state = self.state.clone();
        let inflight_timeout = self.inflight_timeout;

        tokio::spawn(async move {
            Self::run_connect_to_device_stream(
                router,
                cache,
                rate_limiter,
                session_registry,
                state,
                inflight_timeout,
                inbound,
                out_tx,
            )
            .await;
        });

        Ok(Response::new(out_stream))
    }
}
