# 04 - Secrets and Security

Complete guide for managing secrets, OAuth providers, and security configuration in aiwebengine.

## Quick Navigation

- [Secrets Management Overview](#secrets-management-overview)
- [OAuth Provider Setup](#oauth-provider-setup)
- [Bootstrap Admin Configuration](#bootstrap-admin-configuration)
- [Managing Secrets](#managing-secrets)
- [Security Best Practices](#security-best-practices)
- [Secret Rotation](#secret-rotation)

---

## Secrets Management Overview

aiwebengine provides secure secrets management that keeps sensitive values (API keys, passwords, OAuth credentials) secure and never exposes them to JavaScript code.

### Types of Secrets

| Type               | Purpose                         | Examples                        |
| ------------------ | ------------------------------- | ------------------------------- |
| **Authentication** | JWT signing, session management | JWT secret, session secret      |
| **OAuth**          | Third-party authentication      | Google Client ID/Secret         |
| **API Keys**       | External service access         | Anthropic, OpenAI, Stripe       |
| **Database**       | Database credentials            | PostgreSQL password             |
| **Security**       | API authentication              | API key for endpoint protection |

### Security Model

```
┌─────────────────────────────────────┐
│      Rust Layer (Trusted)           │
│  • Stores all secret values         │
│  • Injects into HTTP requests       │
│  • NEVER exposes to JavaScript      │
└─────────────────────────────────────┘
              ↕ (Safe API)
┌─────────────────────────────────────┐
│   JavaScript Layer (Untrusted)      │
│  • Can check: secretStorage.exists()│
│  • Can list: secretStorage.list()   │
│  • CANNOT access secret values      │
└─────────────────────────────────────┘
```

---

## OAuth Provider Setup

### Google OAuth (Recommended)

Google OAuth is the easiest to set up and most commonly used.

#### Step 1: Create OAuth Credentials

1. Go to [Google Cloud Console](https://console.cloud.google.com/apis/credentials)
2. Create a new project (or select existing)
3. Enable APIs:
   - Click "Enable APIs and Services"
   - Enable "Google+ API" or "People API"
4. Create credentials:
   - Click "Create Credentials" → "OAuth 2.0 Client ID"
   - Choose "Web application"
   - Give it a name (e.g., "aiwebengine")

#### Step 2: Configure Authorized Redirect URIs

Add these redirect URIs based on your deployment:

**Local Development:**

```
http://localhost:3000/auth/callback/google
```

**Staging:**

```
https://staging.yourdomain.com/auth/callback/google
```

**Production:**

```
https://yourdomain.com/auth/callback/google
```

> ⚠️ **Important:** The redirect URI must match EXACTLY (including http/https and port)

#### Step 3: Save Credentials

After creating, you'll get:

- **Client ID** (ends with `.apps.googleusercontent.com`)
- **Client Secret** (random string)

Save these for the next step.

#### Step 4: Configure in aiwebengine

**For local development (.env file):**

```bash
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="your-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="your-client-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="http://localhost:3000/auth/callback/google"
```

**For production (environment variables):**

```bash
# Using AWS Secrets Manager
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="$(aws secretsmanager get-secret-value --secret-id google-client-id --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="$(aws secretsmanager get-secret-value --secret-id google-client-secret --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://yourdomain.com/auth/callback/google"
```

#### Step 5: Test

````bash
# Restart aiwebengine
docker-compose restart aiwebengine

### CSRF & Session Encryption Keys (NEW)

aiwebengine requires two 32-byte cryptographic keys for secure session encryption and CSRF protection. These keys must be the same across ALL server instances (do not use per-instance random keys) — otherwise sessions and CSRF tokens will be invalid when requests are routed to a different instance.

Key names (environment variables / config):

- APP_SECURITY__CSRF_KEY — base64-encoded 32-byte key used for CSRF token generation/validation
- APP_SECURITY__SESSION_ENCRYPTION_KEY — base64-encoded 32-byte key used to encrypt session payloads

Recommended generation (on Linux/macOS):

```bash
# Generate a strong 32-byte random key and print Base64
openssl rand -base64 32

# Example output (do NOT use these literal values):
# 1Pv2VndcGhv2q5x8WZ9v+v9R6n9KJfJq9s2fB2xQ9E4=
````

How to provide keys to aiwebengine:

- Docker / docker-compose — set APP_SECURITY**CSRF_KEY and APP_SECURITY**SESSION_ENCRYPTION_KEY in the container environment (examples included in the repo's docker-compose files).
- Kubernetes — create secrets and mount them as environment variables or files; ensure all replicas use the same secret values.
- Secret manager — store both keys in your secrets manager (Vault, Secrets Manager) and load them into the environment during deployment.

Security notes:

- **Do not** store plain keys in source control. Use environment variables or a secrets manager.
- Rotate keys carefully: rotating the session encryption key will invalidate existing sessions; plan rotation window and migrate sessions if needed.
- Keep CSRF keys secret and rotate when you suspect compromise.

# Open sign-in page

open http://localhost:3000/auth/login

# Click "Sign in with Google"

# You should be redirected to Google's OAuth page

````

### Microsoft OAuth (Optional)

#### Prerequisites

- Azure AD tenant (free tier available)
- Admin access to Azure portal

#### Setup Steps

1. Go to [Azure Portal](https://portal.azure.com/)
2. Navigate to "Azure Active Directory"
3. Go to "App registrations" → "New registration"
4. Configure:
   - Name: aiwebengine
   - Supported account types: "Accounts in any organizational directory and personal Microsoft accounts"
   - Redirect URI: `https://yourdomain.com/auth/callback/microsoft`
5. Save **Application (client) ID**
6. Go to "Certificates & secrets" → "New client secret"
7. Save the **Secret value** (shown only once!)

#### Configuration

```bash
export APP_AUTH__PROVIDERS__MICROSOFT__CLIENT_ID="your-application-id"
export APP_AUTH__PROVIDERS__MICROSOFT__CLIENT_SECRET="your-client-secret"
export APP_AUTH__PROVIDERS__MICROSOFT__REDIRECT_URI="https://yourdomain.com/auth/callback/microsoft"
export APP_AUTH__PROVIDERS__MICROSOFT__TENANT_ID="common"  # or your tenant ID
````

**In config.toml**, uncomment:

```toml
[auth.providers.microsoft]
client_id = "${APP_AUTH__PROVIDERS__MICROSOFT__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__MICROSOFT__CLIENT_SECRET}"
redirect_uri = "${APP_AUTH__PROVIDERS__MICROSOFT__REDIRECT_URI}"
tenant_id = "common"
scopes = ["openid", "email", "profile"]
```

### Apple Sign In (Optional)

Apple Sign In requires more complex setup including certificates and keys.

#### Prerequisites

- Apple Developer account ($99/year)
- Verified domain

#### Setup Steps

1. Go to [Apple Developer Portal](https://developer.apple.com/account/)
2. Navigate to "Certificates, Identifiers & Profiles"
3. Create an App ID:
   - Click "Identifiers" → "+" → "App IDs"
   - Enable "Sign in with Apple"
4. Create a Service ID:
   - Click "Identifiers" → "+" → "Services IDs"
   - Configure:
     - Identifier: com.yourdomain.aiwebengine
     - Enable "Sign in with Apple"
     - Configure domains and redirect URLs
5. Create a Private Key:
   - Click "Keys" → "+"
   - Enable "Sign in with Apple"
   - Download the key file (.p8)
   - Save Key ID

#### Configuration

```bash
export APP_AUTH__PROVIDERS__APPLE__CLIENT_ID="com.yourdomain.aiwebengine"
export APP_AUTH__PROVIDERS__APPLE__TEAM_ID="YOUR_TEAM_ID"
export APP_AUTH__PROVIDERS__APPLE__KEY_ID="YOUR_KEY_ID"
export APP_AUTH__PROVIDERS__APPLE__REDIRECT_URI="https://yourdomain.com/auth/callback/apple"

# Private key (multiline, from .p8 file)
export APP_AUTH__PROVIDERS__APPLE__PRIVATE_KEY="-----BEGIN PRIVATE KEY-----
MIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQg...
-----END PRIVATE KEY-----"
```

**In config.toml**, uncomment the Apple section.

---

## Bootstrap Admin Configuration

After setting up OAuth, you'll need administrator access to manage users. The bootstrap admin feature solves the "chicken and egg" problem of creating the first administrator.

### The Problem

Without bootstrap admins, you would face this dilemma:

1. You need an admin to access `/engine/admin` to grant admin roles
2. But how do you make the first user an admin?
3. Without an initial admin, no one can manage user roles

### The Solution

Configure bootstrap admin emails in your configuration file. When a user with a matching email signs in for the first time, they automatically receive the Administrator role.

### Configuration

Add `bootstrap_admins` to your `auth` section:

**config.toml:**

```toml
[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
enabled = true

# Bootstrap admin emails - case insensitive
bootstrap_admins = [
    "admin@example.com",
    "you@company.com"
]

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

**Environment variable override:**

```bash
# JSON array format
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@example.com","you@company.com"]'
```

### How It Works

#### 1. Server Startup

When the server starts, it reads the `bootstrap_admins` configuration:

```
2025-10-24T10:00:00.123Z INFO  Configuring 1 bootstrap admin(s): ["admin@example.com"]
```

#### 2. First Sign-In

When a user signs in for the first time via OAuth:

1. User authenticates with OAuth provider (Google, Microsoft, Apple)
2. System calls `user_repository::upsert_user()` with their email
3. Email is compared (case-insensitive) with bootstrap admin list
4. If match found, user automatically receives `Administrator` role
5. User can now access `/engine/admin` and grant roles to others

```
2025-10-24T10:05:00.456Z DEBUG Created new user: abc-123-def (admin@example.com)
2025-10-24T10:05:00.457Z DEBUG Granted Administrator role to bootstrap admin: abc-123-def (admin@example.com)
```

#### 3. Subsequent Sign-Ins

- User already exists in database
- Roles are preserved (Administrator role remains)
- No special processing needed

### Features

**Case-Insensitive Matching:** Email comparison is case-insensitive

- `Admin@Example.COM` in config
- Matches user signing in as `admin@example.com`

**Multiple Admins:** You can specify multiple bootstrap admin emails

```toml
bootstrap_admins = [
    "ceo@company.com",
    "cto@company.com",
    "devops@company.com"
]
```

**Safe for Production:**

- Only affects NEW users (first sign-in)
- Existing users keep their current roles
- No database migration needed
- Can be changed in config and redeployed

### Example Workflow

#### Initial Setup

**1. Configure bootstrap admin:**

```toml
# config.toml
[auth]
bootstrap_admins = ["you@company.com"]
```

**2. Start server:**

```bash
docker-compose up -d
```

**3. Sign in with OAuth:**

- Navigate to your application
- Click "Sign in with Google" (or other provider)
- Use the email address you configured (`you@company.com`)

**4. Verify admin access:**

- Navigate to `/engine/admin`
- You should see the user management interface
- You now have Administrator privileges

#### Granting Admin to Others

**5. Add more administrators:**

- Access `/engine/admin`
- Find the user you want to promote
- Click "Add Admin" button
- They now have Administrator privileges

**6. Remove bootstrap config (optional):**

Once you have active administrators, you can optionally remove the `bootstrap_admins` configuration:

```toml
# config.toml
[auth]
# bootstrap_admins removed - no longer needed
```

Existing administrators keep their roles even after removing the bootstrap config.

### Security Considerations

**Best Practices:**

✅ **DO:**

- Use company email addresses you control
- Limit to 1-3 bootstrap admins
- Use work email domains you own
- Remove bootstrap config after setup (optional)
- Review server logs for bootstrap admin grants

❌ **DON'T:**

- Use public email domains (gmail.com, outlook.com) unless you control the specific address
- Include test/demo email addresses
- Share configuration files with bootstrap admin emails
- Use the same email across multiple environments

**Environment-Specific Configuration:**

```bash
# Development
export APP_AUTH__BOOTSTRAP_ADMINS='["dev@localhost"]'

# Staging
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@staging.company.com"]'

# Production
export APP_AUTH__BOOTSTRAP_ADMINS='["cto@company.com"]'
```

**Configuration Management:**

Store bootstrap admin configuration securely:

- Use environment variables
- Use secret management systems (Vault, AWS Secrets Manager)
- Don't commit real emails to public repositories

### Troubleshooting

**Problem:** Signed in but don't have admin access

**Check:**

1. Email matches exactly (case-insensitive)
2. Check server logs for confirmation:

   ```bash
   docker-compose logs aiwebengine | grep "bootstrap admin"
   ```

3. Verify configuration loaded:

   ```bash
   docker-compose logs aiwebengine | grep "Configuring bootstrap admin"
   ```

**Problem:** Already signed in before adding bootstrap config

**Solution:**

Delete your user from the database (development only), add bootstrap config, restart server, and sign in again. Or have an existing admin grant you the Administrator role manually.

**Problem:** Multiple environments using same email

**Solution:** Use environment-specific emails with `+` addressing:

- Dev: `yourname+dev@company.com`
- Staging: `yourname+staging@company.com`
- Prod: `yourname@company.com`

Gmail and many providers support `+` addressing.

---

## Managing Secrets

### System Secrets (Authentication & Security)

These secrets are managed via environment variables and configuration.

#### JWT Secret

Used for signing session tokens.

**Generate:**

```bash
openssl rand -base64 48
```

**Configure:**

```bash
export APP_AUTH__JWT_SECRET="generated-secret-here"
```

**Requirements:**

- Minimum 32 characters (recommend 48+ bytes base64)
- Unique per environment
- Rotate regularly (see [Secret Rotation](#secret-rotation))

#### API Key

Used to protect endpoints from unauthorized access.

**Generate:**

```bash
openssl rand -hex 32
```

**Configure:**

```bash
export APP_SECURITY__API_KEY="generated-key-here"
```

#### Database Password

**Generate:**

```bash
openssl rand -base64 24
```

**Configure:**

```bash
export POSTGRES_PASSWORD="generated-password"
export APP_REPOSITORY__DATABASE_URL="postgresql://user:password@host/db"
```

### Application Secrets (AI Services, External APIs)

These secrets are used by JavaScript scripts but never exposed to JavaScript code.

#### Environment Variable Format

Any environment variable with `SECRET_` prefix becomes available:

```bash
# Format: SECRET_{IDENTIFIER}
# The identifier is lowercase of everything after SECRET_

export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."
# → available as identifier: "anthropic_api_key"

export SECRET_OPENAI_API_KEY="sk-..."
# → available as identifier: "openai_api_key"

export SECRET_STRIPE_API_KEY="sk_live_..."
# → available as identifier: "stripe_api_key"
```

#### Using Secrets in JavaScript

JavaScript code can check if secrets exist but CANNOT access values:

```javascript
// ✅ Check if secret exists
if (secretStorage.exists("anthropic_api_key")) {
  console.log("Anthropic API key is configured");
} else {
  return {
    status: 503,
    body: "Service Unavailable: Anthropic API key not configured",
    contentType: "text/plain",
  };
}

// ✅ List all available secrets
const secrets = secretStorage.list();
console.log("Available secrets:", secrets);
// Output: ['anthropic_api_key', 'openai_api_key', 'stripe_api_key']

// ❌ Cannot get secret values
// secretStorage.get('anthropic_api_key');  // This function does NOT exist!
```

#### Template Injection in HTTP Requests

Secret values are automatically injected by Rust when making HTTP requests:

```javascript
function aiChatHandler(req) {
  // Check secret exists first
  function handleRequest(req) {
  if (!secretStorage.exists('anthropic_api_key')) {
    return {
      status: 503,
      body: 'API key not configured',
      contentType: 'text/plain'
    };
  }

  // Make API call - secret is injected via template
  const response = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      // Template syntax: {{secret:identifier}}
      // Rust replaces this with actual secret value before sending request
      'x-api-key': '{{secret:anthropic_api_key}}',
      'anthropic-version': '2023-06-01',
      'content-type': 'application/json'
    },
    body: JSON.stringify({
      model: 'claude-3-haiku-20240307',
      max_tokens: 1024,
      messages: [
        { role: 'user', content: 'Hello!' }
      ]
    })
  });

  return {
    status: response.status,
    body: await response.text(),
    contentType: 'application/json'
  };
}

routeRegistry.registerRoute('/api/chat', 'aiChatHandler', 'POST');
```

### Common Application Secrets

#### AI Services

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

#### Payment Services

```bash
# Stripe
export SECRET_STRIPE_API_KEY="sk_live_..."
export SECRET_STRIPE_WEBHOOK_SECRET="whsec_..."

# PayPal
export SECRET_PAYPAL_CLIENT_ID="..."
export SECRET_PAYPAL_CLIENT_SECRET="..."
```

#### Email Services

```bash
# SendGrid
export SECRET_SENDGRID_API_KEY="SG..."

# Mailgun
export SECRET_MAILGUN_API_KEY="..."
export SECRET_MAILGUN_DOMAIN="mg.yourdomain.com"
```

#### Cloud Services

```bash
# AWS
export SECRET_AWS_ACCESS_KEY_ID="AKIA..."
export SECRET_AWS_SECRET_ACCESS_KEY="..."

# Azure
export SECRET_AZURE_STORAGE_KEY="..."

# Google Cloud
export SECRET_GCP_SERVICE_ACCOUNT_KEY='{"type":"service_account",...}'
```

---

## Security Best Practices

### Development

✅ **DO:**

- Use `.env` files for local secrets (in `.gitignore`)
- Use test/development API keys when available
- Generate strong secrets even for development
- Test with real OAuth providers

❌ **DON'T:**

- Commit `.env` files to Git
- Use production secrets in development
- Share secrets in team chat or email
- Hardcode secrets in code

### Staging

✅ **DO:**

- Use environment variables (not files)
- Use separate secrets from production
- Use staging OAuth configurations
- Test secret rotation procedures

❌ **DON'T:**

- Reuse production secrets
- Use weak secrets "because it's just staging"
- Skip OAuth testing

### Production

✅ **DO:**

- Use secret management systems (AWS Secrets Manager, Vault, etc.)
- Generate cryptographically strong secrets
- Use different secrets per environment
- Rotate secrets regularly
- Audit secret access
- Monitor for secret leaks
- Use HTTPS everywhere
- Set `require_https = true`
- Use `secure = true` for cookies

❌ **DON'T:**

- Store secrets in config files
- Commit secrets to version control
- Share secrets between environments
- Use predictable or weak secrets
- Log secret values
- Expose secrets in error messages
- Use HTTP for OAuth redirects

### Secret Storage

**Local Development:**

```bash
# .env file (in .gitignore)
APP_AUTH__JWT_SECRET=local-dev-secret
SECRET_ANTHROPIC_API_KEY=sk-ant-...
```

**Staging/Production:**

```bash
# AWS Secrets Manager
aws secretsmanager create-secret \
  --name aiwebengine/jwt-secret \
  --secret-string "$(openssl rand -base64 48)"

# Retrieve in deployment
export APP_AUTH__JWT_SECRET="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/jwt-secret \
  --query SecretString --output text)"
```

```bash
# HashiCorp Vault
vault kv put secret/aiwebengine/jwt-secret value="..."

# Retrieve in deployment
export APP_AUTH__JWT_SECRET="$(vault kv get -field=value secret/aiwebengine/jwt-secret)"
```

```bash
# Kubernetes Secrets
kubectl create secret generic aiwebengine-secrets \
  --from-literal=jwt-secret="$(openssl rand -base64 48)"

# Mount as environment variable in pod spec
```

---

## Secret Rotation

Regular secret rotation is a security best practice.

### JWT Secret Rotation

Rotating JWT secrets will invalidate all existing sessions.

**Steps:**

1. **Generate new secret:**

   ```bash
   NEW_SECRET=$(openssl rand -base64 48)
   ```

2. **Store in secret manager:**

   ```bash
   aws secretsmanager update-secret \
     --secret-id aiwebengine/jwt-secret \
     --secret-string "$NEW_SECRET"
   ```

3. **Restart aiwebengine:**

   ```bash
   docker-compose restart aiwebengine
   ```

4. **Users will need to sign in again** (existing sessions invalid)

**Recommended Schedule:** Every 90 days, or immediately if compromised

### API Key Rotation

**Steps:**

1. **Generate new key:**

   ```bash
   NEW_KEY=$(openssl rand -hex 32)
   ```

2. **Update secret manager:**

   ```bash
   aws secretsmanager update-secret \
     --secret-id aiwebengine/api-key \
     --secret-string "$NEW_KEY"
   ```

3. **Restart aiwebengine:**

   ```bash
   docker-compose restart aiwebengine
   ```

4. **Update client applications** with new API key

**Recommended Schedule:** Every 90 days

### OAuth Credentials Rotation

**Steps:**

1. **Create new OAuth credentials** in provider (Google, Microsoft, etc.)
2. **Update both old and new** in secret manager (temporary overlap)
3. **Deploy with new credentials**
4. **Test authentication thoroughly**
5. **Remove old credentials** from secret manager
6. **Delete old credentials** in OAuth provider

**Recommended Schedule:** Annually, or if credentials compromised

### Database Password Rotation

**Steps:**

1. **Create new database user** with new password (don't drop old yet)
2. **Update connection string** with new credentials
3. **Restart aiwebengine**
4. **Verify connectivity**
5. **Drop old database user**

```bash
# In PostgreSQL
CREATE USER aiwebengine_new WITH PASSWORD 'new-password';
GRANT ALL PRIVILEGES ON DATABASE aiwebengine TO aiwebengine_new;

# Update connection string
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine_new:new-password@host/db"

# Restart and verify
docker-compose restart aiwebengine

# After verification
DROP USER aiwebengine;
ALTER USER aiwebengine_new RENAME TO aiwebengine;
```

**Recommended Schedule:** Every 180 days

### External API Keys (Anthropic, OpenAI, etc.)

**Steps:**

1. **Generate new API key** in provider dashboard
2. **Update secret:**

   ```bash
   aws secretsmanager update-secret \
     --secret-id aiwebengine/anthropic-api-key \
     --secret-string "sk-ant-api03-new-key"
   ```

3. **Restart aiwebengine:**

   ```bash
   docker-compose restart aiwebengine
   ```

4. **Verify functionality**
5. **Delete old API key** in provider dashboard

**Recommended Schedule:** Every 90-180 days, or per provider's recommendations

### Rotation Checklist

- [ ] Generate new secret with proper strength
- [ ] Store in secret manager
- [ ] Update environment variables
- [ ] Restart services
- [ ] Test functionality
- [ ] Monitor for errors
- [ ] Remove/revoke old secret
- [ ] Document rotation in change log
- [ ] Update team procedures if needed

---

## Troubleshooting

### OAuth Not Working

**Symptom:** redirect_uri_mismatch error

**Solution:**

1. Check redirect URI matches exactly:

   ```bash
   echo $APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI
   ```

2. Verify in OAuth provider console
3. Check http vs https
4. Check domain spelling
5. Restart after changes

### Secret Not Found

**Symptom:** `secretStorage.exists('key')` returns false

**Solution:**

1. Check environment variable is set:

   ```bash
   env | grep SECRET_
   ```

2. Verify prefix is `SECRET_` (not `SECRETS_`)
3. Check identifier format (lowercase, underscores)
4. Restart server after setting variables

### Template Not Replaced

**Symptom:** `{{secret:key}}` appears literally in request

**Solution:**

1. Verify secret exists: `secretStorage.exists('key')`
2. Check template syntax (no spaces)
3. Verify identifier matches exactly
4. Check server logs for injection errors

### JWT Secret Too Short

**Error:** "JWT secret must be at least 32 characters"

**Solution:**

```bash
# Generate proper secret (48 bytes base64 = 64 characters)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
```

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - Initial setup with OAuth
- **[Configuration](02-CONFIGURATION.md)** - Config file reference
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Environment-specific setup
- **[Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)** - Operational tasks
- **[Quick Reference](QUICK-REFERENCE.md)** - Command cheat sheet
