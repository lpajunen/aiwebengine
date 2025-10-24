# Bootstrap Admin Configuration

## Overview

The bootstrap admin feature allows you to specify email addresses that automatically receive Administrator privileges when they first sign in to the system. This solves the "chicken and egg" problem of how to create the first administrator.

## Problem Solved

Without bootstrap admins, you would face this dilemma:

1. You need an admin to access `/manager` to grant admin roles
2. But how do you make the first user an admin?
3. Without an initial admin, no one can manage user roles

**Solution**: Configure bootstrap admin emails in your configuration file. When a user with a matching email signs in for the first time, they automatically receive the Administrator role.

## Configuration

### TOML Format

Add `bootstrap_admins` to your `auth` section:

```toml
[auth]
jwt_secret = "your-secret-key-at-least-32-characters-long"
enabled = true

# Bootstrap admin emails - case insensitive
bootstrap_admins = [
    "admin@example.com",
    "founder@company.com"
]

[auth.providers.google]
client_id = "your-google-client-id"
client_secret = "your-google-client-secret"
redirect_uri = "http://localhost:8080/auth/callback/google"
```

### YAML Format

```yaml
auth:
  jwt_secret: "your-secret-key-at-least-32-characters-long"
  enabled: true

  # Bootstrap admin emails - case insensitive
  bootstrap_admins:
    - admin@example.com
    - founder@company.com

  providers:
    google:
      client_id: your-google-client-id
      client_secret: your-google-client-secret
      redirect_uri: http://localhost:8080/auth/callback/google
```

## How It Works

### 1. Server Startup

When the server starts, it reads the `bootstrap_admins` configuration and stores it globally.

```
2025-10-21T10:00:00.123Z INFO  Configuring 1 bootstrap admin(s): ["admin@example.com"]
```

### 2. First Sign-In

When a user signs in for the first time via OAuth:

1. User authenticates with OAuth provider (Google, Microsoft, Apple)
2. System calls `user_repository::upsert_user()` with their email
3. Email is compared (case-insensitive) with bootstrap admin list
4. If match found, user automatically receives `Administrator` role
5. User can now access `/manager` and grant roles to others

```
2025-10-21T10:05:00.456Z DEBUG Created new user: abc-123-def (admin@example.com)
2025-10-21T10:05:00.457Z DEBUG Granted Administrator role to bootstrap admin: abc-123-def (admin@example.com)
```

### 3. Subsequent Sign-Ins

- User already exists in database
- Roles are preserved (Administrator role remains)
- No special processing needed

## Features

### Case-Insensitive Matching

Email comparison is case-insensitive:

- `Admin@Example.COM` in config
- Matches user signing in as `admin@example.com`

### Multiple Admins

You can specify multiple bootstrap admin emails:

```toml
bootstrap_admins = [
    "ceo@company.com",
    "cto@company.com",
    "devops@company.com"
]
```

### Safe for Production

- Only affects NEW users (first sign-in)
- Existing users keep their current roles
- No database migration needed
- Can be changed in config and redeployed

## Example Workflow

### Initial Setup

**1. Configure bootstrap admin**

```toml
# config.toml
[auth]
bootstrap_admins = ["you@company.com"]
```

**2. Start server**

```bash
cargo run
```

**3. Sign in with OAuth**

- Navigate to your application
- Click "Sign in with Google" (or other provider)
- Use the email address you configured (`you@company.com`)

**4. Verify admin access**

- Navigate to `/manager`
- You should see the user management interface
- You now have Administrator privileges

### Granting Admin to Others

**5. Add more administrators**

- Access `/manager`
- Find the user you want to promote
- Click "Add Admin" button
- They now have Administrator privileges

**6. Remove bootstrap config (optional)**
Once you have active administrators in the system, you can optionally remove the `bootstrap_admins` configuration:

```toml
# config.toml
[auth]
# bootstrap_admins removed - no longer needed
```

Existing administrators keep their roles even after removing the bootstrap config.

## Security Considerations

### Best Practices

**✅ DO:**

- Use company email addresses you control
- Limit to 1-3 bootstrap admins
- Use work email domains you own
- Remove bootstrap config after setup (optional)
- Review server logs for bootstrap admin grants

**❌ DON'T:**

- Use public email domains (gmail.com, outlook.com) unless you control the specific address
- Include test/demo email addresses
- Share configuration files with bootstrap admin emails
- Use the same email across multiple environments

### Production Recommendations

1. **Development**: Use your development email

```toml
bootstrap_admins = ["dev@localhost"]
```

2. **Staging**: Use staging admin email

```toml
bootstrap_admins = ["admin@staging.company.com"]
```

3. **Production**: Use specific admin emails

```toml
bootstrap_admins = ["cto@company.com"]
```

### Configuration Management

Store bootstrap admin configuration securely:

- Use environment variables: `AIWEBENGINE_AUTH_BOOTSTRAP_ADMINS`
- Use secret management systems (Vault, AWS Secrets Manager)
- Don't commit real emails to public repositories

Example with environment variable:

```bash
export AIWEBENGINE_AUTH_BOOTSTRAP_ADMINS='["admin@company.com"]'
```

## Testing

### Unit Tests

The bootstrap admin feature includes comprehensive tests:

```bash
# Run bootstrap admin tests
cargo test user_repository::tests::test_bootstrap --lib

# Tests verify:
# - Bootstrap admins get Administrator role automatically
# - Regular users don't get admin role
# - Case-insensitive email matching works
```

### Integration Testing

**Test Scenario 1: Bootstrap Admin**

1. Configure `bootstrap_admins = ["test@example.com"]`
2. Sign in with `test@example.com`
3. Check user has Administrator role
4. Access `/manager` successfully

**Test Scenario 2: Regular User**

1. Same configuration
2. Sign in with `other@example.com`
3. Check user only has Authenticated role
4. Cannot access `/manager` (403 Forbidden)

## Troubleshooting

### Bootstrap Admin Not Getting Admin Role

**Problem**: Signed in but don't have admin access

**Check:**

1. Email matches exactly (check case)

```toml
# Config
bootstrap_admins = ["Admin@Example.com"]

# Must sign in with exact email (case-insensitive):
admin@example.com ✅
other@example.com ❌
```

2. Check server logs for confirmation:

```
grep "bootstrap admin" logs/server.log
```

3. Verify configuration loaded:

```
grep "Configuring bootstrap admin" logs/server.log
```

### Already Signed In Before Bootstrap Config

**Problem**: Signed in before adding bootstrap_admins to config

**Solution**: Use the manager UI with an existing admin, or:

1. Delete your user from the database (development only)
2. Add bootstrap config
3. Restart server
4. Sign in again

Or manually grant admin role through code/database.

### Multiple Environments

**Problem**: Same email across dev/staging/prod

**Solution**: Use environment-specific emails:

- Dev: `yourname+dev@company.com`
- Staging: `yourname+staging@company.com`
- Prod: `yourname@company.com`

Gmail and many providers support `+` addressing.

## API Reference

### `set_bootstrap_admins(admins: Vec<String>)`

Called automatically at server startup. Sets the global bootstrap admin list.

**Parameters:**

- `admins`: Vec of email addresses (case-insensitive)

### `upsert_user_with_bootstrap()`

Internal function that creates users and checks bootstrap admin list.

**Behavior:**

- If email matches bootstrap list: Grants Administrator role
- If email doesn't match: Grants only Authenticated role

## Related Documentation

- [User Repository Implementation](./USER_REPOSITORY_IMPLEMENTATION.md)
- [Manager UI Documentation](./MANAGER_UI.md)
- [Authentication Configuration](./engine-administrators/CONFIGURATION.md)

## FAQ

**Q: Can I add more bootstrap admins after deployment?**  
A: Yes, just update the configuration and restart the server. Only affects new users.

**Q: What happens if I remove a bootstrap admin from the config?**  
A: Existing users keep their roles. Only affects future signups.

**Q: Can bootstrap admins be demoted?**  
A: Yes, another admin can remove their Administrator role via `/manager`.

**Q: Is email validation performed?**  
A: No, emails are used as-is. Ensure they're valid and you control them.

**Q: Can I use wildcards?**  
A: No, you must specify exact email addresses.

**Q: How many bootstrap admins can I have?**  
A: No hard limit, but 1-3 is recommended for security.
