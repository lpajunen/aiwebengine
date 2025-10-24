#!/bin/bash
set -e

# Fix ownership of mounted volumes if running as root initially
# This handles the case where volumes are created with root ownership
if [ "$(id -u)" = "0" ]; then
    echo "Fixing ownership of cargo registry and target directories..."
    chown -R aiwebengine:aiwebengine /usr/local/cargo/registry || true
    chown -R aiwebengine:aiwebengine /usr/local/cargo/git || true
    chown -R aiwebengine:aiwebengine /app/target || true
    
    # Switch to aiwebengine user and execute the command
    exec su-exec aiwebengine "$@"
else
    # Already running as non-root user, just execute the command
    exec "$@"
fi
