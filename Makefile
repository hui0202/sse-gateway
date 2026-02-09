# SSE Gateway Development Commands
# Usage: make <command>

.PHONY: help dev run build clean redis redis-stop \
        deploy-cloudrun deploy-cloudrun-delete \
        deploy-gce deploy-gce-start deploy-gce-stop deploy-gce-scale \
        deploy-gce-status deploy-gce-update deploy-gce-ssh deploy-gce-delete \
        secrets-list secret-set secrets-pull secrets-push \
        docker-build docker-run

# Default: show help
help:
	@echo "SSE Gateway Development Commands"
	@echo ""
	@echo "Development:"
	@echo "  make dev            - Start dev server (auto-reload)"
	@echo "  make run            - Run the service"
	@echo "  make build          - Build release version"
	@echo "  make clean          - Clean build artifacts"
	@echo ""
	@echo "Local Services:"
	@echo "  make redis          - Start local Redis"
	@echo "  make redis-stop     - Stop local Redis"
	@echo ""
	@echo "Deploy (GCP):"
	@echo "  make deploy-cloudrun              - Deploy to Cloud Run"
	@echo "  make deploy-cloudrun-delete       - Delete Cloud Run service"
	@echo "  make deploy-gce                   - Deploy to GCE MIG"
	@echo "  make deploy-gce-start             - Start GCE (create LB)"
	@echo "  make deploy-gce-stop              - Stop GCE (zero cost)"
	@echo "  make deploy-gce-scale N=3         - Scale to N instances"
	@echo "  make deploy-gce-status            - Check status"
	@echo "  make deploy-gce-update            - Rolling update"
	@echo "  make deploy-gce-ssh               - SSH into instance"
	@echo "  make deploy-gce-delete            - Delete all resources"
	@echo ""
	@echo "Secrets Management:"
	@echo "  make secrets-list                 - List all secrets"
	@echo "  make secret-set NAME=xx VALUE=yy  - Add/update a secret"
	@echo "  make secrets-pull                 - Download all secrets to .env.local"
	@echo "  make secrets-push                 - Upload .env.local to GCP"
	@echo ""
	@echo "Docker:"
	@echo "  make docker-build   - Build Docker image"
	@echo "  make docker-run     - Run Docker container"

# ============================================================
# Development Commands
# ============================================================

# Dev mode (requires cargo-watch: cargo install cargo-watch)
dev:
	@if [ -f .env.local ]; then set -a && . ./.env.local && set +a; fi && \
	cargo watch -x run

# Run (auto-loads .env.local)
run:
	@if [ -f .env.local ]; then set -a && . ./.env.local && set +a; fi && \
	cargo run

# Build release
build:
	cargo build --release

# Clean
clean:
	cargo clean

# ============================================================
# Local Services
# ============================================================

# Start local Redis (background)
redis:
	@if pgrep -x "redis-server" > /dev/null; then \
		echo "Redis is already running"; \
	else \
		redis-server --daemonize yes; \
		echo "Redis started"; \
	fi

# Stop local Redis
redis-stop:
	@if pgrep -x "redis-server" > /dev/null; then \
		redis-cli shutdown; \
		echo "Redis stopped"; \
	else \
		echo "Redis is not running"; \
	fi

# ============================================================
# Deploy - Cloud Run
# ============================================================

deploy-cloudrun:
	./deploy.sh cloudrun

deploy-cloudrun-delete:
	./deploy.sh cloudrun delete

# ============================================================
# Deploy - GCE MIG
# ============================================================

deploy-gce:
	./deploy.sh gce

deploy-gce-start:
	./deploy.sh gce start

deploy-gce-stop:
	./deploy.sh gce stop

deploy-gce-scale:
	@if [ -z "$(N)" ]; then \
		echo "Usage: make deploy-gce-scale N=3"; \
		exit 1; \
	fi
	./deploy.sh gce scale $(N)

deploy-gce-status:
	./deploy.sh gce status

deploy-gce-update:
	./deploy.sh gce update

deploy-gce-ssh:
	./deploy.sh gce ssh

deploy-gce-delete:
	./deploy.sh gce delete

# ============================================================
# GCP Secret Manager (via deploy.sh)
# ============================================================

# List all secrets
secrets-list:
	./deploy.sh secrets

# Add/update a secret
# Usage: make secret-set NAME=REDIS_URL VALUE=redis://localhost:6379
secret-set:
	@if [ -z "$(NAME)" ] || [ -z "$(VALUE)" ]; then \
		echo "Usage: make secret-set NAME=xxx VALUE=yyy"; \
		echo ""; \
		./deploy.sh secret; \
		exit 1; \
	fi
	./deploy.sh secret $(NAME) "$(VALUE)"

# Download all secrets from GCP to .env.local
secrets-pull:
	@echo "Downloading secrets from GCP to .env.local..."
	@echo "# Auto-generated from GCP Secret Manager" > .env.local
	@for secret in $$(gcloud secrets list --format="value(name)"); do \
		value=$$(gcloud secrets versions access latest --secret="$$secret" 2>/dev/null); \
		if [ -n "$$value" ]; then \
			echo "$$secret=$$value" >> .env.local; \
			echo "  ✓ $$secret"; \
		fi; \
	done
	@echo "✓ Saved to .env.local"

# Upload all secrets from .env.local to GCP
secrets-push:
	@if [ ! -f .env.local ]; then \
		echo "Error: .env.local not found"; \
		exit 1; \
	fi
	@echo "Uploading secrets from .env.local to GCP..."
	@grep -v '^#' .env.local | grep '=' | while IFS='=' read -r name value; do \
		if [ -n "$$name" ] && [ -n "$$value" ]; then \
			./deploy.sh secret "$$name" "$$value"; \
		fi; \
	done
	@echo "✓ Upload complete"

# ============================================================
# Docker
# ============================================================

# Build Docker image
docker-build:
	docker build -t sse-gateway .

# Run Docker container
docker-run:
	docker run -p 8080:8080 --env-file .env.local sse-gateway
