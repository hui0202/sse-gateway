#!/bin/bash
# SSE Gateway - Unified Deployment Script
#
# Usage:
#   ./deploy.sh cloudrun              Deploy to Cloud Run
#   ./deploy.sh cloudrun delete       Delete Cloud Run service
#
#   ./deploy.sh gce                   Deploy to GCE MIG
#   ./deploy.sh gce start|stop|scale|status|update|ssh|delete
#
#   ./deploy.sh secret NAME VALUE     Add/update Secret
#   ./deploy.sh secrets               List all Secrets

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load modules
source "$SCRIPT_DIR/scripts/common.sh"
source "$SCRIPT_DIR/scripts/cloudrun.sh"
source "$SCRIPT_DIR/scripts/gce.sh"

show_help() {
    cat << EOF
SSE Gateway - Unified Deployment Script

Usage:
  ./deploy.sh cloudrun              Deploy to Cloud Run
  ./deploy.sh cloudrun delete       Delete Cloud Run service

  ./deploy.sh gce                   Deploy to GCE MIG
  ./deploy.sh gce start             Start services (create LB)
  ./deploy.sh gce stop              Stop all (zero cost)
  ./deploy.sh gce scale N           Scale to N instances
  ./deploy.sh gce status            View status
  ./deploy.sh gce update            Rolling update
  ./deploy.sh gce ssh               SSH to instance
  ./deploy.sh gce delete            Delete all resources

  ./deploy.sh secret NAME VALUE     Add/update Secret
  ./deploy.sh secrets               List all Secrets

Environment Variables:
  PROJECT       GCP Project ID
  REGION        Region (default: asia-east1)
  ZONE          Zone for GCE (default: asia-east1-a)
  SKIP_BUILD    Skip image build (true/false)
  SKIP_LB       Skip Load Balancer for GCE (true/false)
EOF
}

case "${1:-}" in
    cloudrun)
        case "${2:-}" in
            delete) check_requirements; cloudrun_delete ;;
            *) cloudrun_deploy ;;
        esac ;;
    gce)
        case "${2:-}" in
            start)  check_requirements; gce_start ;;
            stop)   check_requirements; gce_stop ;;
            scale)  check_requirements; gce_scale "$3" ;;
            status) check_requirements; gce_status ;;
            update) check_requirements; gce_update ;;
            ssh)    check_requirements; gce_ssh ;;
            delete) check_requirements; gce_delete ;;
            *) gce_deploy ;;
        esac ;;
    secret)  check_requirements; add_secret "$2" "$3" ;;
    secrets) check_requirements; list_secrets ;;
    help|--help|-h) show_help ;;
    "") show_help ;;
    *) echo "Unknown command: $1"; show_help; exit 1 ;;
esac
