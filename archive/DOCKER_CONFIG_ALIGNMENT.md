# Docker and Config File Alignment

This document describes the aligned naming convention for Docker and configuration files.

## Overview

All Docker files and configuration files now use consistent naming based on deployment environment:

- **Local/Development**: `config.local.toml`
- **Staging**: `config.staging.toml`
- **Production**: `config.production.toml`

## File Mapping

| Environment | Config File              | Dockerfile           | Docker Compose               |
| ----------- | ------------------------ | -------------------- | ---------------------------- |
| Local/Dev   | `config.local.toml`      | `Dockerfile.local`   | `docker-compose.local.yml`   |
| Staging     | `config.staging.toml`    | `Dockerfile.staging` | `docker-compose.staging.yml` |
| Production  | `config.production.toml` | `Dockerfile`         | `docker-compose.yml`         |

## Usage

### Local Development

```bash
# Uses config.local.toml
docker-compose -f docker-compose.local.yml up
```

### Staging

```bash
# Uses config.staging.toml
docker-compose -f docker-compose.staging.yml up
```

### Production

```bash
# Uses config.production.toml
docker-compose up
```

## Key Changes

1. **Renamed references**:
   - `config.dev.toml` → `config.local.toml`
   - `config.prod.toml` → `config.production.toml`

2. **Added staging support**:
   - Created `Dockerfile.staging`
   - Created `docker-compose.staging.yml`
   - Already had `config.staging.toml`

3. **Updated documentation**:
   - DOCKER.md
   - PRODUCTION_CHECKLIST.md
   - DOCKER_QUICK_REFERENCE.md
   - JWT_SESSION_IMPLEMENTATION.md
   - URGENT_TODO.md

## Benefits

- **Consistency**: Config and Docker files use matching terminology
- **Clarity**: Environment names are explicit and unambiguous
- **Completeness**: All three environments (local, staging, production) are now fully supported
- **Convention**: Production uses base filename (`Dockerfile`, `docker-compose.yml`) per Docker best practices
