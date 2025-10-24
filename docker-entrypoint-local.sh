#!/bin/sh
set -e

# Fix permissions for cargo registry and target directories
# This ensures the aiwebengine user can write to Docker volumes
if [ "$(id -u)" = "0" ]; then
    # Running as root, fix permissions
    chown -R aiwebengine:aiwebengine /usr/local/cargo/registry 2>/dev/null || true
    chown -R aiwebengine:aiwebengine /app/target 2>/dev/null || true
    
    # Switch to the aiwebengine user and execute the command
    exec gosu aiwebengine "$@"
else
    # Already running as the correct user
    exec "$@"
fi
