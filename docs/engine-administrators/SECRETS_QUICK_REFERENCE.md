# Secrets Management - Quick Reference

## Overview

AIWebEngine provides secure secrets management that keeps sensitive values (API keys, passwords, tokens) in the Rust layer and never exposes them to JavaScript code.

## Quick Start

### Development

```bash
# Set secrets via environment variables
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."
export SECRET_OPENAI_API_KEY="sk-..."

# Start the server
cargo run
```

### Production

```bash
# Load from secure secret store
export SECRET_ANTHROPIC_API_KEY="$(vault kv get -field=key secret/anthropic)"

# Or from files
export SECRET_ANTHROPIC_API_KEY="$(cat /etc/secrets/anthropic-key)"

# Start the server
./aiwebengine
```

## Environment Variable Format

Any environment variable with `SECRET_` prefix becomes a secret:

| Environment Variable       | Secret Identifier   |
| -------------------------- | ------------------- |
| `SECRET_ANTHROPIC_API_KEY` | `anthropic_api_key` |
| `SECRET_OPENAI_API_KEY`    | `openai_api_key`    |
| `SECRET_MY_SERVICE_TOKEN`  | `my_service_token`  |
| `SECRET_DATABASE_PASSWORD` | `database_password` |

**Rule**: Identifier = lowercase(everything after `SECRET_`)

## Configuration File (Development Only)

**⚠️ Warning: Never commit secrets to version control!**

```yaml
# config.yaml
secrets:
  values:
    anthropic_api_key: "sk-ant-api03-..."
    openai_api_key: "${OPENAI_API_KEY}" # Reference env var
```

Or in TOML:

```toml
# config.toml
[secrets.values]
anthropic_api_key = "sk-ant-api03-..."
openai_api_key = "${OPENAI_API_KEY}"
```

## JavaScript API

### Available Functions

```javascript
// ✅ Check if a secret exists
Secrets.exists("anthropic_api_key"); // returns true/false

// ✅ List all secret identifiers
Secrets.list(); // returns ['anthropic_api_key', 'openai_api_key']

// ❌ Get secret value - NOT AVAILABLE
// Secrets.get('anthropic_api_key')  // This function does not exist!
```

### Security: Values Never Exposed to JavaScript

Secret values are **never** accessible from JavaScript. They can only be used via template injection.

### Using Secrets in HTTP Requests

```javascript
// Template syntax: {{secret:identifier}}
const response = await fetch("https://api.anthropic.com/v1/messages", {
  method: "POST",
  headers: {
    // ✅ Secret injected by Rust before request is sent
    "x-api-key": "{{secret:anthropic_api_key}}",
    "content-type": "application/json",
  },
  body: JSON.stringify({
    model: "claude-3-haiku-20240307",
    messages: [{ role: "user", content: "Hello!" }],
  }),
});
```

### Error Handling Pattern

```javascript
function apiHandler(req) {
  // Check if required secret exists
  if (!Secrets.exists("anthropic_api_key")) {
    writeLog("ERROR: Anthropic API key not configured");
    return {
      status: 503,
      body: JSON.stringify({
        error: "Service Unavailable",
        message: "Please configure SECRET_ANTHROPIC_API_KEY",
      }),
      contentType: "application/json",
    };
  }

  // Proceed with API call using secret injection
  // ...
}
```

## Common Secrets

### AI Services

```bash
# Anthropic Claude
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."

# OpenAI
export SECRET_OPENAI_API_KEY="sk-..."

# Google Gemini
export SECRET_GOOGLE_API_KEY="..."

# Cohere
export SECRET_COHERE_API_KEY="..."
```

### External APIs

```bash
# Payment
export SECRET_STRIPE_API_KEY="sk_live_..."
export SECRET_STRIPE_WEBHOOK_SECRET="whsec_..."

# Email
export SECRET_SENDGRID_API_KEY="SG..."
export SECRET_MAILGUN_API_KEY="..."

# Cloud Services
export SECRET_AWS_ACCESS_KEY_ID="AKIA..."
export SECRET_AWS_SECRET_ACCESS_KEY="..."
export SECRET_AZURE_STORAGE_KEY="..."

# Authentication
export SECRET_JWT_SECRET="your-jwt-secret"
export SECRET_SESSION_SECRET="your-session-secret"
```

### Database Credentials

```bash
export SECRET_DATABASE_PASSWORD="secure-password"
export SECRET_REDIS_PASSWORD="redis-password"
export SECRET_MONGODB_PASSWORD="mongo-password"
```

## Best Practices

### ✅ DO

1. **Use environment variables in production**

   ```bash
   export SECRET_ANTHROPIC_API_KEY="$(vault kv get -field=key secret/anthropic)"
   ```

2. **Use different keys for dev/staging/prod**

   ```bash
   # Development
   export SECRET_STRIPE_API_KEY="sk_test_..."

   # Production
   export SECRET_STRIPE_API_KEY="sk_live_..."
   ```

3. **Check secret availability before using**

   ```javascript
   if (!Secrets.exists("required_key")) {
     throw new Error("Configuration error");
   }
   ```

4. **Add .env to .gitignore**

   ```bash
   echo ".env" >> .gitignore
   ```

5. **Document required secrets in README**

   ```markdown
   ## Required Secrets

   - `SECRET_ANTHROPIC_API_KEY` - Anthropic API key for AI features
   - `SECRET_DATABASE_PASSWORD` - PostgreSQL password
   ```

### ❌ DON'T

1. **Don't commit secrets to Git**

   ```yaml
   # ❌ BAD - Never do this!
   secrets:
     values:
       api_key: "sk-ant-api03-actual-key" # Will be in Git history!
   ```

2. **Don't log secret values**

   ```javascript
   // ❌ BAD - But won't work anyway (Secrets.get doesn't exist)
   // const key = Secrets.get('api_key');
   // writeLog('API Key: ' + key);

   // ✅ GOOD - The engine redacts secrets automatically
   writeLog("API key configured: " + Secrets.exists("api_key"));
   ```

3. **Don't use config files for production secrets**

   ```toml
   # ❌ BAD - Production secrets in config file
   [secrets.values]
   anthropic_api_key = "sk-ant-..."  # Use environment variables instead!
   ```

4. **Don't share API keys between environments**
   ```bash
   # ❌ BAD - Same key everywhere
   # Use separate keys for dev, staging, and production
   ```

## Secret Rotation

When rotating secrets:

1. **Update environment variable**

   ```bash
   export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-new-key"
   ```

2. **Restart the application**

   ```bash
   # Systemd
   sudo systemctl restart aiwebengine

   # Docker
   docker-compose restart

   # Manual
   pkill aiwebengine && ./aiwebengine
   ```

3. **Verify new secret is loaded**
   - Check application logs for startup messages
   - Test functionality that uses the secret
   - Monitor for authentication errors

## Troubleshooting

### Secret Not Found

**Symptom**: JavaScript reports `Secrets.exists('key') === false`

**Solutions**:

1. Check environment variable is set:

   ```bash
   echo $SECRET_ANTHROPIC_API_KEY
   ```

2. Verify prefix is correct (`SECRET_` not `SECRETS_`)

3. Restart the server after setting variables

4. Check for typos in identifier names

### Secret Not Working

**Symptom**: API returns 401 Unauthorized

**Solutions**:

1. Verify the secret value is correct
2. Check for extra spaces or newlines:
   ```bash
   export SECRET_KEY="$(echo -n 'value')"  # -n removes newline
   ```
3. Test the secret outside the application
4. Check API provider's dashboard for key status

### Template Not Replaced

**Symptom**: `{{secret:key}}` appears in request literally

**Solutions**:

1. Verify secret exists: `Secrets.exists('key')`
2. Check template syntax (no spaces, correct identifier)
3. Update to latest version (feature may not be implemented yet)
4. Check Rust logs for template injection errors

## Security Properties

### Trust Boundary

```
┌─────────────────────────────────────┐
│         Rust Layer (Trusted)        │
│  • SecretsManager stores values     │
│  • Template injection happens here  │
│  • HTTP requests made here          │
└─────────────────────────────────────┘
              ↕ (Safe API)
┌─────────────────────────────────────┐
│      JavaScript Layer (Untrusted)   │
│  • Can check: Secrets.exists()      │
│  • Can list: Secrets.list()         │
│  • CANNOT access secret values      │
└─────────────────────────────────────┘
```

### What JavaScript Can Do

- ✅ Check if a secret exists
- ✅ List secret identifiers
- ✅ Reference secrets via template syntax
- ❌ Read secret values
- ❌ Modify secrets
- ❌ Access secret storage

### Automatic Protections

- Secret values are automatically redacted from logs
- Heuristic detection prevents accidental logging
- Template injection happens after JavaScript execution
- No reflection/introspection can access values

## Examples

### AI Assistant Example

```javascript
function aiChatHandler(req) {
  // Check for API key
  if (!Secrets.exists('anthropic_api_key')) {
    return {
      status: 503,
      body: JSON.stringify({
        error: 'AI service not configured',
        setup: 'Set SECRET_ANTHROPIC_API_KEY environment variable'
      }),
      contentType: 'application/json'
    };
  }

  const body = JSON.parse(req.body);

  // Make API call with secret injection
  const response = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      'x-api-key': '{{secret:anthropic_api_key}}',
      'anthropic-version': '2023-06-01',
      'content-type': 'application/json'
    },
    body: JSON.stringify({
      model: 'claude-3-haiku-20240307',
      max_tokens: 1024,
      messages: [{ role: 'user', content: body.prompt }]
    })
  });

  return {
    status: response.status,
    body: await response.text(),
    contentType: 'application/json'
  };
}

register('/api/ai-chat', 'aiChatHandler', 'POST');
```

### Multi-Service Example

```javascript
function init() {
  // Check what services are available
  const services = [];

  if (Secrets.exists("anthropic_api_key")) {
    services.push("claude");
    register("/api/claude", "claudeHandler", "POST");
  }

  if (Secrets.exists("openai_api_key")) {
    services.push("openai");
    register("/api/openai", "openaiHandler", "POST");
  }

  writeLog("Available AI services: " + services.join(", "));
}
```

### Debugging Helper

```javascript
function debugSecretsHandler(req) {
  const secrets = Secrets.list();

  return {
    status: 200,
    body: JSON.stringify(
      {
        configured_secrets: secrets,
        count: secrets.length,
        // Check specific secrets
        has_anthropic: Secrets.exists("anthropic_api_key"),
        has_openai: Secrets.exists("openai_api_key"),
      },
      null,
      2,
    ),
    contentType: "application/json",
  };
}

register("/debug/secrets", "debugSecretsHandler", "GET");
```

## Further Reading

- [Configuration Guide](CONFIGURATION.md) - Full configuration documentation
- [Local Development](local-development.md) - Development workflow with secrets
- [Remote Development](remote-development.md) - Using secrets with the editor
- [Security Analysis](../engine-contributors/implementing/SECRET_MANAGEMENT_SECURITY_ANALYSIS.md) - Detailed security design
