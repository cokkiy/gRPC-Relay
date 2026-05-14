#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="${SERVICE_NAME:-relay}"
INSTALL_BINARY_PATH="${INSTALL_BINARY_PATH:-/usr/local/bin/relay}"
SERVICE_TARGET="${SERVICE_TARGET:-/etc/systemd/system/relay.service}"
REMOVE_DATA="${REMOVE_DATA:-false}"
CONFIG_DIR="${CONFIG_DIR:-/etc/grpc-relay}"
LOG_DIR="${LOG_DIR:-/var/log/grpc-relay}"
DATA_DIR="${DATA_DIR:-/var/lib/grpc-relay}"

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    echo "error: please run as root" >&2
    exit 1
  fi
}

main() {
  require_root

  if systemctl list-unit-files | grep -q "^${SERVICE_NAME}\.service"; then
    systemctl disable --now "${SERVICE_NAME}" || true
  fi

  rm -f "${SERVICE_TARGET}" "${INSTALL_BINARY_PATH}"
  systemctl daemon-reload

  if [[ "${REMOVE_DATA}" == "true" ]]; then
    rm -rf "${CONFIG_DIR}" "${LOG_DIR}" "${DATA_DIR}"
  else
    echo "info: preserved ${CONFIG_DIR}, ${LOG_DIR}, ${DATA_DIR}"
  fi

  echo "uninstalled ${SERVICE_NAME}"
}

main "$@"
