# Multi-stage Dockerfile for aiwebengine
# This Dockerfile creates a minimal production image with the aiwebengine server

# Build stage
# The same Debian release as the runtime image, so the binary links against
# the glibc it will run with.
FROM rust:trixie AS builder

# TLS is rustls throughout, so nothing links OpenSSL.
RUN rustup default nightly

# Create app directory
WORKDIR /app

# Copy dependency manifests
COPY Cargo.toml Cargo.lock ./

# Copy SQLx offline query metadata (required for sqlx::query! macros)
COPY .sqlx ./.sqlx

# Enable SQLx offline mode (no database connection required for compilation)
ENV SQLX_OFFLINE=true

# Create dummy source files to cache dependencies.
#
# Cargo parses the manifest here, with no source copied yet, so Cargo.toml must
# not declare a target by path — an explicit [[bin]], [[example]] or [[test]]
# section asserts a file that does not exist in this layer. Auto-discovered
# targets are fine, since they are found by globbing a directory that is absent.
RUN mkdir -p src && \
    echo "fn main() {}" > src/lib.rs && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy source code and compile-time assets. Each of these is an input to
# `cargo build --release`, not runtime data: migrations are embedded by
# sqlx::migrate!, and assets/ is read by include_bytes! and include_str! from
# non-test code — hiding either fails the lib build with "couldn't read".
# docs/, tests/ and scripts/ are not copied: nothing includes a file from docs/,
# a release build of the lib and bin does not compile test targets, and every
# remaining include of scripts/ is behind cfg(test).
COPY src ./src
COPY migrations ./migrations
COPY assets ./assets

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

# Runtime stage: distroless, with glibc, libgcc and CA certificates and no
# shell or package manager. The binary needs nothing else, and every package
# left out is one the image scan has nothing to report about. The nonroot
# variant runs as uid 65532.
FROM gcr.io/distroless/cc-debian13:nonroot

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/aiwebengine /usr/local/bin/aiwebengine

# The configuration file is the only thing this image reads from disk. Engine
# assets — favicon, stylesheet, logo, the TypeScript declarations — are embedded
# in the binary by include_bytes!, scripts and their assets live in Postgres,
# migrations are compiled in by sqlx::migrate!, and logs go to stdout.
COPY config.toml /app/config.toml

# Expose port
EXPOSE 3000

# Health check: the binary's own TCP connect, since the image has no shell,
# curl or wget. The database-aware /health probe lives at the proxy, in
# Caddy's active health checks.
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
    CMD ["aiwebengine", "--healthcheck"]

# Set environment variables
ENV RUST_LOG=info
ENV CONFIG_FILE=/app/config.toml

# Run the server
CMD ["aiwebengine"]
