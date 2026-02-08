#!/bin/bash

# ============================================
# SSE Gateway - Cloud Run Deployment Script
# ============================================
#
# Use Cases:
#   - High traffic variability, requiring rapid scaling
#   - Concurrency < 50,000 (50 instances x 1000)
#   - Pay-per-use pricing, minimize costs during idle periods
#
# Usage:
#   ./deploy-cloudrun.sh                    # Deploy service
#   ./deploy-cloudrun.sh secret NAME VALUE  # Add/update Secret
#   ./deploy-cloudrun.sh secrets            # List all Secrets
#   ./deploy-cloudrun.sh delete             # Delete service
#
# Environment Variables:
#   PROJECT             - GCP Project ID (required, or uses gcloud default project)
#   REGION              - Deployment region (default: asia-east1)
#   MAX_INSTANCES       - Maximum instances (default: 50, supports 50,000 concurrent)
#   MIN_INSTANCES       - Minimum instances (default: 1)
#   REDIS_URL           - Redis connection URL (optional)
#   ENABLE_DASHBOARD    - Enable Dashboard (default: true)

set -e

# Color definitions
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Configuration
PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
REGION=${REGION:-"asia-east1"}
SERVICE_NAME="gateway-sse"
IMAGE_NAME="gcr.io/$PROJECT_ID/$SERVICE_NAME"

# Cloud Run configuration
MAX_INSTANCES=${MAX_INSTANCES:-50}
MIN_INSTANCES=${MIN_INSTANCES:-1}
MEMORY=${MEMORY:-"1Gi"}
CPU=${CPU:-"2"}
CONCURRENCY=${CONCURRENCY:-1000}

# Pub/Sub configuration
TOPIC_NAME="gateway-subscription"
SUBSCRIPTION_NAME="gateway-subscription-sub"

# Print functions
print_header() {
    echo -e "\n${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  $1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}
print_step() { echo -e "\n${BLUE}[$1]${NC} $2"; }
print_success() { echo -e "  ${GREEN}✓${NC} $1"; }
print_warning() { echo -e "  ${YELLOW}!${NC} $1"; }
print_error() { echo -e "  ${RED}✗${NC} $1"; exit 1; }
print_info() { echo -e "  ${CYAN}→${NC} $1"; }

# Check environment
check_requirements() {
    print_step "1/6" "Checking environment"
    
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI not installed"
    fi
    print_success "gcloud CLI installed"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "Please set project: export PROJECT=your-project-id"
    fi
    print_success "Project: $PROJECT_ID"
    print_success "Region: $REGION"
    print_info "Max instances: $MAX_INSTANCES (supports $((MAX_INSTANCES * CONCURRENCY)) concurrent)"
    
    gcloud config set project $PROJECT_ID --quiet
}

# Enable APIs
enable_apis() {
    print_step "2/6" "Enabling Google Cloud APIs"
    
    apis=(
        "cloudbuild.googleapis.com"
        "run.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    for api in "${apis[@]}"; do
        if gcloud services list --enabled --filter="name:$api" --format="value(name)" 2>/dev/null | grep -q "$api"; then
            print_success "$api (already enabled)"
        else
            gcloud services enable $api --quiet 2>/dev/null
            print_success "$api (newly enabled)"
        fi
    done
    
    # Grant Secret Manager permissions
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    gcloud projects add-iam-policy-binding $PROJECT_ID \
        --member="serviceAccount:${SERVICE_ACCOUNT}" \
        --role="roles/secretmanager.secretAccessor" \
        --quiet 2>/dev/null || true
    print_success "Secret Manager permissions configured"
}

# Configure Pub/Sub
setup_pubsub() {
    print_step "3/6" "Configuring Pub/Sub"
    
    if gcloud pubsub topics describe $TOPIC_NAME &>/dev/null; then
        print_success "Topic: $TOPIC_NAME (already exists)"
    else
        gcloud pubsub topics create $TOPIC_NAME
        print_success "Topic: $TOPIC_NAME (created)"
    fi
    
    if gcloud pubsub subscriptions describe $SUBSCRIPTION_NAME &>/dev/null; then
        print_success "Subscription: $SUBSCRIPTION_NAME (already exists)"
    else
        gcloud pubsub subscriptions create $SUBSCRIPTION_NAME \
            --topic=$TOPIC_NAME \
            --ack-deadline=10 \
            --message-retention-duration=1h
        print_success "Subscription: $SUBSCRIPTION_NAME (created)"
    fi
}

# Build image
build_image() {
    print_step "4/6" "Building container image"
    print_info "Building with Cloud Build..."
    
    gcloud builds submit \
        --tag "$IMAGE_NAME:latest" \
        --quiet \
        .
    
    print_success "Image built: $IMAGE_NAME:latest"
}

# Check if a secret exists in Secret Manager
secret_exists() {
    gcloud secrets describe "$1" --project=$PROJECT_ID &>/dev/null 2>&1
}

# Deploy service
deploy_service() {
    print_step "5/6" "Deploying to Cloud Run"
    
    # Base environment variables (non-sensitive)
    ENV_VARS="RUST_LOG=info"
    
    # Build secrets list (sensitive configs from Secret Manager)
    SECRETS=""
    
    # GCP_PROJECT - check Secret Manager first, fallback to env var
    if secret_exists "GCP_PROJECT"; then
        SECRETS="GCP_PROJECT=GCP_PROJECT:latest"
        print_info "GCP_PROJECT: from Secret Manager"
    else
        ENV_VARS="$ENV_VARS,GCP_PROJECT=$PROJECT_ID"
        print_info "GCP_PROJECT: from environment variable"
    fi
    
    # PUBSUB_SUBSCRIPTION - check Secret Manager first, fallback to env var
    if secret_exists "PUBSUB_SUBSCRIPTION"; then
        [ -n "$SECRETS" ] && SECRETS="$SECRETS,"
        SECRETS="${SECRETS}PUBSUB_SUBSCRIPTION=PUBSUB_SUBSCRIPTION:latest"
        print_info "PUBSUB_SUBSCRIPTION: from Secret Manager"
    else
        ENV_VARS="$ENV_VARS,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
        print_info "PUBSUB_SUBSCRIPTION: from environment variable"
    fi
    
    # REDIS_URL - check Secret Manager first, then env var, then skip
    if secret_exists "REDIS_URL"; then
        [ -n "$SECRETS" ] && SECRETS="$SECRETS,"
        SECRETS="${SECRETS}REDIS_URL=REDIS_URL:latest"
        print_info "REDIS_URL: from Secret Manager"
    elif [ -n "$REDIS_URL" ]; then
        ENV_VARS="$ENV_VARS,REDIS_URL=$REDIS_URL"
        print_info "REDIS_URL: from environment variable"
    else
        print_warning "REDIS_URL: not configured (message replay disabled)"
    fi
    
    # Optional: Dashboard
    if [ "${ENABLE_DASHBOARD:-true}" = "false" ]; then
        ENV_VARS="$ENV_VARS,ENABLE_DASHBOARD=false"
        print_info "Dashboard: disabled"
    fi
    
    # Build deploy command
    DEPLOY_CMD="gcloud run deploy $SERVICE_NAME \
        --image $IMAGE_NAME:latest \
        --region $REGION \
        --platform managed \
        --allow-unauthenticated \
        --min-instances $MIN_INSTANCES \
        --max-instances $MAX_INSTANCES \
        --memory $MEMORY \
        --cpu $CPU \
        --concurrency $CONCURRENCY \
        --timeout 3600s \
        --no-cpu-throttling \
        --session-affinity \
        --set-env-vars $ENV_VARS \
        --quiet"
    
    # Add secrets if any exist
    if [ -n "$SECRETS" ]; then
        DEPLOY_CMD="$DEPLOY_CMD --set-secrets=$SECRETS"
        print_success "Using Secret Manager for sensitive configs"
    fi
    
    # Execute deployment
    eval $DEPLOY_CMD
    
    print_success "Cloud Run deployment complete"
}

# Test service
test_service() {
    print_step "6/6" "Verifying deployment"
    
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    
    sleep 3
    
    if curl -sk "$SERVICE_URL/health" 2>/dev/null | grep -q "OK"; then
        print_success "Health check passed"
    else
        print_warning "Health check failed, service may still be starting"
    fi
}

# Print summary
print_summary() {
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    
    print_header "Cloud Run Deployment Complete"
    echo ""
    echo -e "  Service URL:  ${GREEN}$SERVICE_URL${NC}"
    echo -e "  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}"
    echo -e "  SSE Connect:  ${GREEN}$SERVICE_URL/sse/connect?channel_id=YOUR_CHANNEL${NC}"
    echo ""
    echo -e "  ${CYAN}Configuration:${NC}"
    echo -e "    Instance range: $MIN_INSTANCES - $MAX_INSTANCES"
    echo -e "    Concurrency per instance: $CONCURRENCY"
    echo -e "    Max concurrency: $((MAX_INSTANCES * CONCURRENCY))"
    echo -e "    Resources: $CPU vCPU / $MEMORY"
    echo ""
    echo -e "  ${CYAN}Test command:${NC}"
    echo -e "  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{\"msg\":\"Hello!\"}' \\
    --attribute=channel_id=test,event_type=notification${NC}"
    echo ""
}

# Delete service
delete_service() {
    print_header "Delete Cloud Run Service"
    
    echo -e "${YELLOW}The following resources will be deleted:${NC}"
    echo "  - Cloud Run service: $SERVICE_NAME"
    echo ""
    read -p "Confirm deletion? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        gcloud run services delete $SERVICE_NAME --region $REGION --quiet
        print_success "Service deleted"
    else
        print_info "Cancelled"
    fi
}

# Secret management
add_secret() {
    local name=$1
    local value=$2
    
    if [ -z "$name" ] || [ -z "$value" ]; then
        echo ""
        echo "Usage: ./deploy-cloudrun.sh secret NAME VALUE"
        echo ""
        echo "Available secrets:"
        echo "  REDIS_URL          - Redis connection URL (for message replay)"
        echo "  GCP_PROJECT        - GCP Project ID"
        echo "  PUBSUB_SUBSCRIPTION - Pub/Sub subscription name"
        echo ""
        echo "Example:"
        echo "  ./deploy-cloudrun.sh secret REDIS_URL redis://user:pass@host:6379"
        echo ""
        exit 1
    fi
    
    if gcloud secrets describe $name --project=$PROJECT_ID &>/dev/null; then
        echo -n "$value" | gcloud secrets versions add $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' updated"
    else
        echo -n "$value" | gcloud secrets create $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' created"
    fi
    
    print_info "Secret will be used on next deployment"
}

list_secrets() {
    print_header "Secrets List"
    gcloud secrets list --project=$PROJECT_ID --format="table(name,createTime)"
}

# Main function
main() {
    print_header "SSE Gateway - Cloud Run Deployment"
    
    check_requirements
    enable_apis
    setup_pubsub
    build_image
    deploy_service
    test_service
    print_summary
}

# Command handling
case "${1:-}" in
    secret)
        add_secret "$2" "$3"
        ;;
    secrets)
        list_secrets
        ;;
    delete)
        delete_service
        ;;
    *)
        main
        ;;
esac
