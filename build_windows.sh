#!/bin/bash

set -e  # Exit immediately on error

echo "🚀 Building Polytaoli Windows x86_64 release..."

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

if ! rustup target list | grep -q "x86_64-pc-windows-gnu (installed)"; then
    echo -e "${YELLOW}Adding Windows x86_64 target...${NC}"
    rustup target add x86_64-pc-windows-gnu
fi
echo -e "${GREEN}✅ Tool check complete${NC}"

# 3. Cross-compile the Rust backend
echo -e "${BLUE}⚙️  Step 3/4: Cross-compiling Rust application...${NC}"
cd rust-backend
# Use vendored OpenSSL to avoid depending on system OpenSSL.
OPENSSL_STATIC=1 OPENSSL_VENDORED=1 cross build --release --target x86_64-pc-windows-gnu
cd ..
echo -e "${GREEN}✅ Compilation complete${NC}"

# 4. Package deployment files
echo -e "${BLUE}📦 Step 4/4: Packaging deployment files...${NC}"
rm -rf deploy-windows
mkdir -p deploy-windows

# Copy the binary.
cp rust-backend/target/x86_64-pc-windows-gnu/release/polytaoli.exe deploy-windows/

# Copy configuration files.
cp rust-backend/config.example.toml deploy-windows/
if [ -f rust-backend/config.toml ]; then
    cp rust-backend/config.toml deploy-windows/config.toml.sample
fi

# Create the Windows batch startup script.
cat > deploy-windows/start.bat << 'EOF'
@echo off
REM Check the configuration file
if not exist config.toml (
    echo Error: config.toml was not found
    echo Copy config.example.toml to config.toml and configure it
    pause
    exit /b 1
)

REM Create the log directory
if not exist logs mkdir logs

REM Start the application
echo Starting Polytaoli...
polytaoli.exe
pause
EOF

# Create the PowerShell startup script.
cat > deploy-windows/start.ps1 << 'EOF'
# Check the configuration file.
if (-not (Test-Path "config.toml")) {
    Write-Host "Error: config.toml was not found" -ForegroundColor Red
    Write-Host "Copy config.example.toml to config.toml and configure it"
    Read-Host "Press Enter to exit"
    exit 1
}

# Create the log directory.
if (-not (Test-Path "logs")) {
    New-Item -ItemType Directory -Path "logs" | Out-Null
}

# Start the application.
Write-Host "Starting Polytaoli..." -ForegroundColor Green
.\polytaoli.exe
EOF

# Create the README.
cat > deploy-windows/README.txt << 'EOF'
Polytaoli - Prediction Market Arbitrage Scanner (Windows Version)
================================================

Deployment steps:
1. Copy config.example.toml to config.toml.
2. Edit config.toml and enter your API credentials.
3. Run a startup script:
   - Double-click start.bat (Command Prompt)
   - Or right-click start.ps1 and select Run with PowerShell

Configuration:
- Port: 8000 by default
- Logs: stored in the logs\ directory
- Frontend: visit http://localhost:8000

Stop the application: close the command-line window or press Ctrl+C.

System requirements:
- Windows 10/11 or Windows Server 2016+
- x86_64 architecture
EOF

# Create a ZIP archive.
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
PACKAGE_NAME="polytaoli-windows-x86_64-${TIMESTAMP}.zip"

# Check whether the zip command is available.
if command -v zip &> /dev/null; then
    zip -r "$PACKAGE_NAME" deploy-windows/
else
    echo -e "${YELLOW}zip command not found; using tar...${NC}"
    tar -czf "${PACKAGE_NAME%.zip}.tar.gz" deploy-windows/
    PACKAGE_NAME="${PACKAGE_NAME%.zip}.tar.gz"
fi

echo -e "${GREEN}✅ Packaging complete!${NC}"
echo ""
echo "📦 Deployment package: $PACKAGE_NAME"
echo "📁 Size: $(du -h "$PACKAGE_NAME" | cut -f1)"
echo ""
echo "Deploy to a Windows server:"
echo "  1. Extract the files"
echo "  2. Enter the deploy-windows directory"
echo "  3. Copy config.example.toml to config.toml and edit it"
echo "  4. Double-click start.bat or run start.ps1"
echo ""
echo -e "${GREEN}🎉 Complete!${NC}"
