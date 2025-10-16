# Multi-stage Dockerfile for aiwebengine
# This Dockerfile creates a minimal production image with the aiwebengine server

# Build stage
FROM rust:1.83-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy dependency manifests
COPY Cargo.toml Cargo.lock ./

# Create a dummy main to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy source code
COPY src ./src
COPY tests ./tests

# Build the actual application
RUN touch src/main.rs && \
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
RUN mkdir -p /app/logs /app/scripts /app/assets /app/data && \
    chown -R aiwebengine:aiwebengine /app

# Copy default configuration
COPY config.example.yaml /app/config.yaml

# Copy assets if they exist
COPY assets /app/assets/

# Switch to non-root user
USER aiwebengine

# Expose port
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

# Set environment variables
ENV RUST_LOG=info
ENV CONFIG_FILE=/app/config.yaml

# Run the server
CMD ["aiwebengine"]
