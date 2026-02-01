# Docker Secrets Configuration

This guide explains how to use constrained secrets with Docker deployments of aiwebengine.

## Overview

The aiwebengine application supports two methods for providing secrets:

1. **Environment Variables** (unconstrained - default)
   - Simple to use
   - No access control
   - Any script can send any secret to any URL
   - **Security Risk**: Malicious scripts can exfiltrate secrets

2. **TOML File** (constrained - recommended for production)
   - Access-controlled
   - Secrets restricted to specific URLs and scripts
   - Prevents secret exfiltration
   - **Recommended** for production deployments

## Using Constrained Secrets with Docker

### Step 1: Create secrets.toml File

Create a `secrets.toml` file in your project root (same directory as docker-compose.yml):

```toml
# Example: secrets.toml
[[secret]]
identifier = "ANTHROPIC_API_KEY"
value = "sk-ant-api03-..."
allowed_url_pattern = "https://api.anthropic.com/*"
allowed_script_pattern = "/scripts/ai/*"

[[secret]]
identifier = "GITHUB_TOKEN"
value = "ghp_..."
allowed_url_pattern = "https://api.github.com/*"
allowed_script_pattern = "/scripts/github/*"
```

See [secrets.toml.example](../secrets.toml.example) for more examples.

### Step 2: Mount secrets.toml in Docker

#### Production (docker-compose.yml)

Uncomment the secrets.toml volume mount for both instances:

```yaml
services:
  aiwebengine-1:
    volumes:
      # ... other volumes ...
      # Uncomment this line:
      - ./secrets.toml:/app/secrets.toml:ro
```

```yaml
aiwebengine-2:
  volumes:
    # ... other volumes ...
    # Uncomment this line:
    - ./secrets.toml:/app/secrets.toml:ro
```

#### Staging (docker-compose.staging.yml)

Same as production - uncomment the volume mount:

```yaml
services:
  aiwebengine:
    volumes:
      # ... other volumes ...
      # Uncomment this line:
      - ./secrets.toml:/app/secrets.toml:ro
```

#### Local Development (docker-compose.local.yml)

Same for both development instances:

```yaml
services:
  aiwebengine-dev-1:
    volumes:
      # ... other volumes ...
      # Uncomment this line:
      - ./secrets.toml:/app/secrets.toml:ro
```

### Step 3: Remove or Keep Environment Variables

You can either:

**Option A**: Remove environment variable secrets (recommended)

```yaml
environment:
  # Remove these lines:
  # - SECRET_ANTHROPIC_API_KEY=${SECRET_ANTHROPIC_API_KEY:-}
  # - SECRET_GITHUB_TOKEN=${SECRET_GITHUB_TOKEN:-}
```

**Option B**: Keep both (environment variables act as fallback)

- Keep environment variable definitions
- secrets.toml takes precedence for constrained secrets
- Environment variables provide unconstrained fallback

### Step 4: Deploy

```bash
# Production
docker-compose up -d

# Staging
docker-compose -f docker-compose.staging.yml up -d

# Local development
docker-compose -f docker-compose.local.yml up -d
```

## Security Considerations

### ⚠️ Environment Variables Have No Access Control

Environment variable secrets (SECRET\_\*) have **no URL or script restrictions**:

- Any script can access any secret
- Secrets can be sent to any URL
- Vulnerable to secret exfiltration

### ✅ TOML File Provides Access Control

TOML-based secrets enforce constraints:

- URLs are normalized and matched case-insensitively
- Script URIs are matched case-sensitively
- Glob patterns supported (\* and ?)
- Failed constraint checks are logged

## Deployment Checklist

- [ ] Create `secrets.toml` with constrained secrets
- [ ] Ensure `secrets.toml` is in `.gitignore` (already configured)
- [ ] Uncomment volume mount in docker-compose file
- [ ] Test locally with `docker-compose.local.yml`
- [ ] Remove environment variable secrets (optional but recommended)
- [ ] Deploy to staging first
- [ ] Monitor logs for constraint violations
- [ ] Deploy to production

## Troubleshooting

### Secrets Not Loading

Check logs for:

```
INFO Loaded X constrained secret(s) from secrets.toml
```

If not present:

1. Verify secrets.toml exists in project root
2. Check volume mount is uncommented
3. Verify file permissions (readable by container)

### Constraint Violations

Look for warnings:

```
WARN Secret ANTHROPIC_API_KEY constraint violation: URL not allowed
WARN Secret ANTHROPIC_API_KEY constraint violation: Script not allowed
```

Fix by:

1. Adjusting URL patterns in secrets.toml
2. Updating script URI patterns
3. Using glob wildcards appropriately

### Mixed Environment

If using both environment variables and secrets.toml:

- secrets.toml entries override environment variables
- Environment variables provide fallback for unconstrained access
- Check which secret is being used by examining logs

## Examples

### Restrict Anthropic API to AI scripts only

```toml
[[secret]]
identifier = "ANTHROPIC_API_KEY"
value = "sk-ant-..."
allowed_url_pattern = "https://api.anthropic.com/*"
allowed_script_pattern = "/scripts/ai/*"
```

### Allow GitHub API from multiple script directories

```toml
[[secret]]
identifier = "GITHUB_TOKEN"
value = "ghp_..."
allowed_url_pattern = "https://api.github.com/*"
allowed_script_pattern = "/scripts/{github,integration,deploy}/*"
```

### Development-only unrestricted secret

```toml
[[secret]]
identifier = "DEV_API_KEY"
value = "dev-key-123"
# No constraints = unrestricted (same as environment variable)
```

## Migration from Environment Variables

1. **Audit current secrets**: List all SECRET\_\* environment variables
2. **Determine constraints**: For each secret, identify allowed URLs and scripts
3. **Create secrets.toml**: Convert to TOML format with constraints
4. **Test constraints**: Verify scripts still work with constraints
5. **Deploy with both**: Keep env vars initially as fallback
6. **Monitor logs**: Check for constraint violations
7. **Remove env vars**: Once confident, remove environment variables

## Additional Resources

- [Secrets Access Control Documentation](SECRETS_ACCESS_CONTROL.md)
- [Environment Variable Limitations](SECRETS_ENV_LIMITATION.md)
- [secrets.toml.example](../secrets.toml.example)

## Best Practices

1. **Always use secrets.toml in production**
2. **Use specific patterns, not wildcards for everything**
3. **Test constraints in staging first**
4. **Monitor logs for violations**
5. **Never commit secrets.toml to version control**
6. **Use separate secrets.toml per environment**
7. **Rotate secrets regularly**
8. **Document allowed URL/script patterns for each secret**
