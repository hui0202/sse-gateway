#!/bin/bash

# SSE Gateway 一键部署脚本
#
# 用法:
#   ./deploy.sh                    # 部署服务
#   ./deploy.sh secret NAME VALUE  # 添加/更新 Secret
#   ./deploy.sh secrets            # 列出所有 Secrets
#
# 示例:
#   PROJECT=my-project ./deploy.sh
#   ./deploy.sh secret REDIS_URL redis://localhost:6379
#   ./deploy.sh secrets
#
# 环境变量:
#   PROJECT             - GCP 项目 ID（必需，或使用 gcloud 默认项目）
#   REGION              - 部署区域（默认 asia-east1）
#   REDIS_URL           - Redis 连接 URL（可选，用于消息重放）
#   ENABLE_DASHBOARD    - 是否启用 Dashboard（默认 true）
#   USE_SECRETS         - 是否使用 Secret Manager（默认 true）

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 配置（全部从环境变量读取）
PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
REGION=${REGION:-"asia-east1"}
REDIS_URL=${REDIS_URL:-""}
SERVICE_NAME="gateway-sse"
TOPIC_NAME="gateway-subscription"
SUBSCRIPTION_NAME="gateway-subscription-sub"

# 打印函数
print_step() { echo -e "\n${BLUE}[Step $1]${NC} $2"; }
print_success() { echo -e "${GREEN}✓${NC} $1"; }
print_warning() { echo -e "${YELLOW}!${NC} $1"; }
print_error() { echo -e "${RED}✗${NC} $1"; exit 1; }

# 检查必要条件
check_requirements() {
    print_step "0" "检查环境..."
    
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI 未安装"
    fi
    print_success "gcloud CLI 已安装"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "请指定项目 ID: ./deploy.sh <PROJECT_ID>"
    fi
    print_success "项目: $PROJECT_ID"
    print_success "区域: $REGION"
    
    # 检查 Redis 配置（环境变量或 Secret）
    if [ -n "$REDIS_URL" ]; then
        print_success "Redis: 环境变量已配置"
    elif gcloud secrets describe REDIS_URL --project=$PROJECT_ID &>/dev/null 2>&1; then
        print_success "Redis: Secret 已配置"
    else
        print_warning "Redis: 未配置 (消息重放禁用)"
    fi
    
    # 设置项目
    gcloud config set project $PROJECT_ID --quiet
}

# 启用必要的 API
enable_apis() {
    print_step "1" "启用 Google Cloud APIs..."
    
    apis=(
        "cloudbuild.googleapis.com"
        "run.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    for api in "${apis[@]}"; do
        gcloud services enable $api --quiet 2>/dev/null && print_success "已启用 $api" || print_warning "$api 已启用"
    done
    
    # 授予 Cloud Run 服务账号访问 Secret Manager 的权限
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    gcloud projects add-iam-policy-binding $PROJECT_ID \
        --member="serviceAccount:${SERVICE_ACCOUNT}" \
        --role="roles/secretmanager.secretAccessor" \
        --quiet 2>/dev/null && print_success "已授权 Secret Manager 访问权限" || print_warning "Secret Manager 权限已存在"
}

# 创建 Pub/Sub Topic 和 Subscription
setup_pubsub() {
    print_step "2" "配置 Pub/Sub..."
    
    # 创建 Topic
    if gcloud pubsub topics describe $TOPIC_NAME &>/dev/null; then
        print_warning "Topic $TOPIC_NAME 已存在"
    else
        gcloud pubsub topics create $TOPIC_NAME && print_success "创建 Topic: $TOPIC_NAME"
    fi
    
    # 创建 Subscription
    if gcloud pubsub subscriptions describe $SUBSCRIPTION_NAME &>/dev/null; then
        print_warning "Subscription $SUBSCRIPTION_NAME 已存在"
    else
        gcloud pubsub subscriptions create $SUBSCRIPTION_NAME \
            --topic=$TOPIC_NAME \
            --ack-deadline=10 \
            --message-retention-duration=1h \
            && print_success "创建 Subscription: $SUBSCRIPTION_NAME"
    fi
}

# 构建镜像
build_image() {
    IMAGE_TAG="gcr.io/$PROJECT_ID/$SERVICE_NAME"
    
    print_step "3" "构建容器镜像 (Cloud Build)..."
    
    # 使用内联 cloudbuild 配置，支持缓存
    gcloud builds submit \
        --config /dev/stdin \
        --quiet \
        . << 'CLOUDBUILD_EOF'
steps:
  - name: 'gcr.io/cloud-builders/docker'
    entrypoint: 'bash'
    args: ['-c', 'docker pull gcr.io/$PROJECT_ID/gateway-sse:latest || exit 0']
  - name: 'gcr.io/cloud-builders/docker'
    args:
      - 'build'
      - '--cache-from'
      - 'gcr.io/$PROJECT_ID/gateway-sse:latest'
      - '-t'
      - 'gcr.io/$PROJECT_ID/gateway-sse:latest'
      - '.'
images:
  - 'gcr.io/$PROJECT_ID/gateway-sse:latest'
options:
  machineType: 'E2_HIGHCPU_8'
CLOUDBUILD_EOF
    
    print_success "镜像构建完成: $IMAGE_TAG"
}

# 部署到 Cloud Run
deploy_cloudrun() {
    print_step "4" "部署到 Cloud Run..."
    
    # 检查是否使用 YAML 配置文件部署
    USE_YAML=${USE_YAML:-"true"}
    
    if [ "$USE_YAML" = "true" ] && [ -f "cloudrun-service.yaml" ]; then
        print_success "使用 YAML 配置文件部署"
        
        # 生成临时配置文件，替换变量
        TEMP_YAML=$(mktemp)
        sed -e "s|PROJECT_ID|$PROJECT_ID|g" \
            -e "s|asia-east1|$REGION|g" \
            cloudrun-service.yaml > "$TEMP_YAML"
        
        # 使用 YAML 文件部署
        gcloud run services replace "$TEMP_YAML" \
            --region $REGION \
            --quiet
        
        # 设置公开访问
        gcloud run services add-iam-policy-binding $SERVICE_NAME \
            --region $REGION \
            --member="allUsers" \
            --role="roles/run.invoker" \
            --quiet 2>/dev/null || true
        
        rm -f "$TEMP_YAML"
    else
        # 回退到命令行参数方式
        USE_SECRETS=${USE_SECRETS:-"true"}
        
        if [ "$USE_SECRETS" = "true" ]; then
            print_success "使用 Secret Manager 管理敏感配置"
            
            ENV_VARS="RUST_LOG=info"
            SECRETS="GCP_PROJECT=GCP_PROJECT:latest,PUBSUB_SUBSCRIPTION=PUBSUB_SUBSCRIPTION:latest"
            
            if gcloud secrets describe REDIS_URL --project=$PROJECT_ID &>/dev/null; then
                SECRETS="$SECRETS,REDIS_URL=REDIS_URL:latest"
                print_success "Redis Secret 已配置"
            elif [ -n "$REDIS_URL" ]; then
                ENV_VARS="$ENV_VARS,REDIS_URL=$REDIS_URL"
                print_success "Redis 通过环境变量配置"
            else
                print_warning "Redis 未配置 - 消息重放功能禁用"
            fi
            
            gcloud run deploy $SERVICE_NAME \
                --image gcr.io/$PROJECT_ID/$SERVICE_NAME \
                --region $REGION \
                --platform managed \
                --allow-unauthenticated \
                --min-instances 1 \
                --max-instances 50 \
                --memory 1Gi \
                --cpu 2 \
                --concurrency 1000 \
                --timeout 3600s \
                --no-cpu-throttling \
                --session-affinity \
                --set-env-vars "$ENV_VARS" \
                --set-secrets "$SECRETS" \
                --quiet
        else
            print_warning "不使用 Secret Manager，直接传递环境变量"
            
            ENV_VARS="RUST_LOG=info,GCP_PROJECT=$PROJECT_ID,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
            if [ -n "$REDIS_URL" ]; then
                ENV_VARS="$ENV_VARS,REDIS_URL=$REDIS_URL"
            fi
            
            gcloud run deploy $SERVICE_NAME \
                --image gcr.io/$PROJECT_ID/$SERVICE_NAME \
                --region $REGION \
                --platform managed \
                --allow-unauthenticated \
                --min-instances 1 \
                --max-instances 50 \
                --memory 1Gi \
                --cpu 2 \
                --concurrency 1000 \
                --timeout 3600s \
                --no-cpu-throttling \
                --session-affinity \
                --set-env-vars "$ENV_VARS" \
                --quiet
        fi
    fi
    
    print_success "Cloud Run 部署完成"
}

# 获取服务 URL
get_service_url() {
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    echo "$SERVICE_URL"
}

# 测试服务
test_service() {
    print_step "5" "测试服务..."
    
    SERVICE_URL=$(get_service_url)
    
    # 健康检查
    if curl -sk "$SERVICE_URL/health" | grep -q "OK"; then
        print_success "健康检查通过"
    else
        print_warning "健康检查失败，服务可能还在启动中"
    fi
}

# 打印总结
print_summary() {
    SERVICE_URL=$(get_service_url)
    
    echo ""
    echo -e "${GREEN}=========================================="
    echo "          部署完成!"
    echo "==========================================${NC}"
    echo ""
    echo -e "服务地址:   ${BLUE}$SERVICE_URL${NC}"
    echo -e "Dashboard:  ${BLUE}$SERVICE_URL/dashboard${NC}"
    echo -e "SSE 连接:   ${BLUE}$SERVICE_URL/sse/connect?channel_id=YOUR_CHANNEL${NC}"
    echo ""
    echo "发送测试消息:"
    echo -e "${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
  --message='{\"msg\":\"Hello!\"}' \\
  --attribute=channel_id=test,event_type=notification${NC}"
    echo ""
}

# 添加/更新 Secret
add_secret() {
    local name=$1
    local value=$2
    
    if [ -z "$name" ] || [ -z "$value" ]; then
        print_error "用法: ./deploy.sh secret NAME VALUE"
    fi
    
    PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
    
    echo -e "${BLUE}添加 Secret: ${name}${NC}"
    
    # 检查是否已存在
    if gcloud secrets describe $name --project=$PROJECT_ID &>/dev/null; then
        # 更新
        echo -n "$value" | gcloud secrets versions add $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' 已更新"
    else
        # 创建
        echo -n "$value" | gcloud secrets create $name --data-file=- --project=$PROJECT_ID
        print_success "Secret '$name' 已创建"
    fi
}

# 列出所有 Secrets
list_secrets() {
    PROJECT_ID=${PROJECT:-$(gcloud config get-value project 2>/dev/null)}
    
    echo -e "${BLUE}项目 ${PROJECT_ID} 的 Secrets:${NC}"
    echo ""
    gcloud secrets list --project=$PROJECT_ID --format="table(name,createTime)"
}

# 主函数
main() {
    echo -e "${GREEN}"
    echo "╔═══════════════════════════════════════╗"
    echo "║     SSE Gateway 一键部署              ║"
    echo "╚═══════════════════════════════════════╝"
    echo -e "${NC}"
    
    check_requirements
    enable_apis
    setup_pubsub
    build_image
    deploy_cloudrun
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
    *)
        main
        ;;
esac
