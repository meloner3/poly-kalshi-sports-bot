#!/bin/bash

set -e  # Exit immediately on error

echo "🚀 Building Polytaoli Linux x86_64 release..."

# Color definitions
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 1. Build the frontend
echo -e "${BLUE}📦 Step 1/4: Building frontend...${NC}"
cd web
npm run build
cd ..
# Copy frontend files to rust-backend/static for embedding in the binary.
rm -rf rust-backend/static
cp -r web/dist rust-backend/static
echo -e "${GREEN}✅ Frontend build complete${NC}"

# 2. Check for and install cross-compilation tools
echo -e "${BLUE}🔧 Step 2/4: Checking cross-compilation tools...${NC}"
if ! command -v cross &> /dev/null; then
    echo -e "${YELLOW}Installing cross...${NC}"
    cargo install cross --git https://github.com/cross-rs/cross
fi

if ! rustup target list | grep -q "x86_64-unknown-linux-gnu (installed)"; then
    echo -e "${YELLOW}Adding Linux x86_64 target...${NC}"
    rustup target add x86_64-unknown-linux-gnu
fi
echo -e "${GREEN}✅ Tool check complete${NC}"

# 3. Cross-compile the Rust backend
echo -e "${BLUE}⚙️  Step 3/4: Cross-compiling Rust application...${NC}"
cd rust-backend
# Use vendored OpenSSL to avoid depending on system OpenSSL.
OPENSSL_STATIC=1 OPENSSL_VENDORED=1 cross build --release --target x86_64-unknown-linux-gnu
cd ..
echo -e "${GREEN}✅ Compilation complete${NC}"

# 4. Package deployment files
echo -e "${BLUE}📦 Step 4/4: Packaging deployment files...${NC}"
rm -rf deploy
mkdir -p deploy

# Copy the binary.
cp rust-backend/target/x86_64-unknown-linux-gnu/release/polytaoli deploy/

# Copy configuration files.
cp rust-backend/config.example.toml deploy/
if [ -f rust-backend/config.toml ]; then
    cp rust-backend/config.toml deploy/config.toml.sample
fi

# There is no separate Python order service anymore: the Rust backend calls
# the Polymarket US API directly (Ed25519-signed), the same way it already
# calls Kalshi (RSA-PSS-signed). Nothing to package for it.

# Create the startup script.
cat > deploy/start.sh << 'EOF'
#!/bin/bash

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
EOF

chmod +x deploy/start.sh

# Create the README.
cat > deploy/README.txt << 'EOF'
Polytaoli - Prediction Market Arbitrage Scanner
================================

Deployment steps:
1. Copy config.example.toml to config.toml.
2. Edit config.toml and enter your Kalshi API credentials.
3. Edit config.toml and set polymarket.key_id and polymarket.secret_key, generated at https://polymarket.us/developer.
4. Run: ./start.sh

Configuration:
- Rust backend port: 8000
- Logs: stored in the logs/ directory
- Frontend: visit http://your-server:8000

Service architecture:
- Rust backend: handles arbitrage scanning, WebSockets, the API, and both
  exchanges' order placement (Kalshi RSA-PSS-signed, Polymarket US
  Ed25519-signed) - there is no separate Python order service.

Stop the application: Ctrl+C or kill the process.
EOF

# Package the deployment files.
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
PACKAGE_NAME="polytaoli-linux-x86_64-${TIMESTAMP}.tar.gz"
tar -czf "$PACKAGE_NAME" deploy/

echo -e "${GREEN}✅ Packaging complete!${NC}"
echo ""
echo "📦 Deployment package: $PACKAGE_NAME"
echo "📁 Size: $(du -h "$PACKAGE_NAME" | cut -f1)"
echo ""
echo "Deploy to a Linux server:"
echo "  1. Upload: scp $PACKAGE_NAME user@server:/path/"
echo "  2. Extract: tar -xzf $PACKAGE_NAME"
echo "  3. Configure: cd deploy && cp config.example.toml config.toml && nano config.toml"
echo "  4. Start: ./start.sh"
echo ""
echo -e "${GREEN}🎉 Complete!${NC}"
