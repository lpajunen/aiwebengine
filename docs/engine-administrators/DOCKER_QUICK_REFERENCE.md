# Docker Quick Reference

## Quick Start Commands

```bash
# First-time setup
make docker-setup

# Start production
make docker-prod

# Start development (hot-reload)
make docker-dev

# View logs
make docker-logs

# Stop all containers
make docker-stop
```

## Common Makefile Commands

### Setup & Build

| Command                 | Description                                   |
| ----------------------- | --------------------------------------------- |
| `make docker-setup`     | First-time setup (creates .env, builds image) |
| `make docker-build`     | Build production Docker image                 |
| `make docker-build-dev` | Build development Docker image                |
| `make docker-env`       | Create .env from .env.example                 |

### Running Services

| Command               | Description                              |
| --------------------- | ---------------------------------------- |
| `make docker-prod`    | Start production environment (detached)  |
| `make docker-dev`     | Start development environment (attached) |
| `make docker-dev-bg`  | Start development environment (detached) |
| `make docker-stop`    | Stop all Docker containers               |
| `make docker-restart` | Restart production containers            |

### Logs & Monitoring

| Command                | Description                    |
| ---------------------- | ------------------------------ |
| `make docker-logs`     | View production logs (follow)  |
| `make docker-logs-dev` | View development logs (follow) |
| `make docker-logs-all` | View all service logs          |
| `make docker-ps`       | Check container status         |
| `make docker-stats`    | Show resource usage            |

### Development

| Command                 | Description                         |
| ----------------------- | ----------------------------------- |
| `make docker-shell`     | Open shell in production container  |
| `make docker-shell-dev` | Open shell in development container |
| `make docker-test`      | Run tests in Docker container       |

### Cleanup

| Command                    | Description                        |
| -------------------------- | ---------------------------------- |
| `make docker-clean`        | Stop containers and remove volumes |
| `make docker-clean-images` | Remove Docker images               |
| `make docker-clean-all`    | Full cleanup (containers + images) |
| `make docker-rebuild`      | Rebuild and restart production     |

## Raw Docker Commands

### Production

```bash
# Build
docker build -t aiwebengine:latest .

# Run single container
docker run -d -p 3000:3000 --name aiwebengine aiwebengine:latest

# Using docker-compose
docker-compose up -d
docker-compose down
docker-compose logs -f aiwebengine
```

### Development

```bash
# Build development image
docker build -f Dockerfile.dev -t aiwebengine:dev .

# Using docker-compose
docker-compose -f docker-compose.dev.yml up
docker-compose -f docker-compose.dev.yml down
```

## Environment Variables

Create `.env` file:

```bash
cp .env.example .env
```

Required variables:

```bash
# OAuth (at least one provider)
GOOGLE_CLIENT_ID=your-client-id
GOOGLE_CLIENT_SECRET=your-secret

# Security (generate with: openssl rand -base64 32)
JWT_SECRET=your-random-secret
SESSION_SECRET=your-random-secret

# Optional: Database
POSTGRES_PASSWORD=secure-password
```

## Service URLs

| Service      | URL                          | Notes                 |
| ------------ | ---------------------------- | --------------------- |
| Application  | http://localhost:3000        | Main API              |
| Health Check | http://localhost:3000/health | Status endpoint       |
| PostgreSQL   | localhost:5432               | Database (optional)   |
| Redis        | localhost:6379               | Cache (optional)      |
| Prometheus   | http://localhost:9090        | Metrics (optional)    |
| Grafana      | http://localhost:3001        | Dashboards (optional) |

## Troubleshooting

### Container won't start

```bash
# Check logs
make docker-logs

# Rebuild from scratch
make docker-clean-all
make docker-build
make docker-prod
```

### Permission errors

```bash
# Fix ownership
sudo chown -R $(id -u):$(id -g) logs/ data/ scripts/
```

### Port already in use

```bash
# Find process using port 3000
lsof -i :3000

# Or change port in docker-compose.yml
ports:
  - "3001:3000"
```

### Out of disk space

```bash
# Remove unused Docker resources
docker system prune -a --volumes

# Check disk usage
docker system df
```

## Configuration Files

| File                     | Purpose                                          |
| ------------------------ | ------------------------------------------------ |
| `Dockerfile`             | Production image build                           |
| `Dockerfile.dev`         | Development image build                          |
| `docker-compose.yml`     | Production stack                                 |
| `docker-compose.dev.yml` | Development stack                                |
| `.dockerignore`          | Files excluded from build                        |
| `.env`                   | Environment variables (create from .env.example) |
| `config.prod.toml`       | Production configuration                         |
| `config.dev.toml`        | Development configuration                        |

## Best Practices

1. **Always use `.env` for secrets** - Never commit secrets to git
2. **Use docker-compose for local development** - Easier than raw docker commands
3. **Check logs when issues occur** - `make docker-logs`
4. **Clean up regularly** - `make docker-clean` to free resources
5. **Use dev environment for development** - `make docker-dev` has hot-reload
6. **Generate strong secrets** - `openssl rand -base64 32`

## Quick Workflows

### First-Time Production Setup

```bash
make docker-setup
nano .env  # Edit with your credentials
make docker-prod
make docker-logs  # Verify it's running
```

### Daily Development

```bash
make docker-dev  # Starts with hot-reload
# Edit code, changes auto-reload
# Ctrl+C to stop
```

### Deploying Updates

```bash
git pull
make docker-rebuild
```

### Debugging Issues

```bash
make docker-logs        # Check logs
make docker-ps          # Check status
make docker-shell       # Access container
make docker-stats       # Check resources
```

## Getting Help

- Full documentation: `docs/DOCKER.md`
- Development guide: `docs/local-development.md`
- Configuration: `docs/CONFIGURATION.md`
- Requirements: `REQUIREMENTS.md`
