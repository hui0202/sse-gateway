#!/bin/bash

# ============================================
# SSE Gateway - GCE Managed Instance Group 部署脚本
# ============================================
#
# 适用场景:
#   - 高并发长连接 (50,000+ 并发)
#   - 成本敏感（比 Cloud Run 便宜 20x）
#   - 流量相对稳定
#
# 架构:
#   Load Balancer → Managed Instance Group (自动扩缩) → SSE Gateway
#
# 用法:
#   ./deploy-gce-mig.sh                # 首次部署（构建镜像 + 创建资源）
#   ./deploy-gce-mig.sh start          # 启动所有服务（实例 + LB）
#   ./deploy-gce-mig.sh stop           # 停止所有服务（费用归零）
#   ./deploy-gce-mig.sh scale N        # 快速扩缩实例数量
#   ./deploy-gce-mig.sh status         # 查看状态
#   ./deploy-gce-mig.sh update         # 更新代码（重新构建 + 滚动更新）
#   ./deploy-gce-mig.sh ssh            # SSH 到实例调试
#   ./deploy-gce-mig.sh delete         # 删除所有资源
#
# 环境变量:
#   SKIP_BUILD=true     - 跳过镜像构建，使用已有镜像
#   SKIP_LB=true        - 跳过 Load Balancer（省 $18/月，通过实例 IP 直连）
#
# 环境变量:
#   PROJECT             - GCP 项目 ID
#   REGION              - 部署区域（默认 asia-east1）
#   ZONE                - 部署可用区（默认 asia-east1-a）
#   MACHINE_TYPE        - 机器类型（默认 e2-micro，最便宜 ~$7/月）
#   INIT_SIZE           - 初始实例数（默认 0）

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
ZONE=${ZONE:-"asia-east1-a"}

# 资源名称
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

# 机器配置
MACHINE_TYPE=${MACHINE_TYPE:-"e2-micro"}  # 0.25 vCPU, 1GB RAM (~$7/月)
INIT_SIZE=${INIT_SIZE:-0}                 # 初始实例数（默认 0）

# 容器镜像
IMAGE_NAME="gcr.io/$PROJECT_ID/$SERVICE_NAME:latest"

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
    print_step "1/9" "检查环境"
    
    if ! command -v gcloud &> /dev/null; then
        print_error "gcloud CLI 未安装"
    fi
    print_success "gcloud CLI 已安装"
    
    if [ -z "$PROJECT_ID" ]; then
        print_error "请设置项目: export PROJECT=your-project-id"
    fi
    print_success "项目: $PROJECT_ID"
    print_success "区域: $REGION"
    print_success "可用区: $ZONE"
    print_info "机器类型: $MACHINE_TYPE (~\$7/月)"
    print_info "初始实例: $INIT_SIZE（手动控制）"
    
    gcloud config set project $PROJECT_ID --quiet
}

# 启用 API
enable_apis() {
    print_step "2/9" "启用 Google Cloud APIs"
    
    apis=(
        "compute.googleapis.com"
        "cloudbuild.googleapis.com"
        "pubsub.googleapis.com"
        "containerregistry.googleapis.com"
        "secretmanager.googleapis.com"
    )
    
    for api in "${apis[@]}"; do
        if gcloud services list --enabled --filter="name:$api" --format="value(name)" 2>/dev/null | grep -q "$api"; then
            print_success "$api (已启用)"
        else
            gcloud services enable $api --quiet
            print_success "$api (新启用)"
        fi
    done
}

# 配置 Pub/Sub
setup_pubsub() {
    print_step "3/9" "配置 Pub/Sub"
    
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

# 检查镜像是否存在
image_exists() {
    gcloud container images describe "$IMAGE_NAME" &>/dev/null
}

# 构建镜像
build_image() {
    print_step "4/9" "构建容器镜像"
    
    # 检查是否跳过构建
    if [ "${SKIP_BUILD:-false}" = "true" ]; then
        if image_exists; then
            print_success "跳过构建，使用已有镜像: $IMAGE_NAME"
            return 0
        else
            print_warning "镜像不存在，必须构建"
        fi
    fi
    
    # 检查镜像是否已存在
    if image_exists; then
        print_info "镜像已存在: $IMAGE_NAME"
        read -p "  是否重新构建? (y/N): " rebuild
        if [ "$rebuild" != "y" ] && [ "$rebuild" != "Y" ]; then
            print_success "使用已有镜像"
            return 0
        fi
    fi
    
    print_info "使用 Cloud Build 构建..."
    
    gcloud builds submit \
        --tag "$IMAGE_NAME" \
        --quiet \
        .
    
    print_success "镜像构建完成: $IMAGE_NAME"
}

# 配置防火墙
setup_firewall() {
    print_step "5/9" "配置防火墙规则"
    
    # 允许健康检查
    if gcloud compute firewall-rules describe $FIREWALL_RULE &>/dev/null; then
        print_success "健康检查防火墙规则 (已存在)"
    else
        gcloud compute firewall-rules create $FIREWALL_RULE \
            --network=default \
            --action=allow \
            --direction=ingress \
            --source-ranges=130.211.0.0/22,35.191.0.0/16 \
            --target-tags=$SERVICE_NAME \
            --rules=tcp:8080 \
            --quiet
        print_success "健康检查防火墙规则 (已创建)"
    fi
    
    # 允许 Load Balancer 流量
    if gcloud compute firewall-rules describe $FIREWALL_RULE_LB &>/dev/null; then
        print_success "Load Balancer 防火墙规则 (已存在)"
    else
        gcloud compute firewall-rules create $FIREWALL_RULE_LB \
            --network=default \
            --action=allow \
            --direction=ingress \
            --source-ranges=0.0.0.0/0 \
            --target-tags=$SERVICE_NAME \
            --rules=tcp:8080 \
            --quiet
        print_success "Load Balancer 防火墙规则 (已创建)"
    fi
}

# 创建 Instance Template
create_instance_template() {
    print_step "6/9" "创建 Instance Template"
    
    # 删除旧模板（如果存在）
    if gcloud compute instance-templates describe $INSTANCE_TEMPLATE &>/dev/null; then
        print_warning "删除旧模板..."
        # 模板名加时间戳以支持滚动更新
        INSTANCE_TEMPLATE="${SERVICE_NAME}-template-$(date +%Y%m%d%H%M%S)"
    fi
    
    # 获取服务账号
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    # 创建启动脚本
    STARTUP_SCRIPT=$(cat <<'STARTUP_EOF'
#!/bin/bash
# 增加文件描述符限制以支持高并发
echo "* soft nofile 65535" >> /etc/security/limits.conf
echo "* hard nofile 65535" >> /etc/security/limits.conf
ulimit -n 65535

# 优化网络参数
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
        --container-env="RUST_LOG=info,GCP_PROJECT=$PROJECT_ID,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME" \
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

# 创建健康检查
create_health_check() {
    print_step "7/9" "创建健康检查"
    
    if gcloud compute health-checks describe $HEALTH_CHECK &>/dev/null; then
        print_success "健康检查 (已存在)"
    else
        gcloud compute health-checks create http $HEALTH_CHECK \
            --port=8080 \
            --request-path=/health \
            --check-interval=10s \
            --timeout=5s \
            --healthy-threshold=2 \
            --unhealthy-threshold=3 \
            --quiet
        print_success "健康检查 (已创建)"
    fi
}

# 创建 Managed Instance Group
create_mig() {
    print_step "8/9" "创建 Managed Instance Group"
    
    if gcloud compute instance-groups managed describe $INSTANCE_GROUP --zone=$ZONE &>/dev/null; then
        print_warning "MIG 已存在，更新模板..."
        gcloud compute instance-groups managed set-instance-template $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE \
            --zone=$ZONE \
            --quiet
        
        # 触发滚动更新
        gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
            --version=template=$INSTANCE_TEMPLATE \
            --zone=$ZONE \
            --max-surge=1 \
            --max-unavailable=0 \
            --quiet
        print_success "MIG 滚动更新已启动"
    else
        # 创建 MIG
        gcloud compute instance-groups managed create $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE \
            --size=$INIT_SIZE \
            --zone=$ZONE \
            --health-check=$HEALTH_CHECK \
            --initial-delay=120 \
            --quiet
        print_success "MIG: $INSTANCE_GROUP (已创建)"
        print_success "手动控制模式（用 scale 命令调整实例数）"
    fi
    
    # 设置命名端口
    gcloud compute instance-groups managed set-named-ports $INSTANCE_GROUP \
        --named-ports=http:8080 \
        --zone=$ZONE \
        --quiet
}

# 创建 Load Balancer
create_load_balancer() {
    # 检查是否跳过 LB
    if [ "${SKIP_LB:-false}" = "true" ]; then
        print_step "9/9" "跳过 Load Balancer"
        print_success "Load Balancer 已跳过（省 \$18/月）"
        print_warning "需要通过实例 IP:8080 直接访问"
        return 0
    fi
    
    print_step "9/9" "创建 Load Balancer"
    
    # 创建 Backend Service
    if gcloud compute backend-services describe $BACKEND_SERVICE --global &>/dev/null; then
        print_success "Backend Service (已存在)"
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
        print_success "Backend Service (已创建)"
    fi
    
    # 添加 MIG 到 Backend Service
    gcloud compute backend-services add-backend $BACKEND_SERVICE \
        --instance-group=$INSTANCE_GROUP \
        --instance-group-zone=$ZONE \
        --balancing-mode=UTILIZATION \
        --max-utilization=0.8 \
        --global \
        --quiet 2>/dev/null || true
    print_success "Backend 已关联"
    
    # 创建 URL Map
    if gcloud compute url-maps describe $URL_MAP &>/dev/null; then
        print_success "URL Map (已存在)"
    else
        gcloud compute url-maps create $URL_MAP \
            --default-service=$BACKEND_SERVICE \
            --quiet
        print_success "URL Map (已创建)"
    fi
    
    # 创建 HTTP Proxy
    if gcloud compute target-http-proxies describe $HTTP_PROXY &>/dev/null; then
        print_success "HTTP Proxy (已存在)"
    else
        gcloud compute target-http-proxies create $HTTP_PROXY \
            --url-map=$URL_MAP \
            --quiet
        print_success "HTTP Proxy (已创建)"
    fi
    
    # 创建 Forwarding Rule（获取外部 IP）
    if gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null; then
        print_success "Forwarding Rule (已存在)"
    else
        gcloud compute forwarding-rules create $FORWARDING_RULE \
            --global \
            --target-http-proxy=$HTTP_PROXY \
            --ports=80 \
            --quiet
        print_success "Forwarding Rule (已创建)"
    fi
}

# 获取实例 IP
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

# 获取服务 URL
get_service_url() {
    # 先尝试获取 LB IP
    LB_IP=$(gcloud compute forwarding-rules describe $FORWARDING_RULE --global --format='value(IPAddress)' 2>/dev/null)
    
    if [ -n "$LB_IP" ]; then
        echo "http://$LB_IP"
    else
        # 没有 LB，获取实例 IP
        INSTANCE_IP=$(get_instance_ip)
        if [ -n "$INSTANCE_IP" ]; then
            echo "http://$INSTANCE_IP:8080"
        else
            echo "(实例未运行，启动后运行 ./deploy-gce-mig.sh status 查看)"
        fi
    fi
}

# 打印总结
print_summary() {
    SERVICE_URL=$(get_service_url)
    
    print_header "GCE MIG 部署完成"
    echo ""
    echo -e "  服务地址:     ${GREEN}$SERVICE_URL${NC}"
    echo -e "  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}"
    echo -e "  SSE 连接:     ${GREEN}$SERVICE_URL/sse/connect?channel_id=YOUR_CHANNEL${NC}"
    echo ""
    echo -e "  ${CYAN}配置信息:${NC}"
    echo -e "    机器类型:   $MACHINE_TYPE (~\$7/月/台)"
    echo -e "    控制模式:   手动（用 scale 命令）"
    echo ""
    echo -e "  ${CYAN}成本估算:${NC}"
    echo -e "    每台实例:   ~\$7/月"
    # 检查是否有 Load Balancer
    if [ "${SKIP_LB:-false}" = "true" ] || ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        echo -e "    Load Balancer: ${GREEN}已跳过 (\$0)${NC}"
    else
        echo -e "    Load Balancer ≈ \$18/月"
    fi
    echo ""
    echo -e "  ${CYAN}管理命令:${NC}"
    echo -e "    启动服务:   ${YELLOW}./deploy-gce-mig.sh start${NC}"
    echo -e "    停止服务:   ${YELLOW}./deploy-gce-mig.sh stop${NC}  (费用归零)"
    echo -e "    扩缩实例:   ${YELLOW}./deploy-gce-mig.sh scale 3${NC}"
    echo -e "    查看状态:   ${YELLOW}./deploy-gce-mig.sh status${NC}"
    echo ""
    echo -e "  ${CYAN}测试命令:${NC}"
    echo -e "  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{\"msg\":\"Hello!\"}' \\
    --attribute=channel_id=test,event_type=notification${NC}"
    echo ""
    echo -e "  ${YELLOW}⚠ 注意: Load Balancer 需要 3-5 分钟完成配置${NC}"
    echo ""
}

# 查看状态
show_status() {
    print_header "GCE MIG 状态"
    
    echo -e "\n${CYAN}实例组状态:${NC}"
    gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="table(instance,status,lastAttempt.errors)"
    
    echo -e "\n${CYAN}自动扩缩状态:${NC}"
    gcloud compute instance-groups managed describe $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="yaml(status)"
    
    echo -e "\n${CYAN}Backend 健康状态:${NC}"
    gcloud compute backend-services get-health $BACKEND_SERVICE --global 2>/dev/null || echo "Backend 尚未就绪"
    
    SERVICE_URL=$(get_service_url)
    echo -e "\n${CYAN}服务地址:${NC} $SERVICE_URL"
}

# 手动扩缩
scale_mig() {
    local size=$1
    
    if [ -z "$size" ]; then
        print_error "用法: ./deploy-gce-mig.sh scale <实例数>"
    fi
    
    print_info "扩缩到 $size 台实例..."
    
    gcloud compute instance-groups managed resize $INSTANCE_GROUP \
        --size=$size \
        --zone=$ZONE \
        --quiet
    
    print_success "扩缩完成（当前 $size 台）"
}

# 更新镜像（滚动更新）
update_mig() {
    print_header "滚动更新 MIG"
    
    build_image
    create_instance_template
    
    gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
        --version=template=$INSTANCE_TEMPLATE \
        --zone=$ZONE \
        --max-surge=1 \
        --max-unavailable=0 \
        --quiet
    
    print_success "滚动更新已启动"
    print_info "使用 './deploy-gce-mig.sh status' 查看进度"
}

# SSH 到实例
ssh_instance() {
    INSTANCE=$(gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE \
        --format="value(instance)" \
        --limit=1)
    
    if [ -z "$INSTANCE" ]; then
        print_error "没有运行中的实例"
    fi
    
    print_info "连接到 $INSTANCE..."
    gcloud compute ssh $INSTANCE --zone=$ZONE
}

# 删除所有资源
delete_all() {
    print_header "删除 GCE MIG 资源"
    
    echo -e "${YELLOW}即将删除以下资源:${NC}"
    echo "  - Forwarding Rule: $FORWARDING_RULE"
    echo "  - HTTP Proxy: $HTTP_PROXY"
    echo "  - URL Map: $URL_MAP"
    echo "  - Backend Service: $BACKEND_SERVICE"
    echo "  - Managed Instance Group: $INSTANCE_GROUP"
    echo "  - Health Check: $HEALTH_CHECK"
    echo "  - Instance Template: $INSTANCE_TEMPLATE"
    echo "  - Firewall Rules"
    echo ""
    read -p "确认删除? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        print_info "删除 Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        
        print_info "删除 MIG..."
        gcloud compute instance-groups managed delete $INSTANCE_GROUP --zone=$ZONE --quiet 2>/dev/null || true
        
        print_info "删除其他资源..."
        gcloud compute health-checks delete $HEALTH_CHECK --quiet 2>/dev/null || true
        gcloud compute instance-templates delete $INSTANCE_TEMPLATE --quiet 2>/dev/null || true
        gcloud compute firewall-rules delete $FIREWALL_RULE --quiet 2>/dev/null || true
        gcloud compute firewall-rules delete $FIREWALL_RULE_LB --quiet 2>/dev/null || true
        
        print_success "所有资源已删除"
    else
        print_info "已取消"
    fi
}

# 主函数
main() {
    print_header "SSE Gateway - GCE MIG 部署"
    
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

# 启动所有服务（Load Balancer，实例用 scale 控制）
start_all() {
    print_header "启动服务"
    
    # 检查镜像是否存在
    if ! image_exists; then
        print_error "镜像不存在，请先运行 ./deploy-gce-mig.sh 部署"
    fi
    
    # 创建 Load Balancer（如果不存在）
    if ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "创建 Load Balancer..."
        export SKIP_LB=false
        create_load_balancer
    else
        print_success "Load Balancer 已存在"
    fi
    
    echo ""
    print_success "服务启动完成"
    print_warning "Load Balancer 需要 2-3 分钟生效"
    echo ""
    echo -e "  ${CYAN}启动实例:${NC} ${YELLOW}./deploy-gce-mig.sh scale 1${NC}"
    echo -e "  ${CYAN}查看状态:${NC} ${YELLOW}./deploy-gce-mig.sh status${NC}"
}

# 停止所有服务（实例 + Load Balancer）
stop_all() {
    print_header "停止所有服务"
    
    # 停止实例
    print_info "停止所有实例..."
    gcloud compute instance-groups managed resize $INSTANCE_GROUP \
        --size=0 \
        --zone=$ZONE \
        --quiet 2>/dev/null || true
    print_success "实例已停止"
    
    # 删除 Load Balancer
    if gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "删除 Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        print_success "Load Balancer 已删除"
    else
        print_success "Load Balancer 不存在"
    fi
    
    echo ""
    print_success "所有服务已停止，当前费用: \$0/月"
    echo ""
    echo -e "  ${CYAN}重新启动:${NC} ${YELLOW}./deploy-gce-mig.sh start${NC}"
}

# 删除 Load Balancer（保留实例组）
delete_lb() {
    print_header "删除 Load Balancer"
    
    if ! gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1; then
        print_info "Load Balancer 不存在"
        return 0
    fi
    
    echo -e "${YELLOW}即将删除 Load Balancer 相关资源:${NC}"
    echo "  - Forwarding Rule: $FORWARDING_RULE"
    echo "  - HTTP Proxy: $HTTP_PROXY"
    echo "  - URL Map: $URL_MAP"
    echo "  - Backend Service: $BACKEND_SERVICE"
    echo ""
    echo -e "${GREEN}保留:${NC} MIG、实例、健康检查"
    echo ""
    read -p "确认删除? (y/N): " confirm
    
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        print_info "删除 Load Balancer..."
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        
        print_success "Load Balancer 已删除，每月省 \$18"
        print_info "服务现在需要通过实例 IP:8080 访问"
        print_info "运行 './deploy-gce-mig.sh status' 查看实例 IP"
    else
        print_info "已取消"
    fi
}

# 快速部署（跳过构建）
fast_deploy() {
    print_header "SSE Gateway - GCE MIG 快速部署"
    print_info "跳过镜像构建，使用已有镜像"
    
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

# 命令处理
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
    *)
        main
        ;;
esac
