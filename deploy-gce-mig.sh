#!/bin/bash

# ============================================
# SSE Gateway - GCE Managed Instance Group Deployment Script
# ============================================
#
# Use Cases:
#   - High concurrency long connections (50,000+ concurrent)
#   - Cost sensitive (20x cheaper than Cloud Run)
#   - Relatively stable traffic
#
# Architecture:
#   Load Balancer → Managed Instance Group (auto-scaling) → SSE Gateway
#
# Usage:
#   ./deploy-gce-mig.sh                # Initial deployment (build image + create resources)
#   ./deploy-gce-mig.sh start          # Start all services (instances + LB)
#   ./deploy-gce-mig.sh stop           # Stop all services (zero cost)
#   ./deploy-gce-mig.sh scale N        # Quickly scale instance count
#   ./deploy-gce-mig.sh status         # View status
#   ./deploy-gce-mig.sh update         # Update code (rebuild + rolling update)
#   ./deploy-gce-mig.sh ssh            # SSH to instance for debugging
#   ./deploy-gce-mig.sh secret NAME VALUE  # Add/update Secret
#   ./deploy-gce-mig.sh secrets        # List all Secrets
#   ./deploy-gce-mig.sh delete         # Delete all resources
#
# Environment Variables:
#   SKIP_BUILD=true     - Skip image build, use existing image
#   SKIP_LB=true        - Skip Load Balancer (save $18/month, connect via instance IP)
#
# Environment Variables:
#   PROJECT             - GCP Project ID
#   REGION              - Deployment region (default: asia-east1)
#   ZONE                - Deployment zone (default: asia-east1-a)
#   MACHINE_TYPE        - Machine type (default: e2-micro, cheapest ~$7/month)
#   INIT_SIZE           - Initial instance count (default: 0)

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
ZONE=${ZONE:-"asia-east1-a"}

# Resource names
SERVICE_NAME="gateway-sse"
INSTANCE_TEMPLATE="${SERVICE_NAME}-template"
INSTANCE_GROUP="${SERVICE_NAME}-mig"
HEALTH_CHECK="${SERVICE_NAME}-health-check"
BACKEND_SERVICE="${SERVICE_NAME}-backend"
URL_MAP="${SERVICE_NAME}-url-map"
HTTP_PROXY="${SERVICE_NAME}-http-proxy"
FORWARDING_RULE="${SERVICE_NAME}-forwarding-rule"
FIREWALL_RULE="${SERVICE_NAME}-allow-health-check"
FIREWALL_RULE_LB="${SERVICE_NAME}-allow-lb"

# Machine configuration
MACHINE_TYPE=${MACHINE_TYPE:-"e2-micro"}  # 0.25 vCPU, 1GB RAM (~$7/month)
INIT_SIZE=${INIT_SIZE:-0}                 # Initial instance count (default: 0)

# Container image
IMAGE_NAME="gcr.io/$PROJECT_ID/$SERVICE_NAME:latest"

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
    print_step "1/9" "Checking environment"
    
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI not installed"
    fi
    print_success "gcloud CLI installed"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "Please set project: export PROJECT=your-project-id"
    fi
    print_success "Project: $PROJECT_ID"
    print_success "Region: $REGION"
    print_success "Zone: $ZONE"
    print_info "Machine type: $MACHINE_TYPE (~\$7/month)"
    print_info "Initial instances: $INIT_SIZE (manual control)"
    
    gcloud config set project $PROJECT_ID --quiet
}

# Enable APIs
enable_apis() {
    print_step "2/9" "Enabling Google Cloud APIs"
    
    apis=(
        "compute.googleapis.com"
        "cloudbuild.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    for api in "${apis[@]}"; do
        if gcloud services list --enabled --filter="name:$api" --format="value(name)" 2>/dev/null | grep -q "$api"; then
            print_success "$api (already enabled)"
        else
            gcloud services enable $api --quiet
            print_success "$api (newly enabled)"
        fi
    done
}

# Configure Pub/Sub
setup_pubsub() {
    print_step "3/9" "Configuring Pub/Sub"
    
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

# Check if image exists
image_exists() {
    gcloud container images describe "$IMAGE_NAME" &>/dev/null
}

# Build image
build_image() {
    print_step "4/9" "Building container image"
    
    # Check if build should be skipped
    if [ "${SKIP_BUILD:-false}" = "true" ]; then
        if image_exists; then
            print_success "Skipping build, using existing image: $IMAGE_NAME"
            return 0
        else
            print_warning "Image does not exist, must build"
        fi
    fi
    
    # Check if image already exists
    if image_exists; then
        print_info "Image already exists: $IMAGE_NAME"
        read -p "  Rebuild? (y/N): " rebuild
        if [ "$rebuild" != "y" ] && [ "$rebuild" != "Y" ]; then
            print_success "Using existing image"
            return 0
        fi
    fi
    
    print_info "Building with Cloud Build..."
    
    gcloud builds submit \
        --tag "$IMAGE_NAME" \
        --quiet \
        .
    
    print_success "Image built: $IMAGE_NAME"
}

# Configure firewall
setup_firewall() {
    print_step "5/9" "Configuring firewall rules"
    
    # Allow health checks
    if gcloud compute firewall-rules describe $FIREWALL_RULE &>/dev/null; then
        print_success "Health check firewall rule (already exists)"
    else
        gcloud compute firewall-rules create $FIREWALL_RULE \
            --network=default \
            --action=allow \
            --direction=ingress \
            --source-ranges=130.211.0.0/22,35.191.0.0/16 \
            --target-tags=$SERVICE_NAME \
            --rules=tcp:8080 \
            --quiet
        print_success "Health check firewall rule (created)"
    fi
    
    # Allow Load Balancer traffic
    if gcloud compute firewall-rules describe $FIREWALL_RULE_LB &>/dev/null; then
        print_success "Load Balancer firewall rule (already exists)"
    else
        gcloud compute firewall-rules create $FIREWALL_RULE_LB \
            --network=default \
            --action=allow \
            --direction=ingress \
            --source-ranges=0.0.0.0/0 \
            --target-tags=$SERVICE_NAME \
            --rules=tcp:8080 \
            --quiet
        print_success "Load Balancer firewall rule (created)"
    fi
}

# Check if a secret exists in Secret Manager
secret_exists() {
    gcloud secrets describe "$1" --project=$PROJECT_ID &>/dev/null 2>&1
}

# Get secret value (for building container env)
get_secret_value() {
    gcloud secrets versions access latest --secret="$1" --project=$PROJECT_ID 2>/dev/null
}

# Create Instance Template
create_instance_template() {
    print_step "6/9" "Creating Instance Template"
    
    # Delete old template (if exists)
    if gcloud compute instance-templates describe $INSTANCE_TEMPLATE &>/dev/null; then
        print_warning "Deleting old template..."
        # Add timestamp to template name for rolling updates
        INSTANCE_TEMPLATE="${SERVICE_NAME}-template-$(date +%Y%m%d%H%M%S)"
    fi
    
    # Get service account
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    # Build container environment variables
    CONTAINER_ENV="RUST_LOG=info,GCP_PROJECT=$PROJECT_ID,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
    
    # Check for REDIS_URL in Secret Manager
    if secret_exists "REDIS_URL"; then
        REDIS_SECRET=$(get_secret_value "REDIS_URL")
        if [ -n "$REDIS_SECRET" ]; then
            CONTAINER_ENV="$CONTAINER_ENV,REDIS_URL=$REDIS_SECRET"
            print_info "REDIS_URL: from Secret Manager"
        fi
    elif [ -n "$REDIS_URL" ]; then
        CONTAINER_ENV="$CONTAINER_ENV,REDIS_URL=$REDIS_URL"
        print_info "REDIS_URL: from environment variable"
    else
        print_warning "REDIS_URL: not configured (message replay disabled)"
    fi
    
    # Create startup script
    STARTUP_SCRIPT=$(cat <<'STARTUP_EOF'
#!/bin/bash
# Increase file descriptor limit to support high concurrency
echo "* soft nofile 65535" >> /etc/security/limits.conf
echo "* hard nofile 65535" >> /etc/security/limits.conf
ulimit -n 65535

# Optimize network parameters
sysctl -w net.core.somaxconn=65535
sysctl -w net.ipv4.tcp_max_syn_backlog=65535
sysctl -w net.core.netdev_max_backlog=65535
sysctl -w net.ipv4.tcp_tw_reuse=1
sysctl -w net.ipv4.ip_local_port_range="1024 65535"

echo "System optimized for high concurrency SSE connections"
STARTUP_EOF
)

    gcloud compute instance-templates create-with-container $INSTANCE_TEMPLATE \
        --machine-type=$MACHINE_TYPE \
        --container-image=$IMAGE_NAME \
        --container-env="$CONTAINER_ENV" \
        --tags=$SERVICE_NAME \
        --network=default \
        --service-account=$SERVICE_ACCOUNT \
        --scopes=cloud-platform \
        --metadata=startup-script="$STARTUP_SCRIPT" \
        --boot-disk-size=20GB \
        --boot-disk-type=pd-standard \
        --quiet
    
    print_success "Instance Template: $INSTANCE_TEMPLATE"
}

# Create health check
create_health_check() {
    print_step "7/9" "Creating health check"
    
    if gcloud compute health-checks describe $HEALTH_CHECK &>/dev/null; then
        print_success "Health check (already exists)"
    else
        gcloud compute health-checks create http $HEALTH_CHECK \
            --port=8080 \
            --request-path=/health \
            --check-interval=10s \
            --timeout=5s \
            --healthy-threshold=2 \
            --unhealthy-threshold=3 \
            --quiet
        print_success "Health check (created)"
    fi
}

# Create Managed Instance Group
create_mig() {
    print_step "8/9" "Creating Managed Instance Group"
    
    if gcloud compute instance-groups managed describe $INSTANCE_GROUP --zone=$ZONE &>/dev/null; then
        print_warning "MIG already exists, updating template..."
        gcloud compute instance-groups managed set-instance-template $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE \
            --zone=$ZONE \
            --quiet
        
        # Trigger rolling update
        gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
            --version=template=$INSTANCE_TEMPLATE \
            --zone=$ZONE \
            --max-surge=1 \
            --max-unavailable=0 \
            --quiet
        print_success "MIG rolling update started"
    else
        # Create MIG
        gcloud compute instance-groups managed create $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE \
            --size=$INIT_SIZE \
            --zone=$ZONE \
            --health-check=$HEALTH_CHECK \
            --initial-delay=120 \
            --quiet
        print_success "MIG: $INSTANCE_GROUP (created)"
        print_success "Manual control mode (use scale command to adjust instance count)"
    fi
    
    # Set named ports
    gcloud compute instance-groups managed set-named-ports $INSTANCE_GROUP \
        --named-ports=http:8080 \
        --zone=$ZONE \
        --quiet
}

# Create Load Balancer
create_load_balancer() {
    # Check if LB should be skipped
    if [ "${SKIP_LB:-false}" = "true" ]; then
        print_step "9/9" "Skipping Load Balancer"
        print_success "Load Balancer skipped (save \$18/month)"
        print_warning "Access via instance IP:8080 directly"
        return 0
    fi
    
    print_step "9/9" "Creating Load Balancer"
    
    # Create Backend Service
    if gcloud compute backend-services describe $BACKEND_SERVICE --global &>/dev/null; then
        print_success "Backend Service (already exists)"
    else
        gcloud compute backend-services create $BACKEND_SERVICE \
            --protocol=HTTP \
            --port-name=http \
            --health-checks=$HEALTH_CHECK \
            --global \
            --timeout=3600s \
            --connection-draining-timeout=300s \
            --session-affinity=CLIENT_IP \
            --quiet
        print_success "Backend Service (created)"
    fi
    
    # Add MIG to Backend Service
    gcloud compute backend-services add-backend $BACKEND_SERVICE \
        --instance-group=$INSTANCE_GROUP \
        --instance-group-zone=$ZONE \
        --balancing-mode=UTILIZATION \
        --max-utilization=0.8 \
        --global \
        --quiet 2>/dev/null || true
    print_success "Backend associated"
    
    # Create URL Map
    if gcloud compute url-maps describe $URL_MAP &>/dev/null; then
        print_success "URL Map (already exists)"
    else
        gcloud compute url-maps create $URL_MAP \
            --default-service=$BACKEND_SERVICE \
            --quiet
        print_success "URL Map (created)"
    fi
    
    # Create HTTP Proxy
    if gcloud compute target-http-proxies describe $HTTP_PROXY &>/dev/null; then
        print_success "HTTP Proxy (already exists)"
    else
        gcloud compute target-http-proxies create $HTTP_PROXY \
            --url-map=$URL_MAP \
            --quiet
        print_success "HTTP Proxy (created)"
    fi
    
    # Create Forwarding Rule (get external IP)
    if gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null; then
        print_success "Forwarding Rule (already exists)"
    else
        gcloud compute forwarding-rules create $FORWARDING_RULE \
            --global \
            --target-http-proxy=$HTTP_PROXY \
            --ports=80 \
            --quiet
        print_success "Forwarding Rule (created)"
    fi
}

# Get instance IP
get_instance_ip() {
    INSTANCE=$(gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="value(instance)" \
        --limit=1 2>/dev/null)
    
    if [ -n "$INSTANCE" ]; then
        gcloud compute instances describe $INSTANCE \
            --zone=$ZONE \
            --format='value(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null
    fi
}

# Get service URL
get_service_url() {
    # First try to get LB IP
    LB_IP=$(gcloud compute forwarding-rules describe $FORWARDING_RULE --global --format='value(IPAddress)' 2>/dev/null)
    
    if [ -n "$LB_IP" ]; then
        echo "http://$LB_IP"
    else
        # No LB, get instance IP
        INSTANCE_IP=$(get_instance_ip)
        if [ -n "$INSTANCE_IP" ]; then
            echo "http://$INSTANCE_IP:8080"
        else
            echo "(instance not running, run ./deploy-gce-mig.sh status after starting)"
        fi
    fi
}

# Print summary
print_summary() {
    SERVICE_URL=$(get_service_url)
    
    print_header "GCE MIG Deployment Complete"
    echo ""
    echo -e "  Service URL:  ${GREEN}$SERVICE_URL${NC}"
    echo -e "  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}"
    echo -e "  SSE Connect:  ${GREEN}$SERVICE_URL/sse/connect?channel_id=YOUR_CHANNEL${NC}"
    echo ""
    echo -e "  ${CYAN}Configuration:${NC}"
    echo -e "    Machine type: $MACHINE_TYPE (~\$7/month/instance)"
    echo -e "    Control mode: Manual (use scale command)"
    echo ""
    echo -e "  ${CYAN}Cost estimate:${NC}"
    echo -e "    Per instance: ~\$7/month"
    # Check if Load Balancer exists
    if [ "${SKIP_LB:-false}" = "true" ] || ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        echo -e "    Load Balancer: ${GREEN}skipped (\$0)${NC}"
    else
        echo -e "    Load Balancer: ~\$18/month"
    fi
    echo ""
    echo -e "  ${CYAN}Management commands:${NC}"
    echo -e "    Start service:  ${YELLOW}./deploy-gce-mig.sh start${NC}"
    echo -e "    Stop service:   ${YELLOW}./deploy-gce-mig.sh stop${NC}  (zero cost)"
    echo -e "    Scale instances: ${YELLOW}./deploy-gce-mig.sh scale 3${NC}"
    echo -e "    View status:    ${YELLOW}./deploy-gce-mig.sh status${NC}"
    echo ""
    echo -e "  ${CYAN}Test command:${NC}"
    echo -e "  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{\"msg\":\"Hello!\"}' \\
    --attribute=channel_id=test,event_type=notification${NC}"
    echo ""
    echo -e "  ${YELLOW}Note: Load Balancer takes 3-5 minutes to fully configure${NC}"
    echo ""
}

# Show status
show_status() {
    print_header "GCE MIG Status"
    
    echo -e "\n${CYAN}Instance group status:${NC}"
    gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="table(instance,status,lastAttempt.errors)"
    
    echo -e "\n${CYAN}Auto-scaling status:${NC}"
    gcloud compute instance-groups managed describe $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="yaml(status)"
    
    echo -e "\n${CYAN}Backend health status:${NC}"
    gcloud compute backend-services get-health $BACKEND_SERVICE --global 2>/dev/null || echo "Backend not ready yet"
    
    SERVICE_URL=$(get_service_url)
    echo -e "\n${CYAN}Service URL:${NC} $SERVICE_URL"
}

# Manual scaling
scale_mig() {
    local size=$1
    
    if [ -z "$size" ]; then
        print_error "Usage: ./deploy-gce-mig.sh scale <instance_count>"
    fi
    
    print_info "Scaling to $size instances..."
    
    gcloud compute instance-groups managed resize $INSTANCE_GROUP \
        --size=$size \
        --zone=$ZONE \
        --quiet
    
    print_success "Scaling complete (current: $size instances)"
}

# Update image (rolling update)
update_mig() {
    print_header "Rolling Update MIG"
    
    build_image
    create_instance_template
    
    gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
        --version=template=$INSTANCE_TEMPLATE \
        --zone=$ZONE \
        --max-surge=1 \
        --max-unavailable=0 \
        --quiet
    
    print_success "Rolling update started"
    print_info "Use './deploy-gce-mig.sh status' to view progress"
}

# SSH to instance
ssh_instance() {
    INSTANCE=$(gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="value(instance)" \
        --limit=1)
    
    if [ -z "$INSTANCE" ]; then
        print_error "No running instances"
    fi
    
    print_info "Connecting to $INSTANCE..."
    gcloud compute ssh $INSTANCE --zone=$ZONE
}

# Delete all resources
delete_all() {
    print_header "Delete GCE MIG Resources"
    
    echo -e "${YELLOW}The following resources will be deleted:${NC}"
    echo "  - Forwarding Rule: $FORWARDING_RULE"
    echo "  - HTTP Proxy: $HTTP_PROXY"
    echo "  - URL Map: $URL_MAP"
    echo "  - Backend Service: $BACKEND_SERVICE"
    echo "  - Managed Instance Group: $INSTANCE_GROUP"
    echo "  - Health Check: $HEALTH_CHECK"
    echo "  - Instance Template: $INSTANCE_TEMPLATE"
    echo "  - Firewall Rules"
    echo ""
    read -p "Confirm deletion? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        print_info "Deleting Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        
        print_info "Deleting MIG..."
        gcloud compute instance-groups managed delete $INSTANCE_GROUP --zone=$ZONE --quiet 2>/dev/null || true
        
        print_info "Deleting other resources..."
        gcloud compute health-checks delete $HEALTH_CHECK --quiet 2>/dev/null || true
        gcloud compute instance-templates delete $INSTANCE_TEMPLATE --quiet 2>/dev/null || true
        gcloud compute firewall-rules delete $FIREWALL_RULE --quiet 2>/dev/null || true
        gcloud compute firewall-rules delete $FIREWALL_RULE_LB --quiet 2>/dev/null || true
        
        print_success "All resources deleted"
    else
        print_info "Cancelled"
    fi
}

# Main function
main() {
    print_header "SSE Gateway - GCE MIG Deployment"
    
    check_requirements
    enable_apis
    setup_pubsub
    build_image
    setup_firewall
    create_instance_template
    create_health_check
    create_mig
    create_load_balancer
    print_summary
}

# Start all services (Load Balancer, instances controlled via scale)
start_all() {
    print_header "Starting Services"
    
    # Check if image exists
    if ! image_exists; then
        print_error "Image does not exist, please run ./deploy-gce-mig.sh first to deploy"
    fi
    
    # Create Load Balancer (if not exists)
    if ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "Creating Load Balancer..."
        export SKIP_LB=false
        create_load_balancer
    else
        print_success "Load Balancer already exists"
    fi
    
    echo ""
    print_success "Services started"
    print_warning "Load Balancer takes 2-3 minutes to become effective"
    echo ""
    echo -e "  ${CYAN}Start instances:${NC} ${YELLOW}./deploy-gce-mig.sh scale 1${NC}"
    echo -e "  ${CYAN}View status:${NC} ${YELLOW}./deploy-gce-mig.sh status${NC}"
}

# Stop all services (instances + Load Balancer)
stop_all() {
    print_header "Stopping All Services"
    
    # Stop instances
    print_info "Stopping all instances..."
    gcloud compute instance-groups managed resize $INSTANCE_GROUP \
        --size=0 \
        --zone=$ZONE \
        --quiet 2>/dev/null || true
    print_success "Instances stopped"
    
    # Delete Load Balancer
    if gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "Deleting Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        print_success "Load Balancer deleted"
    else
        print_success "Load Balancer does not exist"
    fi
    
    echo ""
    print_success "All services stopped, current cost: \$0/month"
    echo ""
    echo -e "  ${CYAN}Restart:${NC} ${YELLOW}./deploy-gce-mig.sh start${NC}"
}

# Delete Load Balancer (keep instance group)
delete_lb() {
    print_header "Delete Load Balancer"
    
    if ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "Load Balancer does not exist"
        return 0
    fi
    
    echo -e "${YELLOW}The following Load Balancer resources will be deleted:${NC}"
    echo "  - Forwarding Rule: $FORWARDING_RULE"
    echo "  - HTTP Proxy: $HTTP_PROXY"
    echo "  - URL Map: $URL_MAP"
    echo "  - Backend Service: $BACKEND_SERVICE"
    echo ""
    echo -e "${GREEN}Keeping:${NC} MIG, instances, health check"
    echo ""
    read -p "Confirm deletion? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        print_info "Deleting Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        
        print_success "Load Balancer deleted, saving \$18/month"
        print_info "Service now accessible via instance IP:8080"
        print_info "Run './deploy-gce-mig.sh status' to view instance IP"
    else
        print_info "Cancelled"
    fi
}

# Fast deployment (skip build)
fast_deploy() {
    print_header "SSE Gateway - GCE MIG Fast Deployment"
    print_info "Skipping image build, using existing image"
    
    export SKIP_BUILD=true
    check_requirements
    enable_apis
    setup_pubsub
    build_image
    setup_firewall
    create_instance_template
    create_health_check
    create_mig
    create_load_balancer
    print_summary
}

# Secret management
add_secret() {
    local name=$1
    local value=$2
    
    if [ -z "$name" ] || [ -z "$value" ]; then
        echo ""
        echo "Usage: ./deploy-gce-mig.sh secret NAME VALUE"
        echo ""
        echo "Available secrets:"
        echo "  REDIS_URL          - Redis connection URL (for message replay)"
        echo ""
        echo "Example:"
        echo "  ./deploy-gce-mig.sh secret REDIS_URL redis://user:pass@host:6379"
        echo ""
        echo "Note: After adding/updating secrets, run 'update' to apply changes:"
        echo "  ./deploy-gce-mig.sh update"
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
    
    print_info "Run './deploy-gce-mig.sh update' to apply secret changes"
}

list_secrets() {
    print_header "Secrets List"
    gcloud secrets list --project=$PROJECT_ID --format="table(name,createTime)"
}

# Command handling
case "${1:-}" in
    status)
        check_requirements
        show_status
        ;;
    scale)
        check_requirements
        scale_mig "$2"
        ;;
    start)
        check_requirements
        start_all
        ;;
    stop)
        check_requirements
        stop_all
        ;;
    update)
        check_requirements
        update_mig
        ;;
    ssh)
        check_requirements
        ssh_instance
        ;;
    delete)
        check_requirements
        delete_all
        ;;
    secret)
        check_requirements
        add_secret "$2" "$3"
        ;;
    secrets)
        check_requirements
        list_secrets
        ;;
    *)
        main
        ;;
esac
