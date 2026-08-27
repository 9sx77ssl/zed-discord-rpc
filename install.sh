#!/bin/bash

set -e

echo "=== Zed Discord RPC Installer ==="
echo ""

# Check dependencies
echo "[1/6] Checking dependencies..."
if ! command -v rustc &> /dev/null; then
    echo "Error: Rust not installed. Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

if ! command -v cargo &> /dev/null; then
    echo "Error: Cargo not installed"
    exit 1
fi

# Check for wasm32-wasip2 target
echo "[2/6] Checking WASM target..."
if ! rustup target list --installed | grep -q "wasm32-wasip2"; then
    echo "Adding wasm32-wasip2 target..."
    rustup target add wasm32-wasip2
fi

# Build extension
echo "[3/6] Building extension..."
cargo build --target wasm32-wasip2 --release

# Build daemon
echo "[4/6] Building daemon..."
cd daemon
cargo build --release
cd ..

# Install daemon
echo "[5/6] Installing daemon..."
mkdir -p ~/.local/bin
cp target/wasm32-wasip2/release/zed_discord_rpc.wasm ~/.local/bin/zed-discord-rpc-extension.wasm 2>/dev/null || true
cp daemon/target/release/discord-rpc-daemon ~/.local/bin/zed-discord-rpc-daemon

# Install systemd service
echo "[6/6] Installing systemd service..."
mkdir -p ~/.config/systemd/user
cp zed-discord-rpc.service ~/.config/systemd/user/

# Reload systemd
systemctl --user daemon-reload

# Enable and start service
systemctl --user enable zed-discord-rpc.service
systemctl --user start zed-discord-rpc.service

echo ""
echo "=== Installation Complete ==="
echo ""
echo "Next steps:"
echo "1. Open Zed"
echo "2. Run: zed: extensions"
echo "3. Click 'Install Dev Extension'"
echo "4. Select this directory: $(pwd)"
echo ""
echo "The daemon will start automatically on login."
echo "To check status: systemctl --user status zed-discord-rpc"
echo "To view logs: journalctl --user -u zed-discord-rpc -f"
echo ""
echo "To use without installing extension, run manually:"
echo "  /zed-rpc-update"
