# Makefile for aiwebengine development

.PHONY: help deps upgrade-deps test dev build clean lint format coverage check ci typecheck check-embedded build-desktop
.PHONY: docker-build docker-local docker-staging docker-prod docker-stop docker-logs docker-clean
.PHONY: docker-dns check-dns clean-acme-dns

help:
	@echo "Available commands:"
	@echo ""
	@echo "Development:"
	@echo "  make deps         - Install development tools (cargo-watch, cargo-nextest, cargo-llvm-cov)"
	@echo "  make upgrade-deps - Upgrade npm packages to latest versions"
	@echo "  make dev       - Run development server with auto-reload"
	@echo "  make dev-local - Run development server with localhost OAuth (http://localhost:3000)"
	@echo "  make docker-localhost - Start Docker with localhost only (no DNS setup required)"
	@echo "  make docker-dns       - Start Docker with DNS domain (requires DIGITALOCEAN_TOKEN)"
	@echo "  make check-dns        - Check if DNS domain configuration is available"
	@echo "  make clean-acme-dns   - Remove stale ACME challenge records from the DNS zone"
	@echo "  make test      - Run all tests with cargo-nextest"
	@echo "  make test-simple - Run all tests with cargo test"
	@echo "  make perf-test   - Run performance/load test against production server"
	@echo "  make lint        - Run clippy linter"
	@echo "  make typecheck   - Run TypeScript declaration checks"
	@echo "  make format    - Format code with rustfmt"
	@echo "  make format-check - Check code formatting without modifying"
	@echo "  make coverage  - Generate test coverage report"
	@echo "  make build     - Build release binary"
	@echo "  make clean     - Clean build artifacts"
	@echo "  make check     - Run all pre-commit checks (format, lint, test)"
	@echo "  make ci        - Run full CI pipeline (format, lint, test, coverage)"
	@echo ""
	@echo "Docker:"
	@echo "  make docker-build        - Build production Docker image"
	@echo "  make docker-local        - Start local/development environment with Docker"
	@echo "  make docker-staging      - Start staging environment with Docker"
	@echo "  make docker-prod         - Start production environment with Docker"
	@echo "  make docker-stop         - Stop all Docker containers"
	@echo "  make docker-logs         - View Docker container logs"
	@echo "  make docker-clean        - Stop and remove all Docker containers and volumes"
	@echo "  make docker-shell        - Open shell in running container"
	@echo "  make docker-test         - Run tests in Docker container"
	@echo "  make postgres-local      - Start only PostgreSQL in local environment"
	@echo "  make postgres-local-stop - Stop PostgreSQL in local environment"

# Upgrade npm dependencies to latest versions
upgrade-deps:
	npx npm-check-updates -u && npm install

# Install development dependencies
deps:
	@echo "Installing development tools..."
	cargo install cargo-watch
	cargo install cargo-nextest
	cargo install cargo-llvm-cov
	@if [ ! -d "node_modules" ]; then \
		echo "Installing npm dependencies..."; \
		npm install; \
	else \
		echo "npm dependencies already installed"; \
	fi
	@echo "Development tools installed successfully!"

# Run development server with auto-reload
dev:
	cargo watch -x 'run'

# Run development server locally (with localhost OAuth redirect)
dev-local:
	@echo "Starting development server with localhost OAuth redirect..."
	@echo "Access at: http://localhost:3000"
	@bash -c 'source .env && export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=http://localhost:3000/auth/callback/google && cargo run'

# The features every gate builds with.
#
# Deliberately not --all-features: `embedded-postgres` starts a PostgreSQL of
# the engine's own, which is the one thing the test harness must not do — every
# test claims a numbered slot database on the server DATABASE_URL names
# (tests/common/testdb.rs) — and `embedded-postgres-bundled` would stage a
# 30-40 MB archive into every build that ran it. Compile-checked separately by
# `make check-embedded`; add real features here as they appear.
TEST_FEATURES ?=

# Run tests with cargo-nextest (better output)
# Sources .env (when present) so DATABASE_URL reaches the test harness; without
# it the integration tests fall back to the default local connection string.
test:
	@bash -c 'if [ -f .env ]; then source .env; fi; cargo nextest run --features "$(TEST_FEATURES)" --no-fail-fast'

# Run tests with standard cargo test.
#
# Kept for the cases nextest cannot serve — a debugger attached to one test, an
# environment where nextest is not installed — and not part of any gate, because
# the suite does not pass under it. `cargo test` runs a binary's tests as threads
# in one process, each with its own `#[tokio::test]` runtime, while the database
# pool they share is driven by whichever runtime opened it; when that test ends,
# its runtime goes with it and every later test acquiring one of those
# connections blocks until the pool's timeout. Running one test at a time does
# not help, because the problem is sequence rather than contention. `cargo
# nextest` gives each test its own process, which is the shape this engine is
# built for: one runtime, one pool, for the life of the process.
test-simple:
	@bash -c 'if [ -f .env ]; then source .env; fi; cargo test --features "$(TEST_FEATURES)"'

# Run performance/load tests against production
perf-test:
	@echo "Running performance test against production server..."
	@echo "This will test https://softagen.com with up to 100 concurrent users"
	@echo "Test duration: 6 minutes"
	DOCKER_HOST='' docker run --rm -v "$(CURDIR)/scripts/perf_tests:/scripts" -w /scripts grafana/k6 run load_test.js

# Keep the embedded-database supervisor compiling.
#
# The non-bundled variant on purpose: it type-checks every line of
# src/embedded_db.rs while `embedded-postgres-bundled` would download a
# platform archive to stage into the binary. That download belongs in a release
# build of the desktop app, not in a check.
check-embedded:
	cargo clippy --all-targets --features embedded-postgres -- -D warnings

# Run clippy linter with warnings as errors
lint:
	cargo clippy --all-targets -- -D warnings
	./node_modules/.bin/markdownlint "**/*.md" --ignore node_modules && echo '✓ Markdown files linted'

# Run TypeScript declaration checks
typecheck:
	npm run typecheck

# Format all code
format: format-markdown format-javascript
	cargo fmt --all

# Check formatting without modifying files
format-check:
	cargo fmt --all -- --check

format-markdown:
	./node_modules/.bin/prettier --write "**/*.md"

format-javascript:
	./node_modules/.bin/prettier --write "**/*.js" "**/*.ts"

# Generate test coverage report
coverage:
	cargo llvm-cov --features "$(TEST_FEATURES)" --html
	@echo "Coverage report generated: target/llvm-cov/html/index.html"

# Build release binary
#
# Default features only, which is the point of the feature split: a server build
# compiles none of the embedded-database supervisor and carries none of its
# weight.
build:
	cargo build --release

# Build the desktop standalone binary: a PostgreSQL of its own, with the
# platform archive compiled in so a first launch needs no network. Adds roughly
# 30-40 MB to the binary, and needs network access at build time to stage the
# archive.
build-desktop:
	cargo build --release --features embedded-postgres-bundled

# Clean build artifacts
clean:
	cargo clean

# Pre-commit checks (format check, lint, test)
check: format-check lint typecheck test
	@echo "✓ All checks passed!"

# CI pipeline (format check, lint, test, coverage)
ci: format-check lint typecheck test check-embedded coverage
	@echo "✓ CI pipeline completed!"

# ==================== Docker Commands ====================

# Build production Docker image
docker-build:
	@echo "Building production Docker image..."
	docker build -t aiwebengine:latest .
	@echo "✓ Docker image built successfully!"

# Build local/development Docker image
docker-build-local:
	@echo "Building local/development Docker image..."
	docker build -f Dockerfile.local -t aiwebengine:dev .
	@echo "✓ Local/development Docker image built successfully!"

# Build staging Docker image (uses production Dockerfile)
docker-build-staging:
	@echo "Building staging Docker image..."
	docker build -t aiwebengine:staging .
	@echo "✓ Staging Docker image built successfully!"

# Start local/development environment with Docker Compose
docker-local:
	@echo "Starting local/development environment..."
	docker compose --env-file .env-local -f docker-compose.local.yml up

# Start with localhost only (no DNS setup needed)
docker-localhost:
	@echo "Starting local development with localhost only..."
	@echo "Access at: https://localhost"
	@echo "Note: You may need to accept self-signed certificate warning"
	@echo "Serve a name of your own instead: SITE_HOSTS=my.example.test make docker-localhost"
	@unset DIGITALOCEAN_TOKEN TLS_SNIPPET; \
	docker compose --env-file .env-local -f docker-compose.local.yml up

# Start with DNS domain (requires DIGITALOCEAN_TOKEN)
#
# SITE_HOSTS and TLS_SNIPPET are what the Caddyfile reads; DNS_DOMAIN stays the
# knob to set, and defaults to the name this repository uses. Both variables are
# exported together because the issuer and the hostname have to agree: a DNS-01
# certificate for `localhost` is not obtainable, and Let's Encrypt cannot be
# asked for one for a name it cannot verify.
docker-dns:
	@echo "Starting local development with DNS domain..."
	@if [ -z "$$DIGITALOCEAN_TOKEN" ]; then \
		echo "❌ ERROR: DIGITALOCEAN_TOKEN not set"; \
		echo "   Set it in .env file or export: export DIGITALOCEAN_TOKEN=your_token"; \
		exit 1; \
	fi
	@bash scripts/acme-dns-cleanup.sh
	@export SITE_HOSTS=$${DNS_DOMAIN:-local.softagen.com}; \
	export TLS_SNIPPET=tls_acme_dns; \
	echo "Access at: https://$$SITE_HOSTS"; \
	echo "Note: the local names (https://localhost) are not served in this mode"; \
	docker compose --env-file .env-local -f docker-compose.local.yml up

# Remove leftover ACME challenge records by hand. `docker-dns` runs this first,
# so this target is for clearing the zone without starting the stack.
clean-acme-dns:
	@bash scripts/acme-dns-cleanup.sh

# Check DNS domain availability
check-dns:
	@bash scripts/check-dns.sh

# Start local/development environment in background
docker-local-bg:
	@echo "Starting local/development environment in background..."
	docker compose --env-file .env-local -f docker-compose.local.yml up -d
	@echo "✓ Local/development environment started!"
	@echo "View logs with: make docker-logs-local"

# Start staging environment with Docker Compose
# Each server environment is one env file: it supplies both the values compose
# interpolates and, through ENV_FILE, the environment the containers receive.
# Docker creates a missing bind-mount source as a directory, and mounting a
# directory onto a file inside the image fails with a runc error that names
# overlay2 paths and explains nothing. Catch it here, where the fix is obvious.
check-mounts:
	@for f in Caddyfile config.toml; do \
		if [ -d "$$f" ]; then \
			echo "❌ $$f is a DIRECTORY, not a file."; \
			echo "   Docker created it when the file was missing. Remove it and restore the file:"; \
			echo "     rmdir $$f && git checkout $$f"; \
			exit 1; \
		elif [ ! -f "$$f" ]; then \
			echo "❌ $$f is missing. This checkout is not up to date:"; \
			echo "     git pull"; \
			exit 1; \
		fi; \
	done
	@if [ ! -d caddy-sites ]; then \
		echo "❌ caddy-sites/ is missing. Run: git pull"; exit 1; \
	fi

docker-staging: check-mounts
	@echo "Starting staging environment..."
	docker compose --env-file .env-staging up -d --build
	@echo "✓ Staging environment started!"
	@echo "View logs with: make docker-logs-staging"

# Start production environment with Docker Compose
docker-prod: check-mounts
	@echo "Starting production environment..."
	docker compose --env-file .env-production up -d --build
	@echo "✓ Production environment started!"
	@echo "View logs with: make docker-logs"

# Stop all Docker containers
docker-stop:
	@echo "Stopping Docker containers..."
	docker-compose down
	docker compose --env-file .env-staging down
	docker compose --env-file .env-local -f docker-compose.local.yml down
	@echo "✓ All containers stopped!"

# View production logs
docker-logs:
	docker-compose logs -f aiwebengine-1 aiwebengine-2

# View local/development logs
docker-logs-local:
	docker-compose -f docker-compose.local.yml logs -f aiwebengine-dev

# View staging logs
docker-logs-staging:
	docker compose --env-file .env-staging logs -f aiwebengine-1

# View all service logs
docker-logs-all:
	docker-compose logs -f

# Clean up Docker containers and volumes
docker-clean:
	@echo "Cleaning up Docker containers and volumes..."
	docker-compose down -v
	docker compose --env-file .env-staging down -v
	docker compose --env-file .env-local -f docker-compose.local.yml down -v
	@echo "✓ Docker cleanup completed!"

# Clean up Docker images
docker-clean-images:
	@echo "Removing Docker images..."
	docker rmi aiwebengine:latest aiwebengine:staging aiwebengine:dev 2>/dev/null || true
	@echo "✓ Docker images removed!"

# Full Docker cleanup (containers, volumes, images)
docker-clean-all: docker-clean docker-clean-images
	@echo "✓ Full Docker cleanup completed!"

# Open shell in running production container
docker-shell:
	docker-compose exec aiwebengine /bin/bash

# Open shell in running local/development container
docker-shell-local:
	docker-compose -f docker-compose.local.yml exec aiwebengine-dev /bin/bash

# Open shell in running staging container
docker-shell-staging:
	docker compose --env-file .env-staging exec aiwebengine-1 /bin/bash

# Run tests in Docker container
docker-test:
	docker-compose -f docker-compose.local.yml run --rm aiwebengine-dev cargo test

# Check Docker container status
docker-ps:
	@echo "Production containers:"
	@docker-compose ps
	@echo ""
	@echo "Staging containers:"
	@docker compose --env-file .env-staging ps
	@echo ""
	@echo "Local/development containers:"
	@docker-compose -f docker-compose.local.yml ps

# Restart production containers
docker-restart:
	docker-compose restart

# Rebuild and restart production environment
docker-rebuild:
	@echo "Rebuilding and restarting production environment..."
	docker-compose down
	docker-compose build --no-cache
	docker-compose up -d
	@echo "✓ Production environment rebuilt and restarted!"

# Show Docker resource usage
docker-stats:
	docker stats $(shell docker-compose ps -q)

# Create .env file from example
docker-env:
	@if [ ! -f .env-production ]; then \
		cp .env.example .env-production; \
		echo "✓ Created .env-production from .env.example"; \
		echo "⚠ Fill it in, then check it: set -a; . ./.env-production; set +a; cargo run -- --validate-config"; \
	else \
		echo ".env-production already exists"; \
	fi

# Complete Docker setup for first-time use
docker-setup: docker-env docker-build
	@echo "✓ Docker setup completed!"
	@echo "You can now run: make docker-prod"

# Start only PostgreSQL in local environment
postgres-local:
	@echo "Starting PostgreSQL server in local environment..."
	docker-compose -f docker-compose.local.yml up -d postgres-dev
	@echo "✓ PostgreSQL server started!"
	@echo "Connection details:"
	@echo "  Host: localhost"
	@echo "  Port: 5432"
	@echo "  Database: aiwebengine"
	@echo "  User: aiwebengine"
	@echo "  Password: devpassword"
	@echo ""
	@echo "Connection string: postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# Stop PostgreSQL in local environment
postgres-local-stop:
	@echo "Stopping PostgreSQL server..."
	docker-compose -f docker-compose.local.yml stop postgres-dev
	@echo "✓ PostgreSQL server stopped!"

# View PostgreSQL logs in local environment
postgres-local-logs:
	docker-compose -f docker-compose.local.yml logs -f postgres-dev

build-locally-deploy-prod:
	@echo "Building production Docker image for amd64 platform using Buildx..."
	@DOCKER_HOST='' docker buildx inspect aiwebengine-builder >/dev/null 2>&1 || \
		DOCKER_HOST='' docker buildx create --name aiwebengine-builder --bootstrap
	@DOCKER_HOST='' docker buildx build --builder aiwebengine-builder --platform linux/amd64 -t aiwebengine:latest --load .
	@DOCKER_HOST='' docker save aiwebengine:latest -o aiwebengine_latest.tar
	scp aiwebengine_latest.tar softagen:/tmp/
	ssh softagen 'docker load -i /tmp/aiwebengine_latest.tar && rm /tmp/aiwebengine_latest.tar'
	@echo "✓ Docker amd64 image built and copied to remote server!"
