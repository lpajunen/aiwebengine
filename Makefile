# Makefile for aiwebengine development

.PHONY: help deps test dev build clean lint format coverage check ci
.PHONY: docker-build docker-local docker-staging docker-prod docker-stop docker-logs docker-clean

help:
	@echo "Available commands:"
	@echo ""
	@echo "Development:"
	@echo "  make deps      - Install development tools (cargo-watch, cargo-nextest, cargo-llvm-cov)"
	@echo "  make dev       - Run development server with auto-reload"
	@echo "  make test      - Run all tests with cargo-nextest"
	@echo "  make test-simple - Run all tests with cargo test"
	@echo "  make lint      - Run clippy linter"
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
	cargo watch -x 'run --bin server'

# Run tests with cargo-nextest (better output)
test:
	cargo nextest run --all-features --no-fail-fast

# Run tests with standard cargo test
test-simple:
	cargo test --all-features

# Run clippy linter with warnings as errors
lint:
	cargo clippy --all-targets -- -D warnings

# Format all code
format:
	cargo fmt --all

# Check formatting without modifying files
format-check:
	cargo fmt --all -- --check

format-markdown:
	./node_modules/.bin/prettier --write "**/*.md"

format-javascript:
	./node_modules/.bin/prettier --write "**/*.js"

# Generate test coverage report
coverage:
	cargo llvm-cov --all-features --html
	@echo "Coverage report generated: target/llvm-cov/html/index.html"

# Build release binary
build:
	cargo build --release

# Clean build artifacts
clean:
	cargo clean

# Pre-commit checks (format check, lint, test)
check: format-check lint test-simple
	@echo "✓ All checks passed!"

# CI pipeline (format check, lint, test, coverage)
ci: format-check lint test-simple coverage
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

# Build staging Docker image
docker-build-staging:
	@echo "Building staging Docker image..."
	docker build -f Dockerfile.staging -t aiwebengine:staging .
	@echo "✓ Staging Docker image built successfully!"

# Start local/development environment with Docker Compose
docker-local:
	@echo "Starting local/development environment..."
	docker-compose -f docker-compose.local.yml up

# Start local/development environment in background
docker-local-bg:
	@echo "Starting local/development environment in background..."
	docker-compose -f docker-compose.local.yml up -d
	@echo "✓ Local/development environment started!"
	@echo "View logs with: make docker-logs-local"

# Start staging environment with Docker Compose
docker-staging:
	@echo "Starting staging environment..."
	docker-compose -f docker-compose.staging.yml up -d
	@echo "✓ Staging environment started!"
	@echo "View logs with: make docker-logs-staging"

# Start production environment with Docker Compose
docker-prod:
	@echo "Starting production environment..."
	docker-compose up -d
	@echo "✓ Production environment started!"
	@echo "View logs with: make docker-logs"

# Stop all Docker containers
docker-stop:
	@echo "Stopping Docker containers..."
	docker-compose down
	docker-compose -f docker-compose.staging.yml down
	docker-compose -f docker-compose.local.yml down
	@echo "✓ All containers stopped!"

# View production logs
docker-logs:
	docker-compose logs -f aiwebengine

# View local/development logs
docker-logs-local:
	docker-compose -f docker-compose.local.yml logs -f aiwebengine-dev

# View staging logs
docker-logs-staging:
	docker-compose -f docker-compose.staging.yml logs -f aiwebengine-staging

# View all service logs
docker-logs-all:
	docker-compose logs -f

# Clean up Docker containers and volumes
docker-clean:
	@echo "Cleaning up Docker containers and volumes..."
	docker-compose down -v
	docker-compose -f docker-compose.staging.yml down -v
	docker-compose -f docker-compose.local.yml down -v
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
	docker-compose -f docker-compose.staging.yml exec aiwebengine-staging /bin/bash

# Run tests in Docker container
docker-test:
	docker-compose -f docker-compose.local.yml run --rm aiwebengine-dev cargo test

# Check Docker container status
docker-ps:
	@echo "Production containers:"
	@docker-compose ps
	@echo ""
	@echo "Staging containers:"
	@docker-compose -f docker-compose.staging.yml ps
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
	@if [ ! -f .env ]; then \
		cp .env.example .env; \
		echo "✓ Created .env file from .env.example"; \
		echo "⚠ Please edit .env and set your credentials!"; \
	else \
		echo ".env file already exists"; \
	fi

# Complete Docker setup for first-time use
docker-setup: docker-env docker-build
	@echo "✓ Docker setup completed!"
	@echo "You can now run: make docker-prod"
