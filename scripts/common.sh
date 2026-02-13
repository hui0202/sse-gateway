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
# Secrets 使用服务名前缀，避免多服务冲突
SECRET_PREFIX="${SERVICE_NAME}"

get_full_secret_name() {
    echo "${SECRET_PREFIX}-$1"
}

secret_exists() {
    local full_name=$(get_full_secret_name "$1")
    gcloud secrets describe "$full_name" --project=$PROJECT_ID &>/dev/null 2>&1
}

get_secret_value() {
    local full_name=$(get_full_secret_name "$1")
    gcloud secrets versions access latest --secret="$full_name" --project=$PROJECT_ID 2>/dev/null
}

add_secret() {
    local name=$1 value=$2
    local full_name=$(get_full_secret_name "$name")
    
    if [ -z "$name" ]; then
        cat << EOF

Usage: ./deploy.sh secret NAME [VALUE]

If VALUE is omitted, you'll be prompted to enter it securely (hidden input).

Available secrets:
  REDIS_URL           - Redis connection URL
  GCP_PROJECT         - GCP Project ID

Example:
  ./deploy.sh secret REDIS_URL                    # Interactive (secure)
  ./deploy.sh secret REDIS_URL redis://host:6379  # Direct (visible in history)

Secrets are prefixed with service name: ${SECRET_PREFIX}-NAME

EOF
        exit 1
    fi
    
    # 如果没提供 value，交互式读取（不显示输入）
    if [ -z "$value" ]; then
        read -s -p "Enter value for $name: " value
        echo ""
        if [ -z "$value" ]; then
            print_error "Value cannot be empty"
        fi
    fi
    
    if gcloud secrets describe "$full_name" --project=$PROJECT_ID &>/dev/null; then
        print_warning "Secret '$full_name' already exists"
        read -p "  Overwrite with new version? (y/N): " confirm
        if [ "$confirm" != "y" ] && [ "$confirm" != "Y" ]; then
            echo "  Cancelled"
            return 0
        fi
        printf '%s' "$value" | gcloud secrets versions add "$full_name" --data-file=- --project=$PROJECT_ID
        print_success "Secret '$full_name' updated (new version added)"
    else
        printf '%s' "$value" | gcloud secrets create "$full_name" \
            --data-file=- \
            --labels="service=${SERVICE_NAME}" \
            --project=$PROJECT_ID
        print_success "Secret '$full_name' created"
    fi
}

list_secrets() {
    print_header "Secrets for ${SERVICE_NAME}"
    echo ""
    print_info "Showing secrets with prefix: ${SECRET_PREFIX}-*"
    echo ""
    gcloud secrets list \
        --project=$PROJECT_ID \
        --filter="name ~ ^projects/.*/secrets/${SECRET_PREFIX}-" \
        --format="table(name.basename(),createTime.date(),labels)"
    
    echo ""
    print_info "To view a secret value: gcloud secrets versions access latest --secret=SECRET_NAME"
}

get_all_secret_names() {
    gcloud secrets list \
        --project=$PROJECT_ID \
        --filter="name ~ ^projects/.*/secrets/${SECRET_PREFIX}-" \
        --format="value(name)" | \
    while read full_path; do
        basename="${full_path##*/}"
        echo "${basename#${SECRET_PREFIX}-}"
    done
}

# 格式: ENV_VAR=full-secret-name:latest,ENV_VAR2=full-secret-name2:latest
generate_cloudrun_secrets() {
    local secrets=""
    for name in $(get_all_secret_names); do
        local full_name=$(get_full_secret_name "$name")
        [ -n "$secrets" ] && secrets="$secrets,"
        secrets="${secrets}${name}=${full_name}:latest"
        echo -e "  ${CYAN}→${NC} $name: from Secret Manager" >&2
    done
    echo "$secrets"
}

# 格式: KEY1=value1,KEY2=value2
generate_gce_secrets_env() {
    local env_vars=""
    for name in $(get_all_secret_names); do
        local value=$(get_secret_value "$name")
        [ -n "$env_vars" ] && env_vars="$env_vars,"
        env_vars="${env_vars}${name}=${value}"
        # 输出到 stderr，避免被 $() 捕获
        echo -e "  ${CYAN}→${NC} $name: from Secret Manager" >&2
    done
    echo "$env_vars"
}
