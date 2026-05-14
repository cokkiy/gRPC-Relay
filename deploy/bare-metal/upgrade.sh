#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BINARY_SOURCE="${BINARY_SOURCE:-${REPO_ROOT}/target/release/relay}"
CONFIG_SOURCE="${CONFIG_SOURCE:-${REPO_ROOT}/config/relay.yaml}"
INSTALL_BINARY_PATH="${INSTALL_BINARY_PATH:-/usr/local/bin/relay}"
CONFIG_TARGET="${CONFIG_TARGET:-/etc/grpc-relay/relay.yaml}"
SERVICE_NAME="${SERVICE_NAME:-relay}"
UPDATE_CONFIG="${UPDATE_CONFIG:-false}"

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

main() {
  require_root
  ensure_source "${BINARY_SOURCE}" "relay binary"

  install -m 0755 "${BINARY_SOURCE}" "${INSTALL_BINARY_PATH}"

  if [[ "${UPDATE_CONFIG}" == "true" ]]; then
    ensure_source "${CONFIG_SOURCE}" "relay config"
    install -m 0644 "${CONFIG_SOURCE}" "${CONFIG_TARGET}"
  fi

  systemctl restart "${SERVICE_NAME}"
  echo "upgraded ${SERVICE_NAME}"
  systemctl --no-pager --full status "${SERVICE_NAME}" || true
}

main "$@"
