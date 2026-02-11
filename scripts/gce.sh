#!/bin/bash
# GCE MIG deployment functions

GCE_MACHINE_TYPE=${MACHINE_TYPE:-"f1-micro"}
GCE_INIT_SIZE=${INIT_SIZE:-0}

# Resource names
INSTANCE_TEMPLATE="${SERVICE_NAME}-template"
INSTANCE_GROUP="${SERVICE_NAME}-mig"
HEALTH_CHECK="${SERVICE_NAME}-health-check"
BACKEND_SERVICE="${SERVICE_NAME}-backend"
URL_MAP="${SERVICE_NAME}-url-map"
HTTP_PROXY="${SERVICE_NAME}-http-proxy"
FORWARDING_RULE="${SERVICE_NAME}-forwarding-rule"
FIREWALL_RULE="${SERVICE_NAME}-allow-health-check"
FIREWALL_RULE_LB="${SERVICE_NAME}-allow-lb"

gce_setup_firewall() {
    print_step "5" "Configuring firewall rules"
    
    for rule in "$FIREWALL_RULE:130.211.0.0/22,35.191.0.0/16" "$FIREWALL_RULE_LB:0.0.0.0/0"; do
        name="${rule%%:*}"
        ranges="${rule#*:}"
        if gcloud compute firewall-rules describe $name &>/dev/null; then
            print_success "$name (already exists)"
        else
            gcloud compute firewall-rules create $name \
                --network=default --action=allow --direction=ingress \
                --source-ranges=$ranges --target-tags=$SERVICE_NAME \
                --rules=tcp:8080 --quiet
            print_success "$name (created)"
        fi
    done
}

gce_create_template() {
    print_step "6" "Creating Instance Template"
    
    gcloud compute instance-templates describe $INSTANCE_TEMPLATE &>/dev/null && {
        print_warning "Deleting old template..."
        INSTANCE_TEMPLATE="${SERVICE_NAME}-template-$(date +%Y%m%d%H%M%S)"
    }
    
    PROJECT_NUMBER=$(gcloud projects describe $PROJECT_ID --format='value(projectNumber)')
    SERVICE_ACCOUNT="${PROJECT_NUMBER}-compute@developer.gserviceaccount.com"
    
    CONTAINER_ENV="RUST_LOG=info,NO_COLOR=1,GCP_PROJECT=$PROJECT_ID,PUBSUB_SUBSCRIPTION=$SUBSCRIPTION_NAME"
    
    if secret_exists "REDIS_URL"; then
        CONTAINER_ENV="$CONTAINER_ENV,REDIS_URL=$(get_secret_value REDIS_URL)"
        print_info "REDIS_URL: from Secret Manager"
    elif [ -n "$REDIS_URL" ]; then
        CONTAINER_ENV="$CONTAINER_ENV,REDIS_URL=$REDIS_URL"
        print_info "REDIS_URL: from environment"
    else
        print_warning "REDIS_URL: not configured"
    fi
    
    gcloud compute instance-templates create-with-container $INSTANCE_TEMPLATE \
        --machine-type=$GCE_MACHINE_TYPE \
        --container-image=$IMAGE_NAME \
        --container-env="$CONTAINER_ENV" \
        --tags=$SERVICE_NAME \
        --network=default \
        --service-account=$SERVICE_ACCOUNT \
        --scopes=cloud-platform \
        --metadata-from-file=startup-script="$SCRIPT_DIR/scripts/gce-startup.sh" \
        --boot-disk-size=20GB \
        --boot-disk-type=pd-standard \
        --quiet
    
    print_success "Instance Template: $INSTANCE_TEMPLATE"
}

gce_create_health_check() {
    print_step "7" "Creating health check"
    
    gcloud compute health-checks describe $HEALTH_CHECK &>/dev/null && {
        print_success "Health check (already exists)"
        return 0
    }
    
    gcloud compute health-checks create http $HEALTH_CHECK \
        --port=8080 --request-path=/health \
        --check-interval=10s --timeout=5s \
        --healthy-threshold=2 --unhealthy-threshold=3 --quiet
    print_success "Health check (created)"
}

gce_create_mig() {
    print_step "8" "Creating Managed Instance Group"
    
    if gcloud compute instance-groups managed describe $INSTANCE_GROUP --zone=$ZONE &>/dev/null; then
        print_warning "MIG exists, updating..."
        gcloud compute instance-groups managed set-instance-template $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE --zone=$ZONE --quiet
        gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
            --version=template=$INSTANCE_TEMPLATE --zone=$ZONE \
            --max-surge=1 --max-unavailable=0 --quiet
        print_success "MIG rolling update started"
    else
        gcloud compute instance-groups managed create $INSTANCE_GROUP \
            --template=$INSTANCE_TEMPLATE --size=$GCE_INIT_SIZE --zone=$ZONE \
            --health-check=$HEALTH_CHECK --initial-delay=120 --quiet
        print_success "MIG: $INSTANCE_GROUP (created)"
    fi
    
    gcloud compute instance-groups managed set-named-ports $INSTANCE_GROUP \
        --named-ports=http:8080 --zone=$ZONE --quiet
}

gce_create_lb() {
    [ "${SKIP_LB:-false}" = "true" ] && {
        print_step "9" "Skipping Load Balancer"
        return 0
    }
    
    print_step "9" "Creating Load Balancer"
    
    gcloud compute backend-services describe $BACKEND_SERVICE --global &>/dev/null || \
        gcloud compute backend-services create $BACKEND_SERVICE \
            --protocol=HTTP --port-name=http --health-checks=$HEALTH_CHECK \
            --global --timeout=3600s --connection-draining-timeout=300s \
            --session-affinity=CLIENT_IP --quiet
    print_success "Backend Service"
    
    gcloud compute backend-services add-backend $BACKEND_SERVICE \
        --instance-group=$INSTANCE_GROUP --instance-group-zone=$ZONE \
        --balancing-mode=UTILIZATION --max-utilization=0.8 --global --quiet 2>/dev/null || true
    
    gcloud compute url-maps describe $URL_MAP &>/dev/null || \
        gcloud compute url-maps create $URL_MAP --default-service=$BACKEND_SERVICE --quiet
    print_success "URL Map"
    
    gcloud compute target-http-proxies describe $HTTP_PROXY &>/dev/null || \
        gcloud compute target-http-proxies create $HTTP_PROXY --url-map=$URL_MAP --quiet
    print_success "HTTP Proxy"
    
    gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null || \
        gcloud compute forwarding-rules create $FORWARDING_RULE \
            --global --target-http-proxy=$HTTP_PROXY --ports=80 --quiet
    print_success "Forwarding Rule"
}

gce_get_service_url() {
    LB_IP=$(gcloud compute forwarding-rules describe $FORWARDING_RULE --global --format='value(IPAddress)' 2>/dev/null)
    [ -n "$LB_IP" ] && { echo "http://$LB_IP"; return; }
    
    INSTANCE=$(gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE --format="value(instance)" --limit=1 2>/dev/null)
    [ -n "$INSTANCE" ] && {
        IP=$(gcloud compute instances describe $INSTANCE --zone=$ZONE \
            --format='value(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null)
        [ -n "$IP" ] && echo "http://$IP:8080" || echo "(no instance)"
    } || echo "(no instance)"
}

gce_deploy() {
    print_header "SSE Gateway - GCE MIG Deployment"
    
    print_step "1" "Checking environment"
    check_requirements
    print_success "Zone: $ZONE"
    print_info "Machine: $GCE_MACHINE_TYPE (~\$5/month)"
    
    enable_apis "gce"
    setup_pubsub
    build_image
    gce_setup_firewall
    gce_create_template
    gce_create_health_check
    gce_create_mig
    gce_create_lb
    gce_summary
}

gce_summary() {
    SERVICE_URL=$(gce_get_service_url)
    
    print_header "GCE MIG Deployment Complete"
    cat << EOF

  Service URL:  ${GREEN}$SERVICE_URL${NC}
  Dashboard:    ${GREEN}$SERVICE_URL/dashboard${NC}

  ${CYAN}Commands:${NC}
    Start:  ${YELLOW}./deploy.sh gce start${NC}
    Stop:   ${YELLOW}./deploy.sh gce stop${NC}
    Scale:  ${YELLOW}./deploy.sh gce scale 3${NC}
    Status: ${YELLOW}./deploy.sh gce status${NC}

  ${CYAN}Test:${NC}
  ${YELLOW}gcloud pubsub topics publish $TOPIC_NAME \\
    --message='{"msg":"Hello!"}' \\
    --attribute=channel_id=test,event_type=notification${NC}

EOF
}

gce_status() {
    print_header "GCE MIG Status"
    echo -e "\n${CYAN}Instances:${NC}"
    gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE --format="table(instance,status)"
    echo -e "\n${CYAN}Service URL:${NC} $(gce_get_service_url)"
}

gce_scale() {
    [ -z "$1" ] && print_error "Usage: ./deploy.sh gce scale <count>"
    gcloud compute instance-groups managed resize $INSTANCE_GROUP --size=$1 --zone=$ZONE --quiet
    print_success "Scaled to $1 instances"
}

gce_start() {
    print_header "Starting Services"
    image_exists || print_error "Run './deploy.sh gce' first"
    gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1 || {
        export SKIP_LB=false; gce_create_lb
    }
    print_success "Started. Run './deploy.sh gce scale 1' to start instances"
}

gce_stop() {
    print_header "Stopping All Services"
    gcloud compute instance-groups managed resize $INSTANCE_GROUP --size=0 --zone=$ZONE --quiet 2>/dev/null || true
    print_success "Instances stopped"
    
    gcloud compute forwarding-rules describe $FORWARDING_RULE --global &>/dev/null 2>&1 && {
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        print_success "Load Balancer deleted"
    }
    print_success "Cost: \$0/month"
}

gce_update() {
    print_header "Rolling Update"
    build_image
    gce_create_template
    gcloud compute instance-groups managed rolling-action start-update $INSTANCE_GROUP \
        --version=template=$INSTANCE_TEMPLATE --zone=$ZONE \
        --max-surge=1 --max-unavailable=0 --quiet
    print_success "Update started"
}

gce_ssh() {
    INSTANCE=$(gcloud compute instance-groups managed list-instances $INSTANCE_GROUP \
        --zone=$ZONE --format="value(instance)" --limit=1)
    [ -z "$INSTANCE" ] && print_error "No running instances"
    gcloud compute ssh $INSTANCE --zone=$ZONE
}

gce_delete() {
    print_header "Delete GCE MIG Resources"
    echo -e "${YELLOW}Will delete: MIG, LB, firewall rules${NC}"
    read -p "Confirm? (y/N): " confirm
    
    [ "$confirm" = "y" ] || [ "$confirm" = "Y" ] && {
        gcloud compute forwarding-rules delete $FORWARDING_RULE --global --quiet 2>/dev/null || true
        gcloud compute target-http-proxies delete $HTTP_PROXY --quiet 2>/dev/null || true
        gcloud compute url-maps delete $URL_MAP --quiet 2>/dev/null || true
        gcloud compute backend-services delete $BACKEND_SERVICE --global --quiet 2>/dev/null || true
        gcloud compute instance-groups managed delete $INSTANCE_GROUP --zone=$ZONE --quiet 2>/dev/null || true
        gcloud compute health-checks delete $HEALTH_CHECK --quiet 2>/dev/null || true
        gcloud compute instance-templates delete $INSTANCE_TEMPLATE --quiet 2>/dev/null || true
        gcloud compute firewall-rules delete $FIREWALL_RULE $FIREWALL_RULE_LB --quiet 2>/dev/null || true
        print_success "All resources deleted"
    } || print_info "Cancelled"
}
