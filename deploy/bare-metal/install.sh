#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BINARY_SOURCE="${BINARY_SOURCE:-${REPO_ROOT}/target/release/relay}"
CONFIG_SOURCE="${CONFIG_SOURCE:-${REPO_ROOT}/config/relay.yaml}"
ENV_SOURCE="${ENV_SOURCE:-${SCRIPT_DIR}/relay.env.example}"
SERVICE_SOURCE="${SERVICE_SOURCE:-${SCRIPT_DIR}/relay.service}"

INSTALL_BINARY_PATH="${INSTALL_BINARY_PATH:-/usr/local/bin/relay}"
CONFIG_DIR="${CONFIG_DIR:-/etc/grpc-relay}"
TLS_DIR="${TLS_DIR:-${CONFIG_DIR}/tls}"
LOG_DIR="${LOG_DIR:-/var/log/grpc-relay}"
DATA_DIR="${DATA_DIR:-/var/lib/grpc-relay}"
ENV_TARGET="${ENV_TARGET:-${CONFIG_DIR}/relay.env}"
CONFIG_TARGET="${CONFIG_TARGET:-${CONFIG_DIR}/relay.yaml}"
SERVICE_TARGET="${SERVICE_TARGET:-/etc/systemd/system/relay.service}"
SERVICE_NAME="${SERVICE_NAME:-relay}"
SERVICE_USER="${SERVICE_USER:-relay}"
SERVICE_GROUP="${SERVICE_GROUP:-relay}"

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    echo "error: please run as root" >&2
    exit 1
  fi
}

ensure_source() {
  local path="$1"
  local name="$2"
  if [[ ! -f "${path}" ]]; then
    echo "error: missing ${name}: ${path}" >&2
    exit 1
  fi
}

ensure_user() {
  if ! getent group "${SERVICE_GROUP}" >/dev/null; then
    groupadd --system "${SERVICE_GROUP}"
  fi

  if ! id -u "${SERVICE_USER}" >/dev/null 2>&1; then
    useradd --system \
      --gid "${SERVICE_GROUP}" \
      --home-dir "${DATA_DIR}" \
      --shell /usr/sbin/nologin \
      "${SERVICE_USER}"
  fi
}

main() {
  require_root
  ensure_source "${BINARY_SOURCE}" "relay binary"
  ensure_source "${CONFIG_SOURCE}" "relay config"
  ensure_source "${ENV_SOURCE}" "environment template"
  ensure_source "${SERVICE_SOURCE}" "systemd unit"

  ensure_user

  install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" "${CONFIG_DIR}" "${TLS_DIR}" "${LOG_DIR}" "${DATA_DIR}"
  install -m 0755 "${BINARY_SOURCE}" "${INSTALL_BINARY_PATH}"
  install -m 0644 "${CONFIG_SOURCE}" "${CONFIG_TARGET}"

  if [[ ! -f "${ENV_TARGET}" ]]; then
    install -m 0640 "${ENV_SOURCE}" "${ENV_TARGET}"
  else
    echo "info: preserving existing env file at ${ENV_TARGET}"
  fi

  install -m 0644 "${SERVICE_SOURCE}" "${SERVICE_TARGET}"

  chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "${CONFIG_DIR}" "${TLS_DIR}" "${LOG_DIR}" "${DATA_DIR}"

  systemctl daemon-reload
  systemctl enable --now "${SERVICE_NAME}"

  echo "installed ${SERVICE_NAME}"
  systemctl --no-pager --full status "${SERVICE_NAME}" || true
}

main "$@"
