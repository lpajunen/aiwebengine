# Makefile for aiwebengine development

.PHONY: help deps upgrade-deps test dev build clean lint format coverage check ci typecheck
.PHONY: check-embedded build-desktop run-desktop init-desktop
.PHONY: docker-build docker-local docker-staging docker-prod docker-stop docker-stop-local
.PHONY: docker-logs docker-logs-local docker-logs-staging docker-logs-all
.PHONY: docker-clean docker-clean-local docker-clean-images docker-clean-all
.PHONY: docker-shell docker-shell-local docker-shell-staging docker-ps docker-restart
.PHONY: docker-rebuild docker-stats docker-env docker-setup
.PHONY: docker-backup docker-backup-list docker-backup-fetch docker-restore
.PHONY: docker-pull docker-deploy check-shell-env check-mounts
.PHONY: postgres-local postgres-local-stop postgres-local-logs
.PHONY: docker-dns check-dns clean-acme-dns

help:
	@echo "Available commands:"
	@echo ""
	@echo "Development:"
	@echo "  make deps         - Install development tools (cargo-watch, cargo-nextest, cargo-llvm-cov)"
	@echo "  make upgrade-deps - Upgrade npm packages to latest versions"
	@echo "  make dev       - Run development server with auto-reload"
	@echo "  make dev-local - Run development server with localhost OAuth (http://localhost:3000)"
	@echo "  make test      - Run all tests with cargo-nextest"
	@echo "  make test-simple - Run all tests with cargo test"
	@echo "  make perf-test   - Run performance/load test against production server"
	@echo "  make lint        - Run clippy linter"
	@echo "  make typecheck   - Run TypeScript declaration checks"
	@echo "  make format    - Format code with rustfmt"
	@echo "  make format-check - Check code formatting without modifying"
	@echo "  make coverage  - Generate test coverage report"
	@echo "  make build     - Build release binary"
	@echo "  make build-desktop - Build the standalone desktop binary (embedded PostgreSQL)"
	@echo "  make run-desktop   - Run it; creates its config and keys on first launch"
	@echo "  make init-desktop  - Create that config without starting anything"
	@echo "  make clean     - Clean build artifacts"
	@echo "  make check     - Every gate CI runs (format, lint, typecheck, embedded, test)"
	@echo "  make ci        - make check, plus coverage"
	@echo ""
	@echo "Docker, server environments (ENV=production by default; ENV=staging for the other):"
	@echo "  make docker-build        - Build the production image"
	@echo "  make docker-prod         - Start production"
	@echo "  make docker-staging      - Start staging"
	@echo "  make docker-logs         - Follow the engine logs        (ENV=...)"
	@echo "  make docker-ps           - Container status              (ENV=...)"
	@echo "  make docker-shell        - Shell in an engine container  (ENV=...)"
	@echo "  make docker-pull         - Fetch the image the env file names (ENV=...)"
	@echo "  make docker-deploy       - Roll the engines one at a time    (ENV=...)"
	@echo "  make docker-restart      - Restart every instance at once (ENV=...)"
	@echo "  make docker-rebuild      - Recreate against the current image (ENV=...)"
	@echo "  make docker-stop         - Stop one environment          (ENV=...)"
	@echo "  make docker-clean        - Remove it AND its database    (ENV=... CONFIRM=yes)"
	@echo ""
	@echo "Backups (the env file is the other half — it holds the decryption key):"
	@echo "  make docker-backup       - Take a dump now                (ENV=...)"
	@echo "  make docker-backup-list  - List the dumps held            (ENV=...)"
	@echo "  make docker-backup-fetch - Copy the newest one to ./backups (ENV=...)"
	@echo "  make docker-restore      - Restore one (ENV=... FILE=... CONFIRM=yes)"
	@echo ""
	@echo "Docker, development stack:"
	@echo "  make docker-local        - Start the containerised development stack"
	@echo "  make docker-logs-local   - Follow its engine logs"
	@echo "  make docker-shell-local  - Shell in an engine container"
	@echo "  make docker-stop-local   - Stop it"
	@echo "  make docker-clean-local  - Stop it and delete its volumes"
	@echo "  make postgres-local      - Start only PostgreSQL, for host-side cargo run"
	@echo "  make postgres-local-stop - Stop it"
	@echo "  make docker-localhost    - The same as docker-local: Caddy's own CA, no DNS"
	@echo "  make docker-dns          - A real certificate for a real name (DIGITALOCEAN_TOKEN)"
	@echo "  make check-dns           - Check the DNS name resolves"
	@echo "  make clean-acme-dns      - Remove stale ACME challenge records from the zone"

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
# 13 MB archive into every build that ran it. Compile-checked separately by
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

# Run clippy linter with warnings as errors.
#
# markdownlint globs the working tree rather than reading .gitignore, so the
# ignored directories that hold third-party content are named here: data/ is
# where a desktop install unpacks PostgreSQL, README and all.
lint:
	cargo clippy --all-targets -- -D warnings
	./node_modules/.bin/markdownlint "**/*.md" --ignore node_modules --ignore data --ignore target && echo '✓ Markdown files linted'

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
# platform archive compiled in so a first launch needs no network. Adds about
# 13 MB to the binary (the archive is compressed; it extracts to ~43 MB on first
# run), and needs network access at build time to stage it.
build-desktop:
	cargo build --release --features embedded-postgres-bundled

# Run that binary as a desktop install.
#
# One flag, and everything it needs it makes for itself: `--desktop` creates
# the configuration and its four keys in the platform's application-data
# directory on first launch, then starts a PostgreSQL of its own beside it.
#
# This target used to generate `.env-desktop` with `openssl rand`, which is the
# Makefile standing in for a first-run path the binary did not have. It has one
# now (src/desktop.rs), so a packaged application — which ships no checkout, no
# toolchain and no make — reaches the same install this does.
#
# AIWEBENGINE_DATA_DIR names somewhere else, for a second install or a
# throwaway one.
run-desktop: build-desktop
	./target/release/aiwebengine --desktop

# Create the desktop configuration without starting anything, and say where it
# went. Idempotent: an existing one is left exactly as it is, because
# regenerating secret_encryption_key makes every stored secret unreadable.
init-desktop: build-desktop
	./target/release/aiwebengine --init-config

# Clean build artifacts
clean:
	cargo clean

# Pre-commit checks (format check, lint, test)
# The same gates CI runs, in the same order. check-embedded is one of them:
# the supervisor is compiled by no other target, so a change that breaks it is
# invisible until a desktop build.
check: format-check lint typecheck check-embedded test
	@echo "✓ All checks passed!"

# CI pipeline (format check, lint, test, coverage)
ci: format-check lint typecheck test check-embedded coverage
	@echo "✓ CI pipeline completed!"

# ==================== Docker Commands ====================

# Build production Docker image
# How every docker target reaches a stack.
#
# No target below writes a compose invocation of its own, and that is not
# tidiness. An env file carries COMPOSE_PROJECT_NAME, COMPOSE_PROFILES and
# ENGINE_IMAGE, and a bare `docker compose` reads none of them: it names a
# different project, so it lists no containers, stops nothing, and leaves the
# second instance out of every answer. Which server environment a target acts
# on is ENV:
#
#   make docker-logs                 # production, the default
#   make docker-logs ENV=staging     # the same target, the other deployment
ENV ?= production

# Named explicitly rather than left to the default, so that a COMPOSE_FILE a
# developer picked up by sourcing something cannot redirect a deployment
# command at a different stack. A deployment that needs an overlay — a managed
# database, say — passes its own list:
#   make docker-prod COMPOSE_FILES="-f docker-compose.yml -f docker-compose.managed.yml"
COMPOSE_FILES ?= -f docker-compose.yml
SERVER_COMPOSE = docker compose --env-file .env-$(ENV) $(COMPOSE_FILES)

# The development stack: the same compose file with the development overlay on
# top, and the handful of values Caddy reads supplied on the command line.
#
# On the command line, and not in .env-local, because .env-local is a file
# developers *source* — `source .env-local && cargo run` is the documented
# workflow. A SITE_HOSTS or COMPOSE_PROJECT_NAME left in a shell that way
# outranks the --env-file of every later compose command, including the ones
# that deploy; setting them per invocation cannot leak. It is the same reason
# docker-compose.yml reads ENGINE_DATABASE_URL rather than reusing the name the
# engine's own variable has.
DEV_PROJECT     ?= aiwebengine-local
DEV_SITE_HOSTS  ?= localhost, 127.0.0.1, local.test
DEV_MANAGE_HOST ?= localhost
DEV_TLS_SNIPPET ?= tls_internal
# Two instances by default: rehearsing the deployment is what this stack is
# for, and cross-instance cache invalidation over LISTEN/NOTIFY is one of the
# things only a second instance exercises. For the single-node shape:
#   make docker-local DEV_PROFILES= DEV_UPSTREAMS=aiwebengine-1:3000
DEV_PROFILES    ?= ha
DEV_UPSTREAMS   ?= aiwebengine-1:3000 aiwebengine-2:3000

DEV_COMPOSE = ENV_FILE=.env-local \
	COMPOSE_PROFILES="$(DEV_PROFILES)" \
	SITE_HOSTS="$(DEV_SITE_HOSTS)" \
	MANAGE_HOST="$(DEV_MANAGE_HOST)" \
	TLS_SNIPPET="$(DEV_TLS_SNIPPET)" \
	ENGINE_UPSTREAMS="$(DEV_UPSTREAMS)" \
	POSTGRES_PASSWORD=devpassword \
	ENGINE_IMAGE=aiwebengine:dev \
	docker compose --env-file .env-local -p $(DEV_PROJECT) \
		-f docker-compose.yml -f docker-compose.local.yml

# The engine services this deployment actually runs, asked of compose rather
# than written down. The second instance is behind the `ha` profile, so naming
# it always fails on a single-node deployment and naming only the first hides
# half the logs on a clustered one; `config --services` lists what the active
# profiles select, which is the same source of truth the stack itself uses.
ENGINE_SERVICES = $$($(SERVER_COMPOSE) config --services | grep '^aiwebengine')
DEV_ENGINE_SERVICES = $$($(DEV_COMPOSE) config --services | grep '^aiwebengine')

docker-build:
	@echo "Building production Docker image..."
	docker build -t aiwebengine:latest .
	@echo "✓ Docker image built successfully!"

# Build the development stack's images: the engine's, and the Caddy image with
# the DNS plugin. Through compose rather than `docker build`, since the overlay
# is what names them.
docker-build-local:
	@echo "Building the development images..."
	$(DEV_COMPOSE) build
	@echo "✓ Built."

# Build staging Docker image (uses production Dockerfile)
docker-build-staging:
	@echo "Building staging Docker image..."
	docker build -t aiwebengine:staging .
	@echo "✓ Staging Docker image built successfully!"

# Start local/development environment with Docker Compose
# Start the containerised development stack: the server topology, built from
# source. Two engine instances behind Caddy, plus Postgres — the same compose
# file production runs, with the development overlay on top.
#
# A cold start compiles the crate before anything listens, which takes a while;
# the containers report "starting" until it finishes.
docker-local: docker-localhost

# Local names, Caddy's own CA: no DNS, no network, one browser warning.
docker-localhost:
	@echo "Starting the development stack on https://localhost"
	@echo "  A self-signed certificate — expect the browser warning."
	@echo "  Serve a name of your own: make docker-localhost DEV_SITE_HOSTS=my.example.test"
	$(DEV_COMPOSE) up --build

# A publicly trusted certificate for a name that resolves to a private address,
# through a DNS-01 challenge. Use it when a real certificate and a real
# hostname matter: OAuth redirect URIs a provider will accept, MCP clients that
# refuse self-signed certificates, and anything testing the __Host- cookie
# prefix, which requires Secure.
#
# The hostname and the issuer move together — a DNS-01 certificate for
# `localhost` is not obtainable, and Let's Encrypt will not issue for a name it
# cannot verify — so this target sets both, and the local names are not served
# in this mode.
DNS_DOMAIN ?= local.softagen.com
docker-dns: DEV_SITE_HOSTS = $(DNS_DOMAIN)
docker-dns: DEV_MANAGE_HOST = $(DNS_DOMAIN)
docker-dns: DEV_TLS_SNIPPET = tls_acme_dns
docker-dns:
	@if [ -z "$$DIGITALOCEAN_TOKEN" ]; then \
		echo "❌ DIGITALOCEAN_TOKEN is not set — the DNS-01 challenge needs it."; \
		echo "   export DIGITALOCEAN_TOKEN=..."; \
		exit 1; \
	fi
	@bash scripts/acme-dns-cleanup.sh
	@echo "Starting the development stack on https://$(DNS_DOMAIN)"
	@echo "  The local names are not served in this mode: one block, one issuer."
	$(DEV_COMPOSE) up --build

# Remove leftover ACME challenge records by hand. `docker-dns` runs this first,
# so this target is for clearing the zone without starting the stack.
clean-acme-dns:
	@bash scripts/acme-dns-cleanup.sh

# Check DNS domain availability
check-dns:
	@bash scripts/check-dns.sh

# The same, detached.
docker-local-bg:
	@echo "Starting the development stack in the background..."
	$(DEV_COMPOSE) up -d --build
	@echo "✓ Started. Logs: make docker-logs-local"

# Start staging environment with Docker Compose
# Each server environment is one env file: it supplies both the values compose
# interpolates and, through ENV_FILE, the environment the containers receive.
# Docker creates a missing bind-mount source as a directory, and mounting a
# directory onto a file inside the image fails with a runc error that names
# overlay2 paths and explains nothing. Catch it here, where the fix is obvious.
# Refuse to act on a deployment while the shell is carrying values that would
# outrank its env file.
#
# Compose reads these from the environment in preference to --env-file, so a
# shell that has sourced a development env file — the documented way to run the
# engine on the host — silently redirects a deployment command: the wrong
# project, the wrong compose files, the wrong hostnames, or `env_file:` pointing
# at throwaway keys. The failure is invisible at the point it happens and
# obvious only afterwards, which is why this is a hard stop rather than a
# warning. Set ALLOW_SHELL_ENV=yes if you meant it.
COMPOSE_SHELL_VARS = ENV_FILE COMPOSE_FILE COMPOSE_PROJECT_NAME COMPOSE_PROFILES \
	SITE_HOSTS MANAGE_HOST TLS_SNIPPET ENGINE_UPSTREAMS ENGINE_IMAGE \
	ENGINE_DATABASE_URL POSTGRES_PASSWORD POSTGRES_DB

check-shell-env:
	@if [ "$(ALLOW_SHELL_ENV)" = "yes" ]; then exit 0; fi; \
	found=""; \
	for v in $(COMPOSE_SHELL_VARS); do \
		eval "val=\$$$$v"; \
		[ -n "$$val" ] && found="$$found $$v"; \
	done; \
	if [ -n "$$found" ]; then \
		echo "❌ These are set in this shell and outrank .env-$(ENV):"; \
		for v in $$found; do eval "echo \"     $$v=\$$$$v\""; done; \
		echo "   Compose reads them from the environment first, so this command"; \
		echo "   would act on something other than the $(ENV) deployment."; \
		echo "   Use a shell that has not sourced an env file, or ALLOW_SHELL_ENV=yes."; \
		exit 1; \
	fi

check-mounts: check-shell-env
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

# Start a server environment. Same file, same image, same shape — the env file
# is the only difference between the two.
docker-staging: ENV = staging
docker-staging: check-mounts
	@echo "Starting the staging environment..."
	$(SERVER_COMPOSE) up -d --build
	@echo "✓ Staging started. Logs: make docker-logs ENV=staging"

docker-prod: ENV = production
docker-prod: check-mounts
	@echo "Starting the production environment..."
	$(SERVER_COMPOSE) up -d --build
	@echo "✓ Production started. Logs: make docker-logs"

# Stop one server environment. The development stack is a separate project and
# is stopped separately, so that stopping a deployment cannot take a colleague's
# development containers with it.
docker-stop: check-shell-env
	@echo "Stopping the $(ENV) environment..."
	$(SERVER_COMPOSE) down
	@echo "✓ Stopped. The development stack: make docker-stop-local"

docker-stop-local:
	@echo "Stopping the development stack..."
	$(DEV_COMPOSE) down
	@echo "✓ Stopped."

# Follow the engine logs of one server environment.
docker-logs: check-shell-env
	$(SERVER_COMPOSE) logs -f $(ENGINE_SERVICES)

docker-logs-staging:
	@$(MAKE) --no-print-directory docker-logs ENV=staging

# Follow the development stack's engines.
docker-logs-local:
	$(DEV_COMPOSE) logs -f $(DEV_ENGINE_SERVICES)

# Every service, proxy and database included. Caddy's access log goes to a file
# rather than stdout, so this is quieter than it looks.
docker-logs-all: check-shell-env
	$(SERVER_COMPOSE) logs -f

# Remove one server environment's containers *and its volumes*, which includes
# postgres-data: every script, asset, user, secret, log and revision it holds.
#
# The guard is here because this target used to be harmless by accident. It ran
# a bare `docker-compose down -v`, which — with COMPOSE_PROJECT_NAME set in the
# env file — named a project that did not exist and deleted nothing. Now that
# it reaches the real project, it does exactly what it says.
docker-clean: check-shell-env
	@if [ "$(CONFIRM)" != "yes" ]; then \
		echo "This deletes the $(ENV) database: scripts, assets, users, secrets, logs."; \
		echo "Take a dump first (make docker-backup ENV=$(ENV)), then:"; \
		echo "  make docker-clean ENV=$(ENV) CONFIRM=yes"; \
		exit 1; \
	fi
	@echo "Removing the $(ENV) environment and its volumes..."
	$(SERVER_COMPOSE) down -v
	@echo "✓ Removed."

# The development stack's data is throwaway by construction, so no guard.
docker-clean-local:
	@echo "Removing the development stack and its volumes..."
	$(DEV_COMPOSE) down -v
	@echo "✓ Removed."

# Clean up Docker images
docker-clean-images:
	@echo "Removing Docker images..."
	docker rmi aiwebengine:latest aiwebengine:staging aiwebengine:dev 2>/dev/null || true
	@echo "✓ Docker images removed!"

# Full cleanup of the development stack. Deliberately not the server ones: a
# target that removes a deployment's database should be typed out in full.
docker-clean-all: docker-clean-local docker-clean-images
	@echo "✓ Development cleanup completed!"

# Open a shell in a running engine container.
docker-shell: check-shell-env
	$(SERVER_COMPOSE) exec aiwebengine-1 /bin/bash

docker-shell-staging:
	@$(MAKE) --no-print-directory docker-shell ENV=staging

docker-shell-local:
	$(DEV_COMPOSE) exec aiwebengine-1 /bin/bash

# Check container status
docker-ps: check-shell-env
	@echo "$(ENV) containers:"
	@$(SERVER_COMPOSE) ps
	@echo ""
	@echo "Development containers:"
	@$(DEV_COMPOSE) ps

# Restart one server environment, all instances at once. For a deployment that
# should stay reachable while it restarts, use `make docker-deploy` instead.
docker-restart: check-shell-env
	$(SERVER_COMPOSE) restart

# Recreate one server environment against the current image and configuration.
# `up` rather than `down` then `up`: compose replaces only what changed, and a
# `down` first would take the database with it for no reason.
docker-rebuild: check-shell-env
	@echo "Recreating the $(ENV) environment..."
	$(SERVER_COMPOSE) up -d --build --force-recreate
	@echo "✓ Recreated."

# Fetch the image the env file names, without restarting anything. Separate
# from the roll below so that a slow pull is not part of the window in which
# one instance is down.
docker-pull: check-shell-env
	$(SERVER_COMPOSE) pull $(ENGINE_SERVICES)

# Roll the engines one at a time, waiting for each to come back before touching
# the next.
#
# `docker compose up -d` on its own recreates every instance at once, which on
# a clustered deployment throws away the one property the second instance is
# there for. This replaces them in sequence and waits on the container's own
# health check between them, so the deployment is never without an instance
# that has finished starting.
#
# What carries the requests across the gap is in the Caddyfile, not here: an
# instance closes its listener before it finishes draining, and `lb_try_duration`
# is what retries the refused dial on the other upstream instead of answering
# 502. `stop_grace_period` in the compose file is what lets the drain finish.
#
# On a single-instance deployment this is a restart with a gap in it, and says
# so — there is nowhere for the requests to go.
docker-deploy: check-mounts
	@svcs="$(ENGINE_SERVICES)"; \
	count=$$(echo $$svcs | wc -w | tr -d ' '); \
	if [ "$$count" -lt 2 ]; then \
		echo "⚠ $(ENV) runs one engine instance, so this restart has a gap in it."; \
		echo "  For a rolling one, set COMPOSE_PROFILES=ha and name both in ENGINE_UPSTREAMS."; \
	fi; \
	for svc in $$svcs; do \
		echo "→ replacing $$svc"; \
		$(SERVER_COMPOSE) up -d --no-deps --force-recreate "$$svc" || exit 1; \
		cid=$$($(SERVER_COMPOSE) ps -q "$$svc"); \
		[ -n "$$cid" ] || { echo "  $$svc did not start"; exit 1; }; \
		i=0; stop=""; \
		while [ -z "$$stop" ]; do \
			state=$$(docker inspect -f '{{.State.Status}}' $$cid 2>/dev/null); \
			health=$$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' $$cid 2>/dev/null); \
			case "$$state/$$health" in \
				running/healthy) stop=ok;; \
				running/none) echo "  $$svc has no health check; taking running as up"; stop=ok;; \
				running/starting) ;; \
				running/unhealthy) stop="$$svc came up unhealthy";; \
				*) stop="$$svc is $$state";; \
			esac; \
			[ -n "$$stop" ] && break; \
			i=$$((i + 1)); \
			[ "$$i" -lt 120 ] || stop="$$svc was still starting after four minutes"; \
			[ -n "$$stop" ] || sleep 2; \
		done; \
		if [ "$$stop" != "ok" ]; then \
			echo "  $$stop — stopping the roll here."; \
			echo "  The instances already replaced are running, so the deployment"; \
			echo "  is serving from a mix. Look, then roll again or roll back:"; \
			echo "    make docker-logs ENV=$(ENV)"; \
			exit 1; \
		fi; \
		echo "  $$svc healthy"; \
	done; \
	echo "✓ $(ENV) rolled."

# Backups.
#
# The `backup` profile runs dumps on a schedule inside the stack; these targets
# are for the things a schedule cannot do — taking one now, seeing what is
# there, copying one off the machine, and putting one back.
#
# A dump is half of a backup. The other half is the environment file: it holds
# security.secret_encryption_key, and every script and user secret in the dump
# is ciphertext without it. Keep the two together and store them apart.

# Take a dump now, into the same volume the scheduled ones use.
docker-backup: check-shell-env
	@echo "Dumping the $(ENV) database..."
	$(SERVER_COMPOSE) run --rm backup once

# What dumps the volume holds, newest first.
docker-backup-list: check-shell-env
	@$(SERVER_COMPOSE) run --rm backup list

# Copy the newest dump out of the volume and onto this machine. A backup that
# has never left the host it is a backup of is not one yet.
#
# `run --rm` with the volume attached and the output on stdout, rather than
# `docker cp`: cp names a container, and the scheduled one may not be running
# on a deployment that takes dumps by hand.
docker-backup-fetch: check-shell-env
	@mkdir -p backups
	@name=$$($(SERVER_COMPOSE) run --rm -T backup list | head -1 | tr -d '\r'); \
	case "$$name" in \
		"" | "no dumps"*) echo "No dumps to fetch. Take one: make docker-backup ENV=$(ENV)"; exit 1;; \
	esac; \
	out="backups/$$(basename $$name)"; \
	echo "Fetching $$name -> $$out"; \
	$(SERVER_COMPOSE) run --rm -T --entrypoint sh backup -c "cat $$name" > "$$out"; \
	echo "✓ $$out ($$(du -h "$$out" | cut -f1))"; \
	echo "  Store the env file with it: .env-$(ENV) holds the key that decrypts its secrets."

# Put a dump back. FILE names one inside the volume (see docker-backup-list).
#
# The engines are stopped first and started after, and that is not politeness:
# a restore drops and recreates every table while instances are executing
# scripts against them, and what comes out the other side is neither the dump
# nor what was there before.
docker-restore: check-shell-env
	@if [ -z "$(FILE)" ]; then \
		echo "Which dump? See: make docker-backup-list ENV=$(ENV)"; \
		echo "  make docker-restore ENV=$(ENV) FILE=aiwebengine-....dump CONFIRM=yes"; \
		exit 1; \
	fi
	@if [ "$(CONFIRM)" != "yes" ]; then \
		echo "This REPLACES the $(ENV) database with $(FILE)."; \
		echo "  make docker-restore ENV=$(ENV) FILE=$(FILE) CONFIRM=yes"; \
		exit 1; \
	fi
	@echo "Stopping the engines..."
	$(SERVER_COMPOSE) stop $(ENGINE_SERVICES)
	$(SERVER_COMPOSE) run --rm backup restore "$(FILE)"
	@echo "Starting the engines..."
	$(SERVER_COMPOSE) start $(ENGINE_SERVICES)
	@echo "✓ Restored. Check it answers: make docker-logs ENV=$(ENV)"

# Show Docker resource usage
docker-stats: check-shell-env
	docker stats $$($(SERVER_COMPOSE) ps -q)

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
	$(DEV_COMPOSE) up -d postgres
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
	$(DEV_COMPOSE) stop postgres
	@echo "✓ PostgreSQL server stopped!"

# View PostgreSQL logs in local environment
postgres-local-logs:
	$(DEV_COMPOSE) logs -f postgres

build-locally-deploy-prod:
	@echo "Building production Docker image for amd64 platform using Buildx..."
	@DOCKER_HOST='' docker buildx inspect aiwebengine-builder >/dev/null 2>&1 || \
		DOCKER_HOST='' docker buildx create --name aiwebengine-builder --bootstrap
	@DOCKER_HOST='' docker buildx build --builder aiwebengine-builder --platform linux/amd64 -t aiwebengine:latest --load .
	@DOCKER_HOST='' docker save aiwebengine:latest -o aiwebengine_latest.tar
	scp aiwebengine_latest.tar softagen:/tmp/
	ssh softagen 'docker load -i /tmp/aiwebengine_latest.tar && rm /tmp/aiwebengine_latest.tar'
	@echo "✓ Docker amd64 image built and copied to remote server!"
