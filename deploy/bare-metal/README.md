# Bare-Metal Deployment

This directory contains the native Linux deployment assets for running `relay`
without Docker or Kubernetes.

## Files

- `relay.service`: `systemd` unit for managing the relay process
- `relay.env.example`: host environment template loaded by `systemd`
- `install.sh`: install and enable the service on a host
- `upgrade.sh`: replace the binary and restart the service
- `uninstall.sh`: disable and remove the service

## Suggested host layout

- Binary: `/usr/local/bin/relay`
- Config: `/etc/grpc-relay/relay.yaml`
- Environment: `/etc/grpc-relay/relay.env`
- TLS certs: `/etc/grpc-relay/tls/`
- Logs: `/var/log/grpc-relay/`
- Runtime data: `/var/lib/grpc-relay/`

## Installation steps

1. Build or download the relay binary.
2. Run the installer:

```bash
sudo ./deploy/bare-metal/install.sh
```

3. Edit `/etc/grpc-relay/relay.env` and add real TLS/JWT values if needed.
4. Verify:

```bash
systemctl status relay
curl http://127.0.0.1:8080/health
```

## Upgrade

```bash
sudo ./deploy/bare-metal/upgrade.sh
```

To also refresh the host config from `config/relay.yaml`:

```bash
sudo UPDATE_CONFIG=true ./deploy/bare-metal/upgrade.sh
```

## Uninstall

```bash
sudo ./deploy/bare-metal/uninstall.sh
```

To remove `/etc/grpc-relay`, logs, and runtime data as well:

```bash
sudo REMOVE_DATA=true ./deploy/bare-metal/uninstall.sh
```
