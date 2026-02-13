#!/bin/bash
# Cloud Run deployment functions

CR_MAX_INSTANCES=${MAX_INSTANCES:-10}
CR_MIN_INSTANCES=${MIN_INSTANCES:-0}
CR_MEMORY=${MEMORY:-"1Gi"}
CR_CPU=${CPU:-"2"}
CR_CONCURRENCY=${CONCURRENCY:-1000}

cloudrun_deploy() {
    print_header "SSE Gateway - Cloud Run Deployment"
    
    print_step "1" "Checking environment"
    check_requirements
    print_info "Max instances: $CR_MAX_INSTANCES (supports $((CR_MAX_INSTANCES * CR_CONCURRENCY)) concurrent)"
    
    enable_apis "cloudrun"
    build_image
    
    print_step "4" "Deploying to Cloud Run"
    
    # 基础环境变量
    ENV_VARS="RUST_LOG=info,GCP_PROJECT=$PROJECT_ID"
    print_info "GCP_PROJECT: $PROJECT_ID"
    
    # 自动注入所有 secrets（从 Secret Manager 获取当前服务的所有 secrets）
    SECRETS=$(generate_cloudrun_secrets)
    
    if [ -z "$SECRETS" ]; then
        print_warning "No secrets found for ${SERVICE_NAME}"
    fi
    
    DEPLOY_CMD="gcloud run deploy $SERVICE_NAME \
        --image $IMAGE_NAME \
        --region $REGION \
        --platform managed \
        --allow-unauthenticated \
        --min-instances $CR_MIN_INSTANCES \
        --max-instances $CR_MAX_INSTANCES \
        --memory $CR_MEMORY \
        --cpu $CR_CPU \
        --concurrency $CR_CONCURRENCY \
        --timeout 3600s \
        --cpu-throttling \
        --session-affinity \
        --set-env-vars $ENV_VARS \
        --quiet"
    
    [ -n "$SECRETS" ] && DEPLOY_CMD="$DEPLOY_CMD --set-secrets=$SECRETS"
    eval $DEPLOY_CMD
    
    print_step "6" "Verifying deployment"
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    sleep 3
    
    curl -sk "$SERVICE_URL/health" 2>/dev/null | grep -q "OK" && \
        print_success "Health check passed" || \
        print_warning "Health check failed, service may still be starting"
    
    cloudrun_summary
}

cloudrun_summary() {
    SERVICE_URL=$(gcloud run services describe $SERVICE_NAME --region $REGION --format 'value(status.url)')
    
    print_header "Cloud Run Deployment Complete"
    echo ""
    echo -e "  Service URL:  ${GREEN}$SERVICE_URL${NC}"
    echo -e "  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}"
    echo ""
    echo -e "  ${CYAN}Configuration:${NC}"
    echo -e "    Instances: $CR_MIN_INSTANCES - $CR_MAX_INSTANCES"
    echo -e "    Concurrency: $CR_CONCURRENCY/instance"
    echo -e "    Resources: $CR_CPU vCPU / $CR_MEMORY"
    echo ""
    echo -e "  ${CYAN}Test:${NC}"
    echo -e "  ${YELLOW}curl $SERVICE_URL/health${NC}"
    echo ""
}

cloudrun_delete() {
    print_header "Delete Cloud Run Service"
    echo -e "${YELLOW}Will delete: $SERVICE_NAME${NC}"
    read -p "Confirm? (y/N): " confirm
    
    [ "$confirm" = "y" ] || [ "$confirm" = "Y" ] && {
        gcloud run services delete $SERVICE_NAME --region $REGION --quiet
        print_success "Service deleted"
    } || print_info "Cancelled"
}
