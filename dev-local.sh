#!/bin/bash
# Run aiwebengine locally with cargo run
# Uses localhost redirect URI for Google OAuth

source .env
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=http://localhost:3000/auth/callback/google

echo "Starting aiwebengine with local configuration..."
echo "Access at: http://localhost:3000"
echo "Google OAuth redirect URI: $APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI"
echo "Microsoft OAuth redirect URI: $APP_AUTH__PROVIDERS__MICROSOFT__REDIRECT_URI"
echo "Microsoft OAuth configured: ${APP_AUTH__PROVIDERS__MICROSOFT__CLIENT_ID:+Yes}"
echo ""

cargo run
