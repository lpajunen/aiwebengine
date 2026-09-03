# Multi-stage Dockerfile for aiwebengine
# This Dockerfile creates a minimal production image with the aiwebengine server

# Build stage
FROM rust:bookworm AS builder

# Install build dependencies and nightly toolchain
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/* \
    && rustup default nightly

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

# Install runtime dependencies and security updates
# CVE-2025-15467: Upgrade OpenSSL to 3.0.18-1deb12u2 or later
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && apt-get upgrade -y \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash aiwebengine

# Create app directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/aiwebengine /usr/local/bin/aiwebengine

# The configuration file is the only thing this image reads from disk. Engine
# assets — favicon, stylesheet, logo, the TypeScript declarations — are embedded
# in the binary by include_bytes!, scripts and their assets live in Postgres,
# migrations are compiled in by sqlx::migrate!, and logs go to stdout. The
# /app/logs, /app/scripts, /app/assets, /app/docs and /app/data directories this
# replaced were created and populated for a runtime that never opened them.
COPY config.production.toml /app/config.toml
RUN chown aiwebengine:aiwebengine /app /app/config.toml

# Switch to non-root user
USER aiwebengine

# Expose port
EXPOSE 3000

# Health check
# A TCP connect rather than a request to /health: the runtime stage installs
# ca-certificates and libssl3 only, so neither curl nor wget exists here and the
# HTTP probe this replaced reported every container unhealthy unless a compose
# file overrode it. The database-aware /health probe lives at the proxy, in
# Caddy's active health checks.
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
    CMD timeout 2 bash -c '</dev/tcp/localhost/3000' || exit 1

# Set environment variables
ENV RUST_LOG=info
ENV CONFIG_FILE=/app/config.toml

# Run the server
CMD ["aiwebengine"]
