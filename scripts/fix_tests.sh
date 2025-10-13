#!/bin/bash

# Script to help fix hanging tests
# This kills hanging processes and provides test commands

echo "🔧 Test Optimization Helper"
echo "============================"
echo ""

# Step 1: Kill any hanging aiwebengine processes
echo "1. Killing any hanging aiwebengine processes..."
pkill -9 aiwebengine 2>/dev/null || echo "   No hanging processes found"
echo ""

# Step 2: Show quick test commands
echo "2. Quick Test Commands:"
echo "   ----------------------"
echo ""
echo "   Run unit tests only (fast):"
echo "   $ cargo test --lib --bins"
echo ""
echo "   Run integration tests with standard runner:"
echo "   $ cargo test --test '*'"
echo ""
echo "   Install nextest (one-time, recommended):"
echo "   $ cargo install cargo-nextest"
echo ""
echo "   Run with nextest (parallel, with timeouts):"
echo "   $ cargo nextest run"
echo ""
echo "   Run specific test:"
echo "   $ cargo nextest run test_concurrent_session_limit"
echo ""
echo "   Run with debug logging:"
echo "   $ RUST_LOG=debug cargo nextest run"
echo ""

# Step 3: Check if nextest is installed
echo "3. Checking for cargo-nextest..."
if command -v cargo-nextest &> /dev/null; then
    echo "   ✅ cargo-nextest is installed"
    echo ""
    echo "   Running quick test with nextest..."
    cargo nextest run --lib --bins 2>&1 | head -30
else
    echo "   ❌ cargo-nextest is not installed"
    echo "   Install with: cargo install cargo-nextest"
    echo ""
    echo "   Running quick test with standard runner..."
    cargo test --lib --bins 2>&1 | head -30
fi

echo ""
echo "4. Files Created/Updated:"
echo "   ----------------------"
echo "   ✅ tests/common/mod.rs - Improved test utilities"
echo "   ✅ .cargo/config.toml - Build optimizations"
echo "   ✅ .config/nextest.toml - Test runner config"
echo "   ✅ TEST_OPTIMIZATION.md - Full guide"
echo "   ✅ tests/health_integration_optimized.rs - Example"
echo ""
echo "5. Next Steps:"
echo "   -----------"
echo "   1. Review TEST_OPTIMIZATION.md for full details"
echo "   2. Update integration tests to use new utilities"
echo "   3. Replace long sleep() calls with wait_for_server()"
echo "   4. Run: cargo nextest run"
echo ""
