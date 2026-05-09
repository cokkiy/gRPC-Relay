use crate::auth::AuthService;
use crate::config::AppConfig;
use crate::idempotency::IdempotencyCache;
use crate::rate_limiter::{BandwidthTracker, ConnectionRateLimiter, RateLimiter};
use crate::rbac::{AuthorizationError, RbacPolicyEngine};
use crate::resource_monitor::ResourceMonitor;
use crate::security_metrics::SecurityMetrics;
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
    ListOnlineDevicesRequest, ListOnlineDevicesResponse, RelayMessage, RevokeTokenRequest,
    RevokeTokenResponse, TokenTargetType,
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
    connection_limiter: ConnectionRateLimiter,
    bandwidth_tracker: BandwidthTracker,
    stream_router: StreamRouter,
    session_registry: SessionRegistry,
    inflight_timeout: Duration,
    auth_service: AuthService,
    rbac: RbacPolicyEngine,
    security_metrics: SecurityMetrics,
    resource_monitor: ResourceMonitor,
}

impl RelayGrpcService {
    pub fn new(
        state: Arc<RelayState>,
        config: &AppConfig,
        security_metrics: SecurityMetrics,
        resource_monitor: ResourceMonitor,
    ) -> Self {
        let auth_service = AuthService::new(&config.relay.auth);
        let rbac = RbacPolicyEngine::new(&config.relay.auth);

        Self {
            idempotency_cache: IdempotencyCache::new(
                config.relay.idempotency.cache_capacity,
                config.relay.idempotency.cache_ttl_seconds,
            ),
            rate_limiter: RateLimiter::new(&config.relay.rate_limiting),
            connection_limiter: ConnectionRateLimiter::new(&config.relay.rate_limiting),
            bandwidth_tracker: BandwidthTracker::new(&config.relay.rate_limiting),
            stream_router: StreamRouter::new(&config.relay.stream),
            session_registry: SessionRegistry::new(state.clone()),
            inflight_timeout: Duration::from_secs(60),
            state,
            relay_address: config.relay.address.clone(),
            auth_service,
            rbac,
            security_metrics,
            resource_monitor,
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
        _session_registry: SessionRegistry,
        stream_router: StreamRouter,
        auth_service: AuthService,
        security_metrics: SecurityMetrics,
        connection_limiter: ConnectionRateLimiter,
        resource_monitor: ResourceMonitor,
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
            let device_token = dev_msg.token.clone();
            let is_register_message =
                matches!(dev_msg.payload, Some(device_message::Payload::Register(_)));
            if !is_register_message {
                match current_device_id.as_deref() {
                    Some(current_id) if current_id == dev_id.as_str() => {}
                    Some(current_id) => {
                        tracing::info!(
                            event = "auth_failure",
                            actor_type = "device",
                            device_id = %dev_id,
                            expected_device_id = %current_id,
                            token_prefix = %AuthService::token_prefix(&device_token),
                            reason = "device_id_mismatch_on_stream"
                        );
                        break;
                    }
                    None => {
                        tracing::info!(
                            event = "auth_failure",
                            actor_type = "device",
                            device_id = %dev_id,
                            token_prefix = %AuthService::token_prefix(&device_token),
                            reason = "non_register_before_registration"
                        );
                        break;
                    }
                }
            }

            match dev_msg.payload {
                Some(device_message::Payload::Register(register_req)) => {
                    // Device authentication (MVP token-based)
                    match auth_service.authenticate_device_by_token(&dev_id, &device_token) {
                        Ok(_principal) => {
                            security_metrics.record_auth_success();
                            tracing::info!(
                                event = "auth_success",
                                actor_type = "device",
                                device_id = %dev_id,
                                token_prefix = %AuthService::token_prefix(&device_token),
                                reason = "device_token_ok"
                            );
                            current_device_id = Some(dev_id.clone());
                        }
                        Err(_) => {
                            security_metrics.record_auth_failure();
                            tracing::info!(
                                event = "auth_failure",
                                actor_type = "device",
                                device_id = %dev_id,
                                token_prefix = %AuthService::token_prefix(&device_token),
                                reason = "invalid_device_token"
                            );
                            break;
                        }
                    }

                    // Per-device connection rate limiting
                    if !connection_limiter.allow_device(&dev_id) {
                        security_metrics.record_rate_limit();
                        tracing::info!(
                            event = "rate_limit",
                            actor_type = "device",
                            device_id = %dev_id,
                            reason = "connection_limit_exceeded"
                        );
                        break;
                    }

                    // System resource health check
                    if !resource_monitor.is_healthy() {
                        tracing::warn!(
                            event = "rate_limit",
                            actor_type = "device",
                            device_id = %dev_id,
                            reason = "resource_unhealthy"
                        );
                        break;
                    }

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
                    // Heartbeat authentication (MVP: re-check token matches device_id)
                    if auth_service
                        .authenticate_device_by_token(&dev_id, &device_token)
                        .is_err()
                    {
                        tracing::info!(
                            event = "auth_failure",
                            actor_type = "device",
                            device_id = %dev_id,
                            token_prefix = %AuthService::token_prefix(&device_token),
                            reason = "invalid_device_token_on_heartbeat"
                        );
                        break;
                    } else {
                        security_metrics.record_auth_success();
                        tracing::info!(
                            event = "auth_success",
                            actor_type = "device",
                            device_id = %dev_id,
                            token_prefix = %AuthService::token_prefix(&device_token),
                            reason = "device_token_ok_on_heartbeat"
                        );
                    }

                    let resp = relay_message_heartbeat_response();
                    let _ = out_tx.send(Ok(resp)).await;
                }
                Some(device_message::Payload::Data(data_resp)) => {
                    let Some(response_device_id) = current_device_id.as_ref().cloned() else {
                        tracing::info!(
                            event = "auth_failure",
                            actor_type = "device",
                            device_id = %dev_id,
                            token_prefix = %AuthService::token_prefix(&device_token),
                            reason = "data_before_registration"
                        );
                        break;
                    };
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
        bandwidth_tracker: BandwidthTracker,
        session_registry: SessionRegistry,
        state: Arc<RelayState>,
        inflight_timeout: Duration,
        auth_service: AuthService,
        rbac: RbacPolicyEngine,
        security_metrics: SecurityMetrics,
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

            if msg.token.trim().is_empty() {
                security_metrics.record_auth_failure();
                tracing::info!(
                    event = "auth_failure",
                    actor_type = "controller",
                    controller_id = %msg.controller_id,
                    device_id = %msg.target_device_id,
                    token_prefix = "",
                    reason = "token_missing"
                );
                send_error_response(&out_tx, device_id, seq, ErrorCode::Unauthorized).await;
                continue;
            }

            // MVP: 认证/授权在“创建 stream mapping”时检查一次（避免每条消息重复开销）
            if stream_id.is_none() {
                tracing::info!(
                    event = "controller_request",
                    actor_type = "controller",
                    controller_id = %msg.controller_id,
                    device_id = %msg.target_device_id,
                    method_name = %msg.method_name,
                    sequence_number = %msg.sequence_number,
                    token_prefix = %AuthService::token_prefix(&msg.token),
                    stage = "auth_verify"
                );

                let controller = match auth_service
                    .authenticate_controller(&msg.controller_id, &msg.token)
                {
                    Ok(p) => {
                        security_metrics.record_auth_success();
                        tracing::info!(
                            event = "auth_success",
                            actor_type = "controller",
                            controller_id = %msg.controller_id,
                            device_id = %msg.target_device_id,
                            role = %p.role,
                            token_prefix = %AuthService::token_prefix(&msg.token),
                            reason = "controller_token_ok"
                        );
                        p
                    }
                    Err(_) => {
                        security_metrics.record_auth_failure();
                        tracing::info!(
                            event = "auth_failure",
                            actor_type = "controller",
                            controller_id = %msg.controller_id,
                            device_id = %msg.target_device_id,
                            token_prefix = %AuthService::token_prefix(&msg.token),
                            reason = "invalid_controller_token"
                        );
                        send_error_response(&out_tx, device_id, seq, ErrorCode::Unauthorized).await;
                        continue;
                    }
                };

                let device = match auth_service.get_device_principal_by_id(device_id) {
                    Ok(p) => p,
                    Err(_) => {
                        send_error_response(&out_tx, device_id, seq, ErrorCode::DeviceNotFound)
                            .await;
                        continue;
                    }
                };

                if let Err(
                    AuthorizationError::DeviceProjectForbidden
                    | AuthorizationError::MethodNotAllowed,
                ) = rbac.authorize_controller_to_device(&controller, &device, &msg.method_name)
                {
                    security_metrics.record_authorization_denied();
                    tracing::info!(
                        event = "authorization_denied",
                        actor_type = "controller",
                        controller_id = %msg.controller_id,
                        device_id = %msg.target_device_id,
                        method_name = %msg.method_name,
                        token_prefix = %AuthService::token_prefix(&msg.token),
                        reason = "rbac_or_method_denied"
                    );
                    send_error_response(&out_tx, device_id, seq, ErrorCode::Unauthorized).await;
                    continue;
                }

                tracing::info!(
                    event = "controller_request_to_device",
                    actor_type = "controller",
                    controller_id = %msg.controller_id,
                    device_id = %msg.target_device_id,
                    method_name = %msg.method_name,
                    sequence_number = %msg.sequence_number,
                    token_prefix = %AuthService::token_prefix(&msg.token),
                    stage = "permission_check"
                );

                if !rate_limiter.allow(device_id, &msg.controller_id) {
                    security_metrics.record_rate_limit();
                    tracing::info!(
                        event = "rate_limit",
                        actor_type = "controller",
                        controller_id = %msg.controller_id,
                        device_id = %msg.target_device_id,
                        method_name = %msg.method_name,
                        token_prefix = %AuthService::token_prefix(&msg.token)
                    );
                    send_error_response(&out_tx, device_id, seq, ErrorCode::RateLimited).await;
                    continue;
                }

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

                // Bandwidth check before forwarding
                if !bandwidth_tracker.record_and_check(
                    device_id,
                    &msg.controller_id,
                    msg.encrypted_payload.len() as u64,
                ) {
                    security_metrics.record_rate_limit();
                    tracing::info!(
                        event = "rate_limit",
                        actor_type = "controller",
                        controller_id = %msg.controller_id,
                        device_id = %msg.target_device_id,
                        method_name = %msg.method_name,
                        token_prefix = %AuthService::token_prefix(&msg.token),
                        reason = "bandwidth_exceeded"
                    );
                    send_error_response(&out_tx, device_id, seq, ErrorCode::RateLimited).await;
                    if let Some(ref sid) = stream_id {
                        router.finish_request(sid);
                    }
                    continue;
                }

                tracing::info!(
                    event = "controller_request_to_device",
                    actor_type = "controller",
                    controller_id = %msg.controller_id,
                    device_id = %msg.target_device_id,
                    method_name = %msg.method_name,
                    sequence_number = %msg.sequence_number,
                    stage = "relay_forward"
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
                tracing::info!(
                    event = "controller_request_to_device",
                    actor_type = "controller",
                    controller_id = %msg.controller_id,
                    device_id = %msg.target_device_id,
                    method_name = %msg.method_name,
                    sequence_number = %msg.sequence_number,
                    stage = "relay_response"
                );

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
    use crate::auth::ControllerClaims;
    use crate::config::{
        AppConfig, AuthConfig, HealthConfig, IdempotencyConfig, JwtConfig, LoggingConfig,
        ObservabilityConfig, RateLimitConfig, RelayConfig, StreamConfig,
    };
    use jsonwebtoken::{encode, EncodingKey, Header};
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
                    max_concurrent_streams_per_controller: 100,
                },
                rate_limiting: RateLimitConfig {
                    device_requests_per_second: 100,
                    controller_requests_per_minute: 60_000,
                    global_requests_per_second: 1_000,
                    device_connection_per_minute: 10,
                    global_connections_per_second: 100,
                    device_bandwidth_bytes_per_sec: 10 * 1024 * 1024,
                    controller_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
                    global_bandwidth_bytes_per_sec: 100 * 1024 * 1024,
                    cpu_threshold_percent: 80.0,
                    memory_threshold_mb: 12 * 1024,
                },
                idempotency: IdempotencyConfig {
                    cache_capacity: 10_000,
                    cache_ttl_seconds: 3_600,
                },
                auth: Default::default(),
                tls: Default::default(),
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
        let service = RelayGrpcService::new(
            state.clone(),
            &config,
            crate::security_metrics::SecurityMetrics::default(),
            crate::resource_monitor::ResourceMonitor::new(&config.relay.rate_limiting),
        );
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
            service.auth_service.clone(),
            service.security_metrics.clone(),
            service.connection_limiter.clone(),
            service.resource_monitor.clone(),
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
            service.bandwidth_tracker.clone(),
            service.session_registry.clone(),
            service.state.clone(),
            service.inflight_timeout,
            service.auth_service.clone(),
            service.rbac.clone(),
            service.security_metrics.clone(),
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
    async fn device_stream_rejects_non_register_device_id_mismatch() {
        let (service, _state) = service_with_state(test_config());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop(&service, "dev-1").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        controller_tx
            .send(Ok(controller_message("dev-1", 42, b"hello")))
            .await
            .unwrap();
        let _ = device_stream.recv().await.unwrap().unwrap();

        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-2".into(),
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
        assert_eq!(response.error, ErrorCode::DeviceOffline as i32);
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

    fn test_config_with_auth_enabled() -> AppConfig {
        use crate::config::{ControllerAuthEntry, DeviceAuthEntry};

        let mut controller_tokens = std::collections::HashMap::new();
        controller_tokens.insert(
            "controller-token".to_string(),
            ControllerAuthEntry {
                controller_id: "ctrl-1".to_string(),
                role: "operator".to_string(),
                allowed_project_ids: vec!["proj-1".to_string()],
            },
        );

        let mut device_tokens = std::collections::HashMap::new();
        device_tokens.insert(
            "device-token".to_string(),
            DeviceAuthEntry {
                device_id: "dev-1".to_string(),
                project_id: "proj-1".to_string(),
            },
        );

        // dev-2 也需要配置，才能通过 device_principal 查询
        device_tokens.insert(
            "device-token-dev2".to_string(),
            DeviceAuthEntry {
                device_id: "dev-2".to_string(),
                project_id: "proj-2".to_string(),
            },
        );

        AppConfig {
            relay: RelayConfig {
                id: "relay-test".into(),
                address: "127.0.0.1:50051".into(),
                quic_address: "127.0.1:50052".into(),
                max_device_connections: 1_000,
                heartbeat_interval_seconds: 30,
                stream: StreamConfig {
                    idle_timeout_seconds: 60,
                    max_active_streams: 100,
                    max_concurrent_streams_per_controller: 100,
                },
                rate_limiting: RateLimitConfig {
                    device_requests_per_second: 100,
                    controller_requests_per_minute: 60_000,
                    global_requests_per_second: 1_000,
                    ..Default::default()
                },
                idempotency: IdempotencyConfig {
                    cache_capacity: 10_000,
                    cache_ttl_seconds: 3_600,
                },
                auth: AuthConfig {
                    enabled: true,
                    controller_tokens,
                    device_tokens,
                    method_whitelist: vec!["svc.Device/Invoke".to_string()],
                    jwt: JwtConfig {
                        enabled: true,
                        hs256_secret: "test-secret".to_string(),
                        issuer: Some("grpc-relay-test".to_string()),
                        audience: Some("controller-test".to_string()),
                        clock_skew_seconds: 30,
                    },
                },
                tls: Default::default(),
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

    fn controller_jwt(role: &str, allowed_project_ids: Vec<&str>) -> String {
        let claims = ControllerClaims {
            sub: "ctrl-1".to_string(),
            controller_id: "ctrl-1".to_string(),
            role: role.to_string(),
            allowed_project_ids: allowed_project_ids
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            exp: 4_102_444_800,
            iss: Some("grpc-relay-test".to_string()),
            aud: Some("controller-test".to_string()),
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap()
    }

    fn operator_jwt() -> String {
        controller_jwt("operator", vec!["proj-1"])
    }

    fn admin_jwt() -> String {
        controller_jwt("admin", Vec::new())
    }

    fn controller_message_with_token(
        device_id: &str,
        seq: i64,
        token: &str,
        method_name: &str,
    ) -> ControllerMessage {
        ControllerMessage {
            controller_id: "ctrl-1".into(),
            token: token.into(),
            target_device_id: device_id.into(),
            method_name: method_name.to_string(),
            sequence_number: seq,
            encrypted_payload: b"payload".to_vec(),
        }
    }

    async fn start_device_loop_with_token(
        service: &RelayGrpcService,
        device_id: &str,
        token: &str,
    ) -> (
        mpsc::Sender<Result<DeviceMessage, Status>>,
        mpsc::Receiver<Result<RelayMessage, Status>>,
        String,
    ) {
        let (device_tx, device_rx) = mpsc::channel(16);
        let (relay_tx, relay_rx) = mpsc::channel(16);
        let inbound = ReceiverStream::new(device_rx);
        tokio::spawn(RelayGrpcService::run_device_connect_stream(
            service.state.clone(),
            service.session_registry.clone(),
            service.stream_router.clone(),
            service.auth_service.clone(),
            service.security_metrics.clone(),
            service.connection_limiter.clone(),
            service.resource_monitor.clone(),
            inbound,
            relay_tx,
        ));

        device_tx
            .send(Ok(DeviceMessage {
                device_id: device_id.to_string(),
                token: token.to_string(),
                payload: Some(device_message::Payload::Register(RegisterRequest {
                    device_id: device_id.to_string(),
                    metadata: Default::default(),
                    previous_connection_id: "".to_string(),
                })),
            }))
            .await
            .unwrap();

        let mut relay_rx = relay_rx;
        let register = relay_rx.recv().await.unwrap().unwrap();
        let (connection_id, _session_resumed) = match register.payload.unwrap() {
            relay_message::Payload::RegisterResponse(resp) => {
                (resp.connection_id, resp.session_resumed)
            }
            other => panic!("unexpected register response: {other:?}"),
        };

        (device_tx, relay_rx, connection_id)
    }

    #[tokio::test]
    async fn connect_to_device_rejects_invalid_controller_token() {
        let (service, _state) = service_with_state(test_config_with_auth_enabled());
        let (device_tx, _device_stream, _connection_id) =
            start_device_loop_with_token(&service, "dev-1", "device-token").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;

        // invalid controller token
        controller_tx
            .send(Ok(controller_message_with_token(
                "dev-1",
                1,
                "bad-token",
                "svc.Device/Invoke",
            )))
            .await
            .unwrap();

        let resp = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(resp.device_id, "dev-1");
        assert_eq!(resp.sequence_number, 1);
        assert_eq!(resp.error, ErrorCode::Unauthorized as i32);

        drop(device_tx);
    }

    #[tokio::test]
    async fn connect_to_device_rejects_method_not_in_whitelist() {
        let mut cfg = test_config_with_auth_enabled();
        cfg.relay.auth.method_whitelist = vec!["svc.Device/Other".to_string()];

        let (service, _state) = service_with_state(cfg);
        let (device_tx, _device_stream, _connection_id) =
            start_device_loop_with_token(&service, "dev-1", "device-token").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;
        let token = operator_jwt();

        controller_tx
            .send(Ok(controller_message_with_token(
                "dev-1",
                1,
                &token,
                "svc.Device/Invoke",
            )))
            .await
            .unwrap();

        let resp = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(resp.device_id, "dev-1");
        assert_eq!(resp.sequence_number, 1);
        assert_eq!(resp.error, ErrorCode::Unauthorized as i32);

        drop(device_tx);
    }

    #[tokio::test]
    async fn connect_to_device_rejects_project_forbidden() {
        let mut cfg = test_config_with_auth_enabled();
        // operator can only access proj-2, but dev-1 is proj-1
        if let Some(entry) = cfg.relay.auth.controller_tokens.get_mut("controller-token") {
            entry.allowed_project_ids = vec!["proj-2".to_string()];
        }

        let (service, _state) = service_with_state(cfg);
        let (device_tx, _device_stream, _connection_id) =
            start_device_loop_with_token(&service, "dev-1", "device-token").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;
        let token = controller_jwt("operator", vec!["proj-2"]);

        controller_tx
            .send(Ok(controller_message_with_token(
                "dev-1",
                1,
                &token,
                "svc.Device/Invoke",
            )))
            .await
            .unwrap();

        let resp = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(resp.device_id, "dev-1");
        assert_eq!(resp.sequence_number, 1);
        assert_eq!(resp.error, ErrorCode::Unauthorized as i32);

        drop(device_tx);
    }

    #[tokio::test]
    async fn list_online_devices_filters_by_project_for_non_admin() {
        let (service, _state) = service_with_state(test_config_with_auth_enabled());
        // dev-1 => proj-1, dev-2 => proj-2, controller operator => allowed_project_ids: proj-1 only

        let (_dev1_tx, _dev1_stream, _conn1) =
            start_device_loop_with_token(&service, "dev-1", "device-token").await;
        let (_dev2_tx, _dev2_stream, _conn2) =
            start_device_loop_with_token(&service, "dev-2", "device-token-dev2").await;
        let token = operator_jwt();

        let resp = service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token,
                region_filter: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.devices.len(), 1);
        assert_eq!(resp.devices[0].device_id, "dev-1");
    }

    #[tokio::test]
    async fn connect_to_device_accepts_valid_controller_jwt() {
        let (service, _state) = service_with_state(test_config_with_auth_enabled());
        let (device_tx, mut device_stream, connection_id) =
            start_device_loop_with_token(&service, "dev-1", "device-token").await;
        let (controller_tx, mut controller_stream) = start_controller_loop(&service).await;
        let token = operator_jwt();

        controller_tx
            .send(Ok(controller_message_with_token(
                "dev-1",
                31,
                &token,
                "svc.Device/Invoke",
            )))
            .await
            .unwrap();

        let forwarded = device_stream.recv().await.unwrap().unwrap();
        match forwarded.payload.unwrap() {
            relay_message::Payload::DataRequest(req) => {
                assert_eq!(req.sequence_number, 31);
            }
            other => panic!("unexpected relay payload: {other:?}"),
        }

        device_tx
            .send(Ok(DeviceMessage {
                device_id: "dev-1".into(),
                token: "device-token".into(),
                payload: Some(device_message::Payload::Data(DataResponse {
                    connection_id,
                    sequence_number: 31,
                    encrypted_payload: b"jwt-ok".to_vec(),
                    error: ErrorCode::Ok as i32,
                })),
            }))
            .await
            .unwrap();

        let resp = controller_stream.recv().await.unwrap().unwrap();
        assert_eq!(resp.error, ErrorCode::Ok as i32);
        assert_eq!(resp.encrypted_payload, b"jwt-ok");
    }

    #[tokio::test]
    async fn list_online_devices_returns_resource_exhausted_when_limited() {
        let mut cfg = test_config_with_auth_enabled();
        cfg.relay.rate_limiting.controller_requests_per_minute = 1;
        cfg.relay.rate_limiting.global_requests_per_second = 100;
        let (service, _state) = service_with_state(cfg);
        let token = operator_jwt();

        service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token: token.clone(),
                region_filter: String::new(),
            }))
            .await
            .unwrap();

        let err = service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token,
                region_filter: String::new(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::ResourceExhausted);
    }

    #[tokio::test]
    async fn admin_can_revoke_controller_token_and_metrics_increment() {
        let (service, _state) = service_with_state(test_config_with_auth_enabled());
        let admin_token = admin_jwt();
        let operator_token = operator_jwt();

        let response = service
            .revoke_token(Request::new(RevokeTokenRequest {
                controller_id: "ctrl-1".into(),
                admin_token,
                target_type: TokenTargetType::Controller as i32,
                target_token_hash_or_prefix: operator_token.chars().take(16).collect(),
                reason: "test".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(response.revoked);

        let err = service
            .list_online_devices(Request::new(ListOnlineDevicesRequest {
                controller_id: "ctrl-1".into(),
                token: operator_token,
                region_filter: String::new(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);

        let snapshot = service.security_metrics.snapshot();
        assert_eq!(snapshot.revoked_tokens_total, 1);
        assert!(snapshot.auth_failure_total >= 1);
    }

    #[tokio::test]
    async fn non_admin_cannot_revoke_token() {
        let (service, _state) = service_with_state(test_config_with_auth_enabled());
        let operator_token = operator_jwt();

        let err = service
            .revoke_token(Request::new(RevokeTokenRequest {
                controller_id: "ctrl-1".into(),
                admin_token: operator_token,
                target_type: TokenTargetType::Controller as i32,
                target_token_hash_or_prefix: "anything".into(),
                reason: "test".into(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::PermissionDenied);
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
        // Global connection rate limit
        if !self.connection_limiter.allow_global().await {
            self.security_metrics.record_rate_limit();
            tracing::info!(
                event = "rate_limit",
                actor_type = "system",
                reason = "global_connection_limit_exceeded"
            );
            return Err(Status::resource_exhausted(
                "global connection rate limit exceeded",
            ));
        }

        // System resource health check
        if !self.resource_monitor.is_healthy() {
            self.security_metrics.record_rate_limit();
            tracing::info!(
                event = "rate_limit",
                actor_type = "system",
                reason = "resource_unhealthy"
            );
            return Err(Status::resource_exhausted("system overloaded"));
        }

        let inbound = request.into_inner();

        let (out_tx, out_rx) = mpsc::channel::<std::result::Result<RelayMessage, Status>>(64);
        let out_stream = ReceiverStream::new(out_rx);

        let state = self.state.clone();
        let session_registry = self.session_registry.clone();
        let stream_router = self.stream_router.clone();
        let auth_service = self.auth_service.clone();
        let security_metrics = self.security_metrics.clone();
        let connection_limiter = self.connection_limiter.clone();
        let resource_monitor = self.resource_monitor.clone();
        tokio::spawn(async move {
            Self::run_device_connect_stream(
                state,
                session_registry,
                stream_router,
                auth_service,
                security_metrics,
                connection_limiter,
                resource_monitor,
                inbound,
                out_tx,
            )
            .await;
        });

        Ok(Response::new(out_stream))
    }

    async fn list_online_devices(
        &self,
        request: Request<ListOnlineDevicesRequest>,
    ) -> std::result::Result<Response<ListOnlineDevicesResponse>, Status> {
        let req = request.into_inner();

        if req.controller_id.trim().is_empty() || req.token.trim().is_empty() {
            self.security_metrics.record_auth_failure();
            return Err(Status::unauthenticated("missing controller_id or token"));
        }

        let principal = self
            .auth_service
            .authenticate_controller(&req.controller_id, &req.token)
            .map_err(|_| {
                self.security_metrics.record_auth_failure();
                Status::unauthenticated("invalid controller token")
            })?;
        self.security_metrics.record_auth_success();

        if !self.rate_limiter.allow("*", &req.controller_id) {
            self.security_metrics.record_rate_limit();
            tracing::info!(
                event = "rate_limit",
                actor_type = "controller",
                controller_id = %req.controller_id,
                device_id = "*",
                token_prefix = %AuthService::token_prefix(&req.token)
            );
            return Err(Status::resource_exhausted("rate limited"));
        }

        let all_devices = self
            .session_registry
            .list_online_devices(&self.relay_address);

        // Role-based filtering
        let devices = all_devices
            .into_iter()
            .filter(|d| {
                // region filter (MVP: match metadata["region"])
                if !req.region_filter.trim().is_empty() {
                    let region = d.metadata.get("region").map(|s| s.as_str()).unwrap_or("");
                    if region != req.region_filter.as_str() {
                        return false;
                    }
                }

                if principal.role == "admin" {
                    return true;
                }

                // Controller allowed projects
                // (If a device is not in AuthConfig, skip it)
                let device_principal = self
                    .auth_service
                    .get_device_principal_by_id(&d.device_id)
                    .ok();

                let device_principal = match device_principal {
                    Some(p) => p,
                    None => return false,
                };

                principal
                    .allowed_project_ids
                    .iter()
                    .any(|pid| pid == &device_principal.project_id)
            })
            .collect::<Vec<_>>();

        // If controller is non-admin but has no allowed projects, treat as permission denied (MVP)
        if principal.role != "admin" && principal.allowed_project_ids.is_empty() {
            self.security_metrics.record_authorization_denied();
            return Err(Status::permission_denied("no project permissions"));
        }

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
        let bandwidth_tracker = self.bandwidth_tracker.clone();
        let session_registry = self.session_registry.clone();
        let state = self.state.clone();
        let inflight_timeout = self.inflight_timeout;

        let auth_service = self.auth_service.clone();
        let rbac = self.rbac.clone();
        let security_metrics = self.security_metrics.clone();

        tokio::spawn(async move {
            Self::run_connect_to_device_stream(
                router,
                cache,
                rate_limiter,
                bandwidth_tracker,
                session_registry,
                state,
                inflight_timeout,
                auth_service,
                rbac,
                security_metrics,
                inbound,
                out_tx,
            )
            .await;
        });

        Ok(Response::new(out_stream))
    }

    async fn revoke_token(
        &self,
        request: Request<RevokeTokenRequest>,
    ) -> std::result::Result<Response<RevokeTokenResponse>, Status> {
        let req = request.into_inner();

        if req.controller_id.trim().is_empty() || req.admin_token.trim().is_empty() {
            self.security_metrics.record_auth_failure();
            return Err(Status::unauthenticated("missing controller_id or token"));
        }
        if req.target_token_hash_or_prefix.trim().is_empty() {
            return Err(Status::invalid_argument(
                "missing target_token_hash_or_prefix",
            ));
        }

        let principal = self
            .auth_service
            .authenticate_controller(&req.controller_id, &req.admin_token)
            .map_err(|_| {
                self.security_metrics.record_auth_failure();
                Status::unauthenticated("invalid controller token")
            })?;
        self.security_metrics.record_auth_success();

        if principal.role != "admin" {
            self.security_metrics.record_authorization_denied();
            tracing::info!(
                event = "authorization_denied",
                actor_type = "controller",
                controller_id = %req.controller_id,
                reason = "revoke_requires_admin"
            );
            return Err(Status::permission_denied("admin role required"));
        }

        match TokenTargetType::try_from(req.target_type) {
            Ok(TokenTargetType::Controller) => self
                .auth_service
                .revoke_controller_token(&req.target_token_hash_or_prefix),
            Ok(TokenTargetType::Device) => self
                .auth_service
                .revoke_device_token(&req.target_token_hash_or_prefix),
            _ => return Err(Status::invalid_argument("invalid target_type")),
        }

        let logged_token_prefix = req
            .target_token_hash_or_prefix
            .chars()
            .take(8)
            .collect::<String>();

        self.security_metrics.record_revoked_token();
        tracing::info!(
            event = "token_revoked",
            actor_type = "controller",
            controller_id = %req.controller_id,
            target_type = req.target_type,
            token_prefix = %logged_token_prefix,
            reason = %req.reason
        );

        Ok(Response::new(RevokeTokenResponse { revoked: true }))
    }
}
