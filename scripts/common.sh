#!/bin/bash
# Common functions and configurations

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Basic config
PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
REGION=${REGION:-"asia-east1"}
ZONE=${ZONE:-"asia-east1-a"}

# Resource names
SERVICE_NAME="gateway-sse"
IMAGE_NAME="gcr.io/$PROJECT_ID/$SERVICE_NAME:latest"

# Pub/Sub
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

# Check requirements
check_requirements() {
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI not installed"
    fi
    print_success "gcloud CLI installed"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "Please set project: export PROJECT=your-project-id"
    fi
    print_success "Project: $PROJECT_ID"
    print_success "Region: $REGION"
    
    gcloud config set project $PROJECT_ID --quiet
}

# Enable APIs
enable_apis() {
    local mode=$1
    print_step "2" "Enabling Google Cloud APIs"
    
    local apis=(
        "cloudbuild.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    [ "$mode" = "cloudrun" ] && apis+=("run.googleapis.com")
    [ "$mode" = "gce" ] && apis+=("compute.googleapis.com")
    
    for api in "${apis[@]}"; do
        if gcloud services list --enabled --filter="name:$api" --format="value(name)" 2>/dev/null | grep -q "$api"; then
            print_success "$api (already enabled)"
        else
            gcloud services enable $api --quiet 2>/dev/null
            print_success "$api (newly enabled)"
        fi
    done
    
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    gcloud projects add-iam-policy-binding $PROJECT_ID \
        --member="serviceAccount:${SERVICE_ACCOUNT}" \
        --role="roles/secretmanager.secretAccessor" \
        --quiet 2>/dev/null || true
    print_success "Secret Manager permissions configured"
}

# Setup Pub/Sub
setup_pubsub() {
    print_step "3" "Configuring Pub/Sub"
    
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

# Image functions
image_exists() {
    gcloud container images describe "$IMAGE_NAME" &>/dev/null
}

build_image() {
    print_step "4" "Building container image"
    
    if [ "${SKIP_BUILD:-false}" = "true" ] && image_exists; then
        print_success "Skipping build, using existing image"
        return 0
    fi
    
    if image_exists; then
        print_info "Image already exists: $IMAGE_NAME"
        read -p "  Rebuild? (y/N): " rebuild
        [ "$rebuild" != "y" ] && [ "$rebuild" != "Y" ] && { print_success "Using existing image"; return 0; }
    fi
    
    print_info "Building with Cloud Build..."
    gcloud builds submit --tag "$IMAGE_NAME" --quiet .
    print_success "Image built: $IMAGE_NAME"
}

# Secret functions
secret_exists() {
    gcloud secrets describe "$1" --project=$PROJECT_ID &>/dev/null 2>&1
}

get_secret_value() {
    gcloud secrets versions access latest --secret="$1" --project=$PROJECT_ID 2>/dev/null
}

add_secret() {
    local name=$1 value=$2
    
    if [ -z "$name" ] || [ -z "$value" ]; then
        cat << EOF

Usage: ./deploy.sh secret NAME VALUE

Available secrets:
  REDIS_URL           - Redis connection URL
  GCP_PROJECT         - GCP Project ID
  PUBSUB_SUBSCRIPTION - Pub/Sub subscription name

Example:
  ./deploy.sh secret REDIS_URL redis://user:pass@host:6379

EOF
        exit 1
    fi
    
    if gcloud secrets describe $name --project=$PROJECT_ID &>/dev/null; then
        echo -n "$value" | gcloud secrets versions add $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' updated"
    else
        echo -n "$value" | gcloud secrets create $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' created"
    fi
}

list_secrets() {
    print_header "Secrets List"
    gcloud secrets list --project=$PROJECT_ID --format="table(name,createTime)"
}
