#!/bin/bash
set -e

if [ -z "$GATEWAY_ADDR" ]; then
    PUSH_PORT=${PUSH_PORT:-9000}
    METADATA_URL="http://metadata.google.internal/computeMetadata/v1"
    METADATA_HEADER="Metadata-Flavor: Google"
    
    # === GCP ===
    GCP_INSTANCE=$(curl -sf -m 2 -H "$METADATA_HEADER" "$METADATA_URL/instance/name" 2>/dev/null || true)
    if [ -n "$GCP_INSTANCE" ]; then
        export INSTANCE_ID="${GCP_INSTANCE}"
        
        EXTERNAL_IP=$(curl -sf -m 2 -H "$METADATA_HEADER" \
            "$METADATA_URL/instance/network-interfaces/0/access-configs/0/external-ip" 2>/dev/null || true)
        if [ -n "$EXTERNAL_IP" ]; then
            export GATEWAY_ADDR="${EXTERNAL_IP}:${PUSH_PORT}"
            echo "Using GCP external IP: $GATEWAY_ADDR"
            exec /app/gateway "$@"
        fi
        
        # Fallback to internal IP
        INTERNAL_IP=$(curl -sf -m 2 -H "$METADATA_HEADER" \
            "$METADATA_URL/instance/network-interfaces/0/ip" 2>/dev/null || true)
        if [ -n "$INTERNAL_IP" ]; then
            export GATEWAY_ADDR="${INTERNAL_IP}:${PUSH_PORT}"
            echo "Using GCP internal IP: $GATEWAY_ADDR"
            exec /app/gateway "$@"
        fi
    fi
fi

exec /app/gateway "$@"
