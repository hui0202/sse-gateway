#!/bin/bash

# ============================================
# SSE Gateway - Cloud Run 部署脚本
# ============================================
#
# 适用场景:
#   - 流量波动大，需要快速扩缩容
#   - 并发需求 < 50,000 (50 实例 x 1000)
#   - 希望按用量付费，无流量时降到最低成本
#
# 用法:
#   ./deploy-cloudrun.sh                    # 部署服务
#   ./deploy-cloudrun.sh secret NAME VALUE  # 添加/更新 Secret
#   ./deploy-cloudrun.sh secrets            # 列出所有 Secrets
#   ./deploy-cloudrun.sh delete             # 删除服务
#
# 环境变量:
#   PROJECT             - GCP 项目 ID（必需，或使用 gcloud 默认项目）
#   REGION              - 部署区域（默认 asia-east1）
#   MAX_INSTANCES       - 最大实例数（默认 50，支持 50,000 并发）
#   MIN_INSTANCES       - 最小实例数（默认 1）
#   REDIS_URL           - Redis 连接 URL（可选）
#   ENABLE_DASHBOARD    - 是否启用 Dashboard（默认 true）

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# 配置
PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
REGION=${REGION:-"asia-east1"}
SERVICE_NAME="gateway-sse"
IMAGE_NAME="gcr.io/$PROJECT_ID/$SERVICE_NAME"

# Cloud Run 配置
MAX_INSTANCES=${MAX_INSTANCES:-50}
MIN_INSTANCES=${MIN_INSTANCES:-1}
MEMORY=${MEMORY:-"1Gi"}
CPU=${CPU:-"2"}
CONCURRENCY=${CONCURRENCY:-1000}

# Pub/Sub 配置
TOPIC_NAME="gateway-subscription"
SUBSCRIPTION_NAME="gateway-subscription-sub"

# 打印函数
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

# 检查环境
check_requirements() {
    print_step "1/6" "检查环境"
    
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI 未安装"
    fi
    print_success "gcloud CLI 已安装"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "请设置项目: export PROJECT=your-project-id"
    fi
    print_success "项目: $PROJECT_ID"
    print_success "区域: $REGION"
    print_info "最大实例: $MAX_INSTANCES (支持 $((MAX_INSTANCES * CONCURRENCY)) 并发)"
    
    gcloud config set project $PROJECT_ID --quiet
}

# 启用 API
enable_apis() {
    print_step "2/6" "启用 Google Cloud APIs"
    
    apis=(
        "cloudbuild.googleapis.com"
        "run.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    for api in "${apis[@]}"; do
        if gcloud services list --enabled --filter="name:$api" --format="value(name)" 2>/dev/null | grep -q "$api"; then
            print_success "$api (已启用)"
        else
            gcloud services enable $api --quiet 2>/dev/null
            print_success "$api (新启用)"
        fi
    done
    
    # 授予 Secret Manager 权限
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    gcloud projects add-iam-policy-binding $PROJECT_ID \
        --member="serviceAccount:${SERVICE_ACCOUNT}" \
        --role="roles/secretmanager.secretAccessor" \
        --quiet 2>/dev/null || true
    print_success "Secret Manager 权限已配置"
}

# 配置 Pub/Sub
setup_pubsub() {
    print_step "3/6" "配置 Pub/Sub"
    
    if gcloud pubsub topics describe $TOPIC_NAME &>/dev/null; then
        print_success "Topic: $TOPIC_NAME (已存在)"
    else
        gcloud pubsub topics create $TOPIC_NAME
        print_success "Topic: $TOPIC_NAME (已创建)"
    fi
    
    if gcloud pubsub subscriptions describe $SUBSCRIPTION_NAME &>/dev/null; then
        print_success "Subscription: $SUBSCRIPTION_NAME (已存在)"
    else
        gcloud pubsub subscriptions create $SUBSCRIPTION_NAME \
            --topic=$TOPIC_NAME \
            --ack-deadline=10 \
            --message-retention-duration=1h
        print_success "Subscription: $SUBSCRIPTION_NAME (已创建)"
    fi
}

# 构建镜像
build_image() {
    print_step "4/6" "构建容器镜像"
    print_info "使用 Cloud Build 构建..."
    
    gcloud builds submit \
        --tag "$IMAGE_NAME:latest" \
        --quiet \
        .
    
    print_success "镜像构建完成: $IMAGE_NAME:latest"
}

# 部署服务
deploy_service() {
    print_step "5/6" "部署到 Cloud Run"
    
    # 基础环境变量
    ENV_VARS="RUST_LOG=info,GCP_PROJECT=$PROJECT_ID,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
    
    # 可选: Redis
    if [ -n "$REDIS_URL" ]; then
        ENV_VARS="$ENV_VARS,REDIS_URL=$REDIS_URL"
        print_info "Redis: 已配置"
    fi
    
    # 可选: Dashboard
    if [ "${ENABLE_DASHBOARD:-true}" = "false" ]; then
        ENV_VARS="$ENV_VARS,ENABLE_DASHBOARD=false"
        print_info "Dashboard: 已禁用"
    fi
    
    gcloud run deploy $SERVICE_NAME \
        --image "$IMAGE_NAME:latest" \
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
        --set-env-vars "$ENV_VARS" \
        --quiet
    
    print_success "Cloud Run 部署完成"
}

# 测试服务
test_service() {
    print_step "6/6" "验证部署"
    
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    
    sleep 3
    
    if curl -sk "$SERVICE_URL/health" 2>/dev/null | grep -q "OK"; then
        print_success "健康检查通过"
    else
        print_warning "健康检查失败，服务可能还在启动"
    fi
}

# 打印总结
print_summary() {
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    
    print_header "Cloud Run 部署完成"
    echo ""
    echo -e "  服务地址:     ${GREEN}$SERVICE_URL${NC}"
    echo -e "  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}"
    echo -e "  SSE 连接:     ${GREEN}$SERVICE_URL/sse/connect?channel_id=YOUR_CHANNEL${NC}"
    echo ""
    echo -e "  ${CYAN}配置信息:${NC}"
    echo -e "    实例范围:   $MIN_INSTANCES - $MAX_INSTANCES"
    echo -e "    每实例并发: $CONCURRENCY"
    echo -e "    最大并发:   $((MAX_INSTANCES * CONCURRENCY))"
    echo -e "    资源:       $CPU vCPU / $MEMORY"
    echo ""
    echo -e "  ${CYAN}测试命令:${NC}"
    echo -e "  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{\"msg\":\"Hello!\"}' \\
    --attribute=channel_id=test,event_type=notification${NC}"
    echo ""
}

# 删除服务
delete_service() {
    print_header "删除 Cloud Run 服务"
    
    echo -e "${YELLOW}即将删除以下资源:${NC}"
    echo "  - Cloud Run 服务: $SERVICE_NAME"
    echo ""
    read -p "确认删除? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        gcloud run services delete $SERVICE_NAME --region $REGION --quiet
        print_success "服务已删除"
    else
        print_info "已取消"
    fi
}

# Secret 管理
add_secret() {
    local name=$1
    local value=$2
    
    if [ -z "$name" ] || [ -z "$value" ]; then
        print_error "用法: ./deploy-cloudrun.sh secret NAME VALUE"
    fi
    
    if gcloud secrets describe $name --project=$PROJECT_ID &>/dev/null; then
        echo -n "$value" | gcloud secrets versions add $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' 已更新"
    else
        echo -n "$value" | gcloud secrets create $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' 已创建"
    fi
}

list_secrets() {
    print_header "Secrets 列表"
    gcloud secrets list --project=$PROJECT_ID --format="table(name,createTime)"
}

# 主函数
main() {
    print_header "SSE Gateway - Cloud Run 部署"
    
    check_requirements
    enable_apis
    setup_pubsub
    build_image
    deploy_service
    test_service
    print_summary
}

# 命令处理
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
