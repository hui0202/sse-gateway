# ============================================
# Stage 1: Chef - 准备依赖计划
# ============================================
FROM rust:1.88-slim-bookworm AS chef

RUN cargo install cargo-chef --locked
WORKDIR /app

# ============================================
# Stage 2: Planner - 分析依赖
# ============================================
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ============================================
# Stage 3: Builder - 构建应用
# ============================================
FROM chef AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 复制依赖计划并构建依赖（这一层会被缓存）
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# 复制源代码并构建应用
COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary
COPY --from=builder /app/target/release/gateway /app/gateway

# Copy configuration file (if exists)
COPY config.yaml* ./

# Set environment variables for Cloud Run
ENV RUST_LOG=info
ENV CLOUD_RUN=true

# Expose port (Cloud Run will set PORT env var)
EXPOSE 8080

# Run the gateway
CMD ["./gateway"]
