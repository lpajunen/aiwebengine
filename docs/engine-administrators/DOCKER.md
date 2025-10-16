# Docker Deployment Guide

This guide explains how to deploy aiwebengine using Docker and Docker Compose.

## Table of Contents

- [Quick Start](#quick-start)
- [Production Deployment](#production-deployment)
- [Development Deployment](#development-deployment)
- [Configuration](#configuration)
- [Services](#services)
- [Monitoring](#monitoring)
- [Troubleshooting](#troubleshooting)

## Quick Start

### Prerequisites

- Docker 20.10+
- Docker Compose 2.0+
- At least 2GB of available RAM

### Production Deployment

1. **Clone the repository:**

   ```bash
   git clone <repository-url>
   cd aiwebengine
   ```

2. **Configure environment variables:**

   ```bash
   cp .env.example .env
   # Edit .env and set your OAuth credentials and secrets
   nano .env
   ```

3. **Build and start the services:**

   ```bash
   docker-compose up -d
   ```

4. **Check the logs:**

   ```bash
   docker-compose logs -f aiwebengine
   ```

5. **Access the application:**
   - API: http://localhost:3000
   - Health check: http://localhost:3000/health

### Development Deployment

For development with hot-reload:

```bash
docker-compose -f docker-compose.dev.yml up
```

This will start the development server with:

- Hot-reload on code changes
- Debug logging enabled
- Source code mounted as volumes
- Development database

## Production Deployment

### Docker Files

The project includes several Docker-related files:

- **Dockerfile**: Multi-stage production build
- **Dockerfile.dev**: Development build with hot-reload
- **docker-compose.yml**: Production services
- **docker-compose.dev.yml**: Development services
- **.dockerignore**: Files to exclude from Docker builds
- **.env.example**: Environment variable template

### Building the Production Image

```bash
# Build the production image
docker build -t aiwebengine:latest .

# Or use docker-compose
docker-compose build
```

### Running Production Services

```bash
# Start all services
docker-compose up -d

# Start only aiwebengine (without optional services)
docker-compose up -d aiwebengine

# Stop all services
docker-compose down

# Stop and remove volumes
docker-compose down -v
```

### Production Configuration

1. **Create a production config file:**

   ```bash
   cp config.example.yaml config.prod.toml
   # Edit config.prod.toml for production settings
   ```

2. **Set environment variables in `.env`:**

   ```bash
   # Required for OAuth
   GOOGLE_CLIENT_ID=your-actual-client-id
   GOOGLE_CLIENT_SECRET=your-actual-secret

   # Generate strong secrets
   JWT_SECRET=$(openssl rand -base64 32)
   SESSION_SECRET=$(openssl rand -base64 32)
   ```

3. **Update docker-compose.yml** to use your config:
   ```yaml
   volumes:
     - ./config.prod.toml:/app/config.yaml:ro
   ```

## Development Deployment

### Development with Hot-Reload

```bash
# Start development environment
docker-compose -f docker-compose.dev.yml up

# The server will automatically reload when you change source files
```

### Development Features

- **Hot-reload**: Code changes trigger automatic rebuild
- **Debug logging**: Full debug output enabled
- **Source mounting**: No need to rebuild for code changes
- **Cargo caching**: Faster rebuilds with cached dependencies
- **Interactive terminal**: Attach to container for debugging

### Accessing Development Services

- **aiwebengine**: http://localhost:3000
- **PostgreSQL**: localhost:5432 (username: aiwebengine, password: devpassword)
- **Redis**: localhost:6379

## Configuration

### Environment Variables

All configuration can be overridden via environment variables:

```bash
# Server configuration
SERVER_HOST=0.0.0.0
SERVER_PORT=3000
REQUEST_TIMEOUT_MS=5000

# Logging
RUST_LOG=info
LOGGING_LEVEL=info

# Database (if using PostgreSQL)
DATABASE_URL=postgresql://user:pass@postgres:5432/aiwebengine

# OAuth2
GOOGLE_CLIENT_ID=your-client-id
GOOGLE_CLIENT_SECRET=your-client-secret

# Security
JWT_SECRET=your-jwt-secret
SESSION_SECRET=your-session-secret
```

### Configuration Precedence

1. Environment variables (highest)
2. Config file specified by `CONFIG_FILE`
3. Default values (lowest)

### Volume Mounts

Production mounts:

```yaml
volumes:
  - ./logs:/app/logs # Log persistence
  - ./scripts:/app/scripts:ro # Script directory (read-only)
  - ./data:/app/data # Database and data files
  - ./config.prod.toml:/app/config.yaml:ro # Configuration
```

## Services

### Core Services

#### aiwebengine (Required)

The main application server.

```bash
# View logs
docker-compose logs -f aiwebengine

# Restart
docker-compose restart aiwebengine

# Access shell
docker-compose exec aiwebengine /bin/bash
```

### Optional Services

#### PostgreSQL

For production database instead of SQLite:

```yaml
# Uncomment in docker-compose.yml
postgres:
  # ... configuration
```

Update your config to use PostgreSQL:

```toml
[repository]
database_type = "postgresql"
database_url = "postgresql://aiwebengine:password@postgres:5432/aiwebengine"
```

#### Redis

For caching and session storage (future enhancement):

```bash
# Start Redis
docker-compose up -d redis

# Access Redis CLI
docker-compose exec redis redis-cli
```

#### Prometheus & Grafana

For monitoring and observability:

```bash
# Start monitoring stack
docker-compose up -d prometheus grafana

# Access Grafana: http://localhost:3001
# Default credentials: admin/admin (change in .env)
```

## Monitoring

### Health Checks

All services include health checks:

```bash
# Check service health
docker-compose ps

# Manual health check
curl http://localhost:3000/health
```

### Logs

```bash
# View all logs
docker-compose logs -f

# View specific service logs
docker-compose logs -f aiwebengine

# Last 100 lines
docker-compose logs --tail=100 aiwebengine
```

### Metrics (Future)

When Prometheus is enabled:

- Prometheus: http://localhost:9090
- Grafana: http://localhost:3001

## Troubleshooting

### Container Won't Start

```bash
# Check logs
docker-compose logs aiwebengine

# Check container status
docker-compose ps

# Rebuild from scratch
docker-compose down -v
docker-compose build --no-cache
docker-compose up
```

### Permission Issues

```bash
# Fix ownership of volumes
sudo chown -R 1000:1000 logs/ data/ scripts/

# Or run with correct user
docker-compose exec -u aiwebengine aiwebengine /bin/bash
```

### Database Connection Issues

```bash
# Check PostgreSQL is running
docker-compose ps postgres

# Check PostgreSQL logs
docker-compose logs postgres

# Test connection
docker-compose exec postgres psql -U aiwebengine -d aiwebengine
```

### Hot-Reload Not Working (Development)

```bash
# Ensure source is mounted correctly
docker-compose -f docker-compose.dev.yml config

# Check cargo-watch is running
docker-compose -f docker-compose.dev.yml logs aiwebengine-dev

# Rebuild development image
docker-compose -f docker-compose.dev.yml build --no-cache
```

### Out of Memory

```bash
# Increase Docker memory limit in Docker Desktop settings
# Or add memory limits to docker-compose.yml:

services:
  aiwebengine:
    deploy:
      resources:
        limits:
          memory: 512M
        reservations:
          memory: 256M
```

### Network Issues

```bash
# Check network configuration
docker network ls
docker network inspect aiwebengine_aiwebengine-network

# Recreate network
docker-compose down
docker-compose up
```

## Production Checklist

Before deploying to production:

- [ ] Set strong `JWT_SECRET` and `SESSION_SECRET`
- [ ] Configure OAuth credentials for your domain
- [ ] Update CORS origins in config
- [ ] Enable HTTPS (or use reverse proxy)
- [ ] Set `RUST_LOG=info` or `warn`
- [ ] Configure log rotation and retention
- [ ] Set up database backups
- [ ] Configure monitoring and alerts
- [ ] Review security headers in config
- [ ] Test health check endpoint
- [ ] Set up SSL certificates
- [ ] Configure firewall rules
- [ ] Enable rate limiting
- [ ] Review and set resource limits

## Advanced Usage

### Using a Reverse Proxy

Example nginx configuration:

```nginx
server {
    listen 80;
    server_name your-domain.com;

    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### Multi-Environment Deployment

```bash
# Production
docker-compose up -d

# Staging
docker-compose -f docker-compose.yml -f docker-compose.staging.yml up -d

# Development
docker-compose -f docker-compose.dev.yml up
```

### Scaling

```bash
# Run multiple instances (requires load balancer)
docker-compose up -d --scale aiwebengine=3
```

### Backup and Restore

```bash
# Backup data volume
docker run --rm -v aiwebengine_postgres-data:/data -v $(pwd):/backup \
  alpine tar czf /backup/postgres-backup.tar.gz /data

# Restore data volume
docker run --rm -v aiwebengine_postgres-data:/data -v $(pwd):/backup \
  alpine tar xzf /backup/postgres-backup.tar.gz -C /
```

## References

- **REQ-DEPLOY-002**: Container Support
- **REQ-DEV-007**: Development Tooling
- **REQ-DEV-008**: Development Environment
- **REQ-CFG-001**: Configuration Sources
- **REQ-LOG-002**: Log Rotation

## Support

For issues and questions:

- Check logs: `docker-compose logs -f`
- Review configuration files
- Check GitHub issues
- Refer to main documentation in `docs/`
