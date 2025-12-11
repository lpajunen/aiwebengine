# Engine Administrator Documentation

Welcome! This documentation helps you deploy, configure, and maintain aiwebengine instances.

## Who Is This For?

You're an **Engine Administrator** if you:

- Deploy and manage aiwebengine servers
- Configure infrastructure and security
- Monitor system health and performance
- Handle backups, updates, and maintenance

> **Note:** If you're building applications _using_ aiwebengine, see [Solution Developer Documentation](../solution-developers/).

## Quick Links

| I want to...                       | Go to                                                                |
| ---------------------------------- | -------------------------------------------------------------------- |
| 🚀 Get started quickly             | [01-GETTING-STARTED.md](01-GETTING-STARTED.md)                       |
| 📋 See common commands             | [QUICK-REFERENCE.md](QUICK-REFERENCE.md)                             |
| ⚙️ Understand configuration        | [02-CONFIGURATION.md](02-CONFIGURATION.md)                           |
| 🏃 Run in local/staging/production | [03-RUNNING-ENVIRONMENTS.md](03-RUNNING-ENVIRONMENTS.md)             |
| 🔐 Set up OAuth and secrets        | [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md)             |
| 📊 Monitor and maintain            | [05-MONITORING-AND-MAINTENANCE.md](05-MONITORING-AND-MAINTENANCE.md) |
| 🔧 Fix problems                    | [06-TROUBLESHOOTING.md](06-TROUBLESHOOTING.md)                       |

## Documentation Structure

### For First-Time Setup

**Start here if this is your first time:**

1. **[Getting Started](01-GETTING-STARTED.md)** - Prerequisites and first deployment
2. **[Configuration](02-CONFIGURATION.md)** - Understanding config files and environment variables
3. **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Choose local, staging, or production

### For Day-to-Day Operations

**Reference these for ongoing administration:**

- **[Quick Reference](QUICK-REFERENCE.md)** - Fast lookup for commands and variables
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - Managing credentials and OAuth
- **[Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)** - Keeping systems healthy
- **[Troubleshooting](06-TROUBLESHOOTING.md)** - Solving common problems

## Common Tasks

### Initial Setup

```bash
# 1. Copy configuration
cp config.local.toml config.toml

# 2. Set up environment
cp .env.example .env
# Edit .env with your credentials

# 3. Run locally
source .env && cargo run

# Or use Docker
make docker-local
```

### Production Deployment

```bash
# 1. Use production config
cp config.production.toml config.toml

# 2. Set secrets via environment (NEVER in files!)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"
export APP_SECURITY__CSRF_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(openssl rand -base64 32)"

# 3. Deploy with Docker
make docker-prod
```

See [03-RUNNING-ENVIRONMENTS.md](03-RUNNING-ENVIRONMENTS.md) for complete guides.

## System Requirements

### Minimum Requirements

- **CPU**: 1 core
- **RAM**: 512 MB
- **Disk**: 1 GB
- **OS**: Linux, macOS, or Windows with WSL2

### Recommended for Production

- **CPU**: 2+ cores
- **RAM**: 2+ GB
- **Disk**: 10+ GB (for logs and data)
- **OS**: Ubuntu 22.04 LTS or similar

### Software Requirements

- **Docker**: 20.10+ (for containerized deployment)
- **Rust**: Latest stable (for building from source)
- **PostgreSQL**: 14+ (recommended for all environments)

## Configuration Environments

aiwebengine supports three pre-configured environments:

| Environment    | Config File              | Use Case               | Security |
| -------------- | ------------------------ | ---------------------- | -------- |
| **Local**      | `config.local.toml`      | Development, debugging | Relaxed  |
| **Staging**    | `config.staging.toml`    | Testing, QA            | Moderate |
| **Production** | `config.production.toml` | Live deployments       | Strict   |

Each environment has optimized settings for its use case. See [02-CONFIGURATION.md](02-CONFIGURATION.md) for details.

## Getting Help

### Documentation

- 📚 [Complete Documentation Index](../INDEX.md)
- 👥 [Solution Developer Docs](../solution-developers/)
- 🛠️ [Engine Contributor Docs](../engine-contributors/)

### Common Issues

- [Troubleshooting Guide](06-TROUBLESHOOTING.md)
- [GitHub Issues](https://github.com/lpajunen/aiwebengine/issues)

### Support Channels

- Open an issue on GitHub for bugs or questions
- Check existing documentation first
- Include error messages and logs when reporting issues

## Best Practices

✅ **DO:**

- Use environment variables for secrets in production
- Test in staging before deploying to production
- Keep logs for debugging and auditing
- Regular backups of database and configuration
- Monitor system health and performance
- Keep Docker images and dependencies updated

❌ **DON'T:**

- Commit secrets to version control
- Use development configs in production
- Skip testing before production deployment
- Ignore security warnings or updates
- Run as root user (use dedicated service user)
- Expose sensitive endpoints without authentication

## Security Considerations

🔐 **Critical Security Points:**

1. **Secrets Management**
   - NEVER commit secrets to Git
   - Use environment variables in production
   - Rotate secrets regularly
   - Use strong, randomly generated values

2. **HTTPS**
   - Always use HTTPS in production
   - Let Caddy handle certificates automatically
   - Verify certificate validity

3. **Authentication**
   - Configure OAuth providers correctly
   - Use strong JWT secrets (48+ bytes)
   - Generate strong CSRF and session encryption keys (32 bytes each)
   - Ensure all server instances use the same encryption keys
   - Set session timeouts appropriately
   - Limit bootstrap admin accounts

4. **Database**
   - Use strong database passwords
   - Limit network access to database
   - Regular backups
   - Keep PostgreSQL updated

See [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md) for comprehensive security guidance.

## Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                  Caddy (Reverse Proxy)               │
│              HTTPS, Auto-certificates                │
└─────────────────────┬───────────────────────────────┘
                      │
┌─────────────────────┴───────────────────────────────┐
│              aiwebengine (Rust/Axum)                 │
│  ┌──────────────────────────────────────────────┐   │
│  │         JavaScript Engine (QuickJS)          │   │
│  │  • Script Execution                          │   │
│  │  • HTTP Handlers                             │   │
│  │  • GraphQL                                   │   │
│  └──────────────────────────────────────────────┘   │
│                                                      │
│  • Authentication & Authorization                    │
│  • Request/Response Processing                       │
│  • Logging & Monitoring                             │
└─────────────────────┬────────────────────────────────┘
                      │
┌─────────────────────┴────────────────────────────────┐
│              PostgreSQL Database                      │
│  • User accounts                                     │
│  • Scripts                                           │
│  • Application data                                  │
└──────────────────────────────────────────────────────┘
```

## Next Steps

**New to aiwebengine?** → Start with [01-GETTING-STARTED.md](01-GETTING-STARTED.md)

**Ready to deploy?** → Choose your environment in [03-RUNNING-ENVIRONMENTS.md](03-RUNNING-ENVIRONMENTS.md)

**Need quick answers?** → Check [QUICK-REFERENCE.md](QUICK-REFERENCE.md)

---

**Last updated:** December 2025

