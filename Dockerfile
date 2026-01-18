# Multi-stage Dockerfile for aiwebengine
# This Dockerfile creates a minimal production image with the aiwebengine server

# Build stage
FROM rustlang/rust:nightly-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy dependency manifests
COPY Cargo.toml Cargo.lock ./

# Copy SQLx offline query metadata (required for sqlx::query! macros)
COPY .sqlx ./.sqlx

# Enable SQLx offline mode (no database connection required for compilation)
ENV SQLX_OFFLINE=true

# Create dummy source files to cache dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/lib.rs && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy source code and compile-time assets
COPY src ./src
COPY migrations ./migrations
COPY scripts ./scripts
COPY assets ./assets
COPY docs ./docs
COPY tests ./tests

# Copy build script for capturing build metadata
COPY build.rs ./build.rs

# Accept build arguments for git metadata (set during docker build)
# These are used by build.rs when .git directory is not available
ARG GIT_SHA=""
ARG GIT_COMMIT_TIMESTAMP=""
ARG BUILD_TIMESTAMP=""

# Set as environment variables for build.rs to use
ENV VERGEN_GIT_SHA=${GIT_SHA}
ENV VERGEN_GIT_COMMIT_TIMESTAMP=${GIT_COMMIT_TIMESTAMP}
ENV VERGEN_BUILD_TIMESTAMP=${BUILD_TIMESTAMP}

# Build the actual application
# Git metadata will be captured from ENV vars if .git directory is not present
RUN touch src/lib.rs src/main.rs && \
    cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash aiwebengine

# Create app directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/aiwebengine /usr/local/bin/aiwebengine

# Create necessary directories
RUN mkdir -p /app/logs /app/scripts /app/assets /app/docs /app/data && \
    chown -R aiwebengine:aiwebengine /app

# Copy default configuration (use production config as base)
COPY config.production.toml /app/config.toml

# Copy assets and docs if they exist
COPY assets /app/assets/
COPY docs /app/docs/

# Switch to non-root user
USER aiwebengine

# Expose port
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

# Set environment variables
ENV RUST_LOG=info
ENV CONFIG_FILE=/app/config.toml

# Run the server
CMD ["aiwebengine"]
