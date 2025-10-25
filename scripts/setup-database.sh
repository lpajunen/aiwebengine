#!/bin/bash
# Quick database setup script for first-time setup

set -e

echo "🚀 AIWebEngine Database Setup"
echo "==============================="
echo ""

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
    echo "❌ Error: Docker is not running"
    echo "Please start Docker and try again"
    exit 1
fi

# Check if SQLx CLI is installed
if ! command -v sqlx &> /dev/null; then
    echo "⚠️  SQLx CLI not found"
    echo "Installing SQLx CLI..."
    cargo install sqlx-cli --no-default-features --features postgres
else
    echo "✅ SQLx CLI is installed"
fi

# Start PostgreSQL container
echo ""
echo "📦 Starting PostgreSQL container..."
docker-compose -f docker-compose.local.yml up -d postgres-dev

# Wait for PostgreSQL to be ready
echo "⏳ Waiting for PostgreSQL to be ready..."
sleep 5

# Check if container is healthy
if docker inspect aiwebengine-postgres-dev | grep -q '"Status": "healthy"' || docker exec aiwebengine-postgres-dev pg_isready -U aiwebengine > /dev/null 2>&1; then
    echo "✅ PostgreSQL is ready"
else
    echo "⚠️  PostgreSQL is starting up..."
    sleep 5
fi

# Make db.sh executable
if [ ! -x "scripts/db.sh" ]; then
    echo "🔧 Making db.sh executable..."
    chmod +x scripts/db.sh
fi

# Set DATABASE_URL
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"
echo "✅ DATABASE_URL set"

# Run migrations
echo ""
echo "📊 Running database migrations..."
./scripts/db.sh migrate-run

# Verify setup
echo ""
echo "🔍 Verifying database setup..."
./scripts/db.sh psql -c "\dt" | grep -q "scripts" && echo "✅ Tables created successfully"

echo ""
echo "=========================================="
echo "✅ Database setup complete!"
echo ""
echo "Quick commands:"
echo "  ./scripts/db.sh psql           - Interactive SQL"
echo "  ./scripts/db.sh migrate-info   - Check migrations"
echo "  ./scripts/db.sh --help         - All commands"
echo ""
echo "Start the engine with:"
echo "  cargo run"
echo "=========================================="
