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
    setup_pubsub
    build_image
    
    print_step "5" "Deploying to Cloud Run"
    
    ENV_VARS="RUST_LOG=info"
    SECRETS=""
    
    if secret_exists "GCP_PROJECT"; then
        SECRETS="GCP_PROJECT=GCP_PROJECT:latest"
        print_info "GCP_PROJECT: from Secret Manager"
    else
        ENV_VARS="$ENV_VARS,GCP_PROJECT=$PROJECT_ID"
        print_info "GCP_PROJECT: $PROJECT_ID"
    fi
    
    if secret_exists "PUBSUB_SUBSCRIPTION"; then
        [ -n "$SECRETS" ] && SECRETS="$SECRETS,"
        SECRETS="${SECRETS}PUBSUB_SUBSCRIPTION=PUBSUB_SUBSCRIPTION:latest"
        print_info "PUBSUB_SUBSCRIPTION: from Secret Manager"
    else
        ENV_VARS="$ENV_VARS,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
        print_info "PUBSUB_SUBSCRIPTION: $SUBSCRIPTION_NAME"
    fi
    
    if secret_exists "REDIS_URL"; then
        [ -n "$SECRETS" ] && SECRETS="$SECRETS,"
        SECRETS="${SECRETS}REDIS_URL=REDIS_URL:latest"
        print_info "REDIS_URL: from Secret Manager"
    elif [ -n "$REDIS_URL" ]; then
        ENV_VARS="$ENV_VARS,REDIS_URL=$REDIS_URL"
        print_info "REDIS_URL: from environment"
    else
        print_warning "REDIS_URL: not configured"
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
    cat << EOF

  Service URL:  ${GREEN}$SERVICE_URL${NC}
  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}

  ${CYAN}Configuration:${NC}
    Instances: $CR_MIN_INSTANCES - $CR_MAX_INSTANCES
    Concurrency: $CR_CONCURRENCY/instance
    Resources: $CR_CPU vCPU / $CR_MEMORY

  ${CYAN}Test:${NC}
  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{"msg":"Hello!"}' \\
    --attribute=channel_id=test,event_type=notification${NC}

EOF
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
