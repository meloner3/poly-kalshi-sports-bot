#!/bin/bash

# Startup script for the Polytaoli Rust backend and frontend.
#
# The Rust backend calls the Polymarket US API directly (Ed25519-signed
# requests in rust-backend/src/clients/polymarket.rs), the same way it
# already does for Kalshi. There is no separate order-submission process
# for either exchange.

echo "🚀 Starting Polytaoli (Rust backend version)"
echo "=================================="

# Verify the script is running from the project root.
if [ ! -d "rust-backend" ] || [ ! -d "web" ]; then
    echo "❌ Error: run this script from the project root"
    exit 1
fi

# Store all process IDs.
PIDS=""

# Start the Rust backend.
echo ""
echo "📦 Starting Rust backend (port 8000)..."
cd rust-backend

# Check the configuration file.
if [ ! -f "config.toml" ]; then
    echo "⚠️  Warning: config.toml does not exist; copying the example file..."
    if [ -f "config.example.toml" ]; then
        cp config.example.toml config.toml
        echo "✅ Created config.toml; edit it, then run this again"
        kill $PIDS 2>/dev/null
        exit 1
    else
        echo "❌ Error: config.example.toml does not exist either"
        kill $PIDS 2>/dev/null
        exit 1
    fi
fi

# Compile before starting the backend. Running `cargo run --release` in the
# background makes the health check race the release build after code changes.
echo "🔨 Building Rust backend..."
if ! cargo build --release; then
    echo "❌ Error: failed to build the Rust backend"
    kill $PIDS 2>/dev/null
    exit 1
fi

# Start the already-built backend in the background.
./target/release/polytaoli &
RUST_PID=$!
PIDS="$PIDS $RUST_PID"
echo "✅ Rust backend started (PID: $RUST_PID)"

# Wait for the backend to start.
echo "⏳ Waiting for the backend to start..."
sleep 10

# Check that the backend is healthy (with retries).
HEALTH_CHECK_ATTEMPTS=5
for i in $(seq 1 $HEALTH_CHECK_ATTEMPTS); do
    if curl -s http://localhost:8000/api/health > /dev/null 2>&1; then
        break
    fi
    if [ $i -lt $HEALTH_CHECK_ATTEMPTS ]; then
        echo "⏳ Health check attempt $i failed, retrying..."
        sleep 2
    fi
done

if ! curl -s http://localhost:8000/api/health > /dev/null 2>&1; then
    echo "❌ Error: failed to start the Rust backend - health check failed after $HEALTH_CHECK_ATTEMPTS attempts"
    echo "📝 Check the logs with: tail -100 rust-backend/logs/polytaoli.log"
    kill $PIDS 2>/dev/null
    exit 1
fi

echo "✅ Rust backend health check passed"

# Start the frontend.
cd ../web
echo ""
echo "🌐 Starting frontend (port 5173)..."

# Check for node_modules.
if [ ! -d "node_modules" ]; then
    echo "📦 Installing frontend dependencies..."
    npm install
fi

# Start the frontend development server.
npm run dev &
WEB_PID=$!
PIDS="$PIDS $WEB_PID"
echo "✅ Frontend started (PID: $WEB_PID)"

echo ""
echo "=================================="
echo "✅ Startup complete!"
echo ""
echo "📊 Rust backend: http://localhost:8000"
echo "🌐 Frontend: http://localhost:5173"
echo ""
echo "Press Ctrl+C to stop all services"
echo "=================================="

# Handle Ctrl+C.
trap "echo ''; echo '🛑 Stopping services...'; kill $PIDS 2>/dev/null; exit 0" INT

# Wait for child processes.
wait
