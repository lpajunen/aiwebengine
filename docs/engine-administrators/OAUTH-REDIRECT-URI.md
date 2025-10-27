# OAuth Redirect URI Configuration

This document explains how to configure OAuth redirect URIs for different development environments.

## The Problem

When running AIWebEngine in different environments, the OAuth redirect URI needs to match the URL you're accessing the application from:

- **`cargo run`**: Accessed at `http://localhost:3000` → needs redirect URI: `http://localhost:3000/auth/callback/google`
- **`make docker-local`**: Accessed at `https://local.softagen.com` → needs redirect URI: `https://local.softagen.com/auth/callback/google`

Google OAuth (and other OAuth providers) require the redirect URI to match exactly.

## The Solution

### Step 1: Configure Google Cloud Console

Add BOTH redirect URIs to your Google OAuth 2.0 Client:

1. Go to [Google Cloud Console - Credentials](https://console.cloud.google.com/apis/credentials)
2. Select your OAuth 2.0 Client ID
3. Under "Authorized redirect URIs", add:
   - `http://localhost:3000/auth/callback/google`
   - `https://local.softagen.com/auth/callback/google`
4. Click "Save"

### Step 2: Run with the Appropriate Configuration

#### For `cargo run` (localhost)

Use the new `make dev-local` command which automatically sets the correct redirect URI:

```bash
make dev-local
```

This is equivalent to:

```bash
source .env && APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=http://localhost:3000/auth/callback/google cargo run
```

#### For Docker (`make docker-local`)

No changes needed! The redirect URI is already configured in `.env`:

```bash
make docker-local
```

The `.env` file has:

```bash
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=https://local.softagen.com/auth/callback/google
```

### Step 3: Access the Application

- **Cargo run**: <http://localhost:3000>
- **Docker local**: <https://local.softagen.com>

## Configuration Files

### `.env`

The `.env` file contains the default redirect URI for Docker:

```bash
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=https://local.softagen.com/auth/callback/google
```

### `config.local.toml`

The fallback redirect URI in the config file:

```toml
[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

This is only used if the environment variable is not set.

## Environment Variable Override

Environment variables always take precedence over config file values. The `make dev-local` command uses this to override the redirect URI:

```bash
APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=http://localhost:3000/auth/callback/google cargo run
```

## Quick Reference

| Run Method              | Command             | Access URL                   | Redirect URI                                      |
| ----------------------- | ------------------- | ---------------------------- | ------------------------------------------------- |
| Cargo (localhost)       | `make dev-local`    | `http://localhost:3000`      | `http://localhost:3000/auth/callback/google`      |
| Docker (local.softagen) | `make docker-local` | `https://local.softagen.com` | `https://local.softagen.com/auth/callback/google` |

## Troubleshooting

### "redirect_uri_mismatch" Error

This means the redirect URI doesn't match what's configured in Google Cloud Console.

**Solution**:

1. Check which URL you're accessing the app from
2. Verify the corresponding redirect URI is added in Google Cloud Console
3. For `cargo run`, make sure you're using `make dev-local` (not just `cargo run`)

### OAuth Works in Docker but Not with Cargo Run

**Solution**: You're probably running `cargo run` without the environment variable override. Use:

```bash
make dev-local
```

Instead of:

```bash
source .env && cargo run  # This uses the Docker redirect URI from .env!
```

## Additional OAuth Providers

The same principle applies to other OAuth providers (Microsoft, Apple, etc.):

1. Add both redirect URIs to the provider's configuration
2. Use environment variable overrides when running with `cargo run`:

```bash
# Microsoft example
APP_AUTH__PROVIDERS__MICROSOFT__REDIRECT_URI=http://localhost:3000/auth/callback/microsoft cargo run
```
