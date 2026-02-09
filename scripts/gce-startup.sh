#!/bin/bash
# GCE instance startup script (runs on host, not in container)
# System optimization

set -e

echo "* soft nofile 65535" >> /etc/security/limits.conf
echo "* hard nofile 65535" >> /etc/security/limits.conf
sysctl -w net.core.somaxconn=65535
sysctl -w net.ipv4.tcp_max_syn_backlog=65535

METADATA_URL="http://metadata.google.internal/computeMetadata/v1"
METADATA_HEADER="Metadata-Flavor: Google"

INSTANCE_NAME=$(curl -sf -H "$METADATA_HEADER" "$METADATA_URL/instance/name" || echo "unknown")
EXTERNAL_IP=$(curl -sf -H "$METADATA_HEADER" "$METADATA_URL/instance/network-interfaces/0/access-configs/0/external-ip" || echo "unknown")

echo "Instance: $INSTANCE_NAME"
echo "External IP: $EXTERNAL_IP"
echo "Startup script completed"
