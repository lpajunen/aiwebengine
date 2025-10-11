# Makefile for aiwebengine development

.PHONY: help deps test dev build clean lint format coverage check ci

help:
	@echo "Available commands:"
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

# Install development dependencies
deps:
	@echo "Installing development tools..."
	cargo install cargo-watch
	cargo install cargo-nextest
	cargo install cargo-llvm-cov
	@echo "Development tools installed successfully!"

# Run development server with auto-reload
dev:
	cargo watch -x 'run --bin server'

# Run tests with cargo-nextest (better output)
test:
	cargo nextest run --all-features

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
