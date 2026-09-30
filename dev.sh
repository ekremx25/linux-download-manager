#!/usr/bin/env bash
# Quick dev launcher for Linux Download Manager
set -e

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${REPO_DIR}"

# Terminate any running app instance (exact process name only to never affect terminal)
pkill -x linux-download-manager 2>/dev/null || true

# Sync browser extension files to ~/Documents/Linux Download Manager Extension
mkdir -p "$HOME/Documents/Linux Download Manager Extension"
cp -r browser/chromium/* "$HOME/Documents/Linux Download Manager Extension/" 2>/dev/null || true

echo "==> Starting Linux Download Manager in Development Mode..."
cargo run --manifest-path src-tauri/Cargo.toml "$@"
