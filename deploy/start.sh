#!/bin/bash

# The Rust backend calls the Polymarket US API directly (Ed25519-signed),
# the same way it already calls Kalshi. There is no separate order-
# submission process for either exchange.

# Store all process IDs.
PIDS=""

# Handle Ctrl+C.
trap "echo ''; echo '🛑 Stopping services...'; kill $PIDS 2>/dev/null; exit 0" INT

# Ensure the binary is executable.
chmod +x polytaoli

# Check the configuration file.
if [ ! -f config.toml ]; then
    echo "❌ Error: config.toml was not found"
    echo "Copy config.example.toml to config.toml and configure it"
    exit 1
fi

# Create the log directory.
mkdir -p logs

# Start the Rust backend.
echo "🚀 Starting Rust backend (port 8000)..."
./polytaoli &
RUST_PID=$!
PIDS="$PIDS $RUST_PID"
echo "✅ Rust backend started (PID: $RUST_PID)"

echo ""
echo "=================================="
echo "✅ Startup complete!"
echo ""
echo "📊 Rust backend: http://localhost:8000"
echo ""
echo "Press Ctrl+C to stop all services"
echo "=================================="

# Wait for child processes.
wait
