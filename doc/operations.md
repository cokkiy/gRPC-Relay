# gRPC-Relay 运维操作手册

> 说明：`deploy/kubernetes/` 通过 Kustomize 统一加上 `grpc-relay-` 前缀。以下命令默认使用渲染后的资源名，例如 `deployment/grpc-relay-relay`、`svc/grpc-relay-relay`、`hpa/grpc-relay-relay-hpa`。

## 1. 启动服务

### 裸机 / systemd 方式

```bash
# 安装二进制、配置、systemd unit，并启动服务
sudo ./deploy/bare-metal/install.sh

# 验证健康检查
systemctl status relay
curl http://127.0.0.1:8080/health
```

### Docker Compose 方式

```bash
# 启动全部服务（Relay + MQTT + Prometheus + Grafana + Jaeger）
docker-compose up -d

# 仅启动 Relay 和 MQTT
docker-compose up -d relay mqtt

# 查看启动状态
docker-compose ps

# 验证健康检查
curl http://localhost:8080/health
```

### Kubernetes 方式

```bash
# 通过 Kustomize 部署
kubectl apply -k deploy/kubernetes/

# 或逐文件部署
kubectl apply -f deploy/kubernetes/namespace.yaml
kubectl apply -f deploy/kubernetes/serviceaccount.yaml
kubectl apply -f deploy/kubernetes/configmap.yaml
kubectl apply -f deploy/kubernetes/secret.yaml
kubectl apply -f deploy/kubernetes/deployment.yaml
kubectl apply -f deploy/kubernetes/service.yaml
kubectl apply -f deploy/kubernetes/hpa.yaml
kubectl apply -f deploy/kubernetes/pdb.yaml
kubectl apply -f deploy/kubernetes/networkpolicy.yaml

# 查看 Pod 状态
kubectl get pods -n relay-system

# 端口转发到本地测试
kubectl port-forward -n relay-system svc/grpc-relay-relay 50051:50051 8080:8080

# 验证健康检查
curl http://localhost:8080/health
```

---

## 2. 停止服务

### 裸机 / systemd 方式

```bash
# 仅停止服务
sudo systemctl stop relay

# 完整卸载（保留配置与日志）
sudo ./deploy/bare-metal/uninstall.sh
```

### Docker Compose 方式（停止）

```bash
# 停止 relay 容器
docker-compose stop relay

# 完全清理
docker-compose down

# 清理数据卷（注意：会删除持久化数据）
docker-compose down -v
```

### Kubernetes 方式

```bash
# 删除部署
kubectl delete deployment grpc-relay-relay -n relay-system

# 或缩放至 0（保留 Deployment 配置）
kubectl scale deployment grpc-relay-relay --replicas=0 -n relay-system

# 完全清理
kubectl delete -k deploy/kubernetes/
```

**当前实现说明**：Relay 进程当前仅监听 `Ctrl+C`（`tokio::signal::ctrl_c()`）触发退出，未实现基于 SIGTERM 的请求排空/连接排空流程。由 systemd / Docker / Kubernetes 发出的 SIGTERM 将按进程默认行为终止。

---

## 3. 滚动更新

### 裸机 / systemd 方式

```bash
# 更新二进制并重启
sudo ./deploy/bare-metal/upgrade.sh

# 如需一并覆盖主机配置
sudo UPDATE_CONFIG=true ./deploy/bare-metal/upgrade.sh

# 回滚方式：重新放回上一版本二进制后再次执行 upgrade.sh
```

### Docker Compose 方式

```bash
# 拉取新镜像并重建
docker-compose pull relay
docker-compose up -d --no-deps relay

# 查看日志确认更新
docker-compose logs -f relay
```

### Kubernetes 方式

```bash
# 更新镜像
kubectl set image deployment/grpc-relay-relay relay=ghcr.io/<owner>/grpc-relay:v1.1.0 -n relay-system

# 监控滚动更新进度
kubectl rollout status deployment/grpc-relay-relay -n relay-system

# 查看更新历史
kubectl rollout history deployment/grpc-relay-relay -n relay-system

# 回滚到上一版本
kubectl rollout undo deployment/grpc-relay-relay -n relay-system

# 回滚到指定版本
kubectl rollout undo deployment/grpc-relay-relay --to-revision=2 -n relay-system
```

---

## 4. 扩容/缩容

### 裸机 / systemd 方式

裸机方案默认是单实例。若需要横向扩容，建议通过多台主机或前置负载均衡器分流；当前仓库未提供 `systemd` 模板化多实例单机编排。

### Kubernetes 方式

```bash
# 手动扩容
kubectl scale deployment grpc-relay-relay --replicas=3 -n relay-system

# 查看 HPA 状态
kubectl get hpa grpc-relay-relay-hpa -n relay-system

# 查看 HPA 详细信息
kubectl describe hpa grpc-relay-relay-hpa -n relay-system

# 修改 HPA 阈值
kubectl edit hpa grpc-relay-relay-hpa -n relay-system
```

**注意**：首版为单节点部署（replicas: 1）。HPA 配置已就绪，多节点场景下自动生效。

---

## 5. 日志查看

### 裸机 / systemd 方式

```bash
# 查看 systemd 日志
journalctl -u relay -f

# 查看审计日志
sudo tail -f /var/log/grpc-relay/audit.log
```

### Docker Compose 方式

```bash
# 查看 Relay 日志（实时跟踪）
docker-compose logs -f relay

# 查看 MQTT Broker 日志
docker-compose logs -f mqtt

# 按日志级别过滤（仅 ERROR）
docker-compose logs relay | grep ERROR

# 查看审计日志文件
docker-compose exec relay cat /var/log/relay/audit.log
```

### Kubernetes 方式

```bash
# 查看部署日志
kubectl logs -f deployment/grpc-relay-relay -n relay-system

# 查看指定 Pod 日志
kubectl logs -f relay-xxxxxxxxxx-xxxxx -n relay-system

# 查看最近 100 行
kubectl logs --tail=100 deployment/grpc-relay-relay -n relay-system

# 查看前一个容器实例日志（crash 后调试）
kubectl logs --previous deployment/grpc-relay-relay -n relay-system

# 查看审计日志
kubectl exec -it deployment/grpc-relay-relay -n relay-system -- tail -f /var/log/relay/audit.log

# 查看所有容器日志（包含 MQTT）
kubectl logs -f deployment/grpc-relay-relay -n relay-system --all-containers
```

---

## 6. 指标查看

### Prometheus 指标端点

```bash
# 查看原始指标
curl http://localhost:8080/metrics

# 查看安全指标
curl http://localhost:8080/metrics/security
```

### 关键 Prometheus 查询

```promql
# 活跃设备连接数
relay_active_device_connections

# P99 延迟（5 分钟窗口）
histogram_quantile(0.99, rate(relay_request_latency_seconds_bucket[5m]))

# 错误率（5 分钟窗口）
rate(relay_errors_total[5m]) / rate(relay_requests_total[5m])

# 设备连接速率
rate(relay_device_connections_total[5m])

# MQTT 发布速率
relay_mqtt_publish_rate

# CPU 使用率
relay_cpu_usage_percent

# 内存使用率
relay_memory_usage_percent

# 认证失败率（每分钟）
rate(relay_auth_failures_total[1m])

# 限流触发次数（5 分钟窗口）
rate(relay_rate_limit_hits_total[5m])
```

### Grafana 仪表板

浏览器访问 `http://localhost:3000`，使用你在 `.env` 中设置的 `GRAFANA_ADMIN_PASSWORD` 登录（用户名默认 `admin`），进入 "gRPC-Relay Overview" 仪表板。

---

## 7. 故障排查

### 问题 1：设备无法连接

```bash
# 1. 检查 Relay 服务健康状态
curl http://localhost:8080/health

# 2. 检查端口是否监听
netstat -tuln | grep 50051    # gRPC

# 3. 检查容器/Pod 日志
docker-compose logs relay | grep -E "ERROR|WARN"
kubectl logs deployment/grpc-relay-relay -n relay-system | grep -E "ERROR|WARN"

# 4. 检查防火墙规则
iptables -L -n | grep 50051

# 5. 检查连接日志（审计）
docker-compose exec relay tail -50 /var/log/relay/audit.log | grep device_connect

# 6. 检查认证服务配置
curl http://localhost:8080/health | jq '.components.auth_service'
```

### 问题 2：高延迟

```bash
# 1. 查看 P50/P95/P99 延迟
curl -s http://localhost:8080/metrics | grep relay_request_latency

# 2. 检查 CPU 和内存使用
docker stats relay-001
kubectl top pod -n relay-system

# 3. 查看活跃流数（高的流数可能导致延迟）
curl -s http://localhost:8080/metrics | grep relay_active_streams

# 4. 检查队列深度
curl -s http://localhost:8080/metrics | grep relay_pending_messages

# 5. 检查 MQTT 积压
curl -s http://localhost:8080/metrics | grep relay_mqtt_publish_rate
```

### 问题 3：MQTT 断连

```bash
# 1. 检查 MQTT Broker 状态
curl http://localhost:8080/health | jq '.components.mqtt_client'

# 2. 查看 MQTT Broker 日志
docker-compose logs mqtt | grep ERROR

# 3. 查看 Relay MQTT 状态
curl -s http://localhost:8080/metrics | grep relay_mqtt

# 4. 手动测试 MQTT 连接
mosquitto_sub -h localhost -p 1883 -t "relay/+/device/online" -C 1 -W 5

# 5. 检查 Controller 是否降级到 gRPC 轮询
curl http://localhost:8080/health | jq '.metrics'
```

### 问题 4：内存泄漏

```bash
# 1. 查看内存使用趋势（持续观察 10 分钟）
watch -n 10 'docker stats relay-001 --no-stream'

# K8s:
kubectl top pod -n relay-system --containers

# 2. 查看进程 RSS
docker-compose exec relay ps aux

# 3. 检查文件描述符数（持续增长 = 连接泄漏）
curl -s http://localhost:8080/metrics | grep relay_open_file_descriptors

# 4. 检查活跃连接数（与设备基数对比）
curl -s http://localhost:8080/metrics | grep relay_active_device_connections

# 5. 检查审计日志是否有未关闭的 session
docker-compose exec relay grep session_expired /var/log/relay/audit.log
```

---

## 8. 备份和恢复

### 裸机配置备份

```bash
sudo cp /etc/grpc-relay/relay.yaml /etc/grpc-relay/relay.yaml.bak
sudo cp /etc/grpc-relay/relay.env /etc/grpc-relay/relay.env.bak
sudo tar -czf grpc-relay-tls-backup.tgz /etc/grpc-relay/tls
```

### 配置备份

```bash
# 备份 Kubernetes ConfigMap
kubectl get configmap grpc-relay-relay-config -n relay-system -o yaml > relay-config-backup.yaml

# 备份 Kubernetes Secret
kubectl get secret grpc-relay-relay-secrets -n relay-system -o yaml > relay-secrets-backup.yaml

# 备份 Docker Compose 配置
cp config/relay.yaml config/relay.yaml.bak
cp docker-compose.yml docker-compose.yml.bak
```

### 会话状态备份（如启用 Redis 持久化，后续版本）

```bash
# Redis RDB 备份
redis-cli --rdb /backup/dump.rdb
```

### 恢复

```bash
# K8s 恢复
kubectl apply -f relay-config-backup.yaml
kubectl apply -f relay-secrets-backup.yaml

# 重启 Pod 使配置生效
kubectl rollout restart deployment/grpc-relay-relay -n relay-system
```

---

## 9. 安全加固

### 裸机证书与密钥

```bash
# 更新 TLS 证书后重启服务
sudo install -m 0640 server.crt /etc/grpc-relay/tls/server.crt
sudo install -m 0640 server.key /etc/grpc-relay/tls/server.key
sudo chown relay:relay /etc/grpc-relay/tls/server.crt /etc/grpc-relay/tls/server.key
sudo systemctl restart relay
```

### 裸机 JWT 密钥更新

```bash
sudo editor /etc/grpc-relay/relay.env
sudo systemctl restart relay
```

### 证书轮换

```bash
# 1. 生成新证书
openssl req -x509 -newkey rsa:4096 -keyout server.key -out server.crt -days 365 -nodes \
  -subj "/CN=relay.example.com"

# 2. 更新 Kubernetes Secret
kubectl create secret generic grpc-relay-relay-secrets-new \
  --from-file=tls.crt=server.crt \
  --from-file=tls.key=server.key \
  -n relay-system

# 3. 更新 Deployment（指到新 Secret）
kubectl edit deployment grpc-relay-relay -n relay-system
# 修改 volumes 中 secretName: grpc-relay-relay-secrets → grpc-relay-relay-secrets-new

# 4. 滚动更新
kubectl rollout restart deployment/grpc-relay-relay -n relay-system
```

### Token 撤销

```bash
# 通过 gRPC RevokeToken RPC 撤销（admin_token 在请求体中）
grpcurl -d '{"controller_id":"ctrl-1","admin_token":"'"$ADMIN_JWT"'","target_type":"DEVICE","target_token_hash_or_prefix":"device-token-prefix","reason":"device decommissioned"}' \
  localhost:50051 relay.v1.RelayService/RevokeToken

# 撤销 Controller JWT
grpcurl -d '{"controller_id":"ctrl-1","admin_token":"'"$ADMIN_JWT"'","target_type":"CONTROLLER","target_token_hash_or_prefix":"eyJ...","reason":"controller token rotation"}' \
  localhost:50051 relay.v1.RelayService/RevokeToken
```

### 更新 JWT 密钥

```bash
# K8s: 更新 Secret 中的 jwt-hs256-secret
kubectl create secret generic grpc-relay-relay-secrets \
  --from-file=jwt-hs256-secret=<(echo -n "new-strong-secret") \
  --dry-run=client -o yaml -n relay-system | kubectl apply -f - -n relay-system

# 重启使新密钥生效
kubectl rollout restart deployment/grpc-relay-relay -n relay-system
```

---

## 10. 告警响应

| 告警          | 级别     | 响应流程                                                                 |
| ------------- | -------- | ------------------------------------------------------------------------ |
| CPU > 80%     | Warning  | 检查连接数趋势，评估是否需要扩容；检查是否有异常流量                      |
| CPU > 95%     | Critical | 立即扩容或启动限流；通知值班人员；分析 CPU profile                        |
| 内存 > 85%    | Warning  | 检查是否存在内存泄漏；对比历史内存趋势                                    |
| 内存 > 95%    | Critical | 重启服务缓解（如确认泄漏）；准备扩容                                      |
| 错误率 > 1%   | Warning  | 查询审计日志定位错误来源；检查下游依赖（MQTT/Auth）状态                   |
| 错误率 > 5%   | Critical | 检查是否需要回滚到上一版本；通知值班人员                                  |
| P99 > 50ms    | Warning  | 检查网络延迟；检查队列深度；检查活跃流数                                  |
| P99 > 100ms   | Critical | 分析 trace 定位慢链路；评估是否需要扩容或优化                             |
| MQTT 断连 30s | Warning  | 检查 MQTT Broker 状态；Controller 自动切换到 gRPC 轮询补偿               |
| MQTT 断连 60s | Critical | 检查 Broker 是否需要重启；考虑切换备用 Broker；通知值班人员               |
| 认证失败 >10  | Warning  | 检查是否有异常登录尝试；查询来源 IP                                       |
| 认证失败 >50  | Critical | 可能是暴力攻击；启动 IP 封禁；通知安全团队                                |

**告警抑制规则**：同一告警 5 分钟内只发送一次；Critical 告警触发后抑制同指标的 Warning 告警。
