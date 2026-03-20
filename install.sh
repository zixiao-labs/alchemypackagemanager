#!/usr/bin/env bash
set -euo pipefail

REPO="zixiao-labs/alchemypackagemanager"
INSTALL_DIR="${ALCHEMY_INSTALL_DIR:-$HOME/.alchemy/bin}"

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="macos" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH_NAME="amd64" ;;
    arm64|aarch64) ARCH_NAME="arm64" ;;
    *)             echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

ARTIFACT="alchemy-${PLATFORM}-${ARCH_NAME}"

echo "Installing Alchemy package manager..."
echo "  Platform: ${PLATFORM}-${ARCH_NAME}"
echo "  Install dir: ${INSTALL_DIR}"
echo ""

# Try downloading from latest release; fall back to building from source
RELEASE_URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT}"

if curl -fsSL --head "$RELEASE_URL" >/dev/null 2>&1; then
    echo "Downloading pre-built binary..."
    mkdir -p "$INSTALL_DIR"
    curl -fsSL "$RELEASE_URL" -o "${INSTALL_DIR}/alchemy"
    chmod +x "${INSTALL_DIR}/alchemy"
else
    echo "No pre-built binary found. Building from source..."
    if ! command -v cargo &>/dev/null; then
        echo "Error: Rust toolchain not found. Install from https://rustup.rs"
        exit 1
    fi
    cargo install --git "https://github.com/${REPO}.git" alchemy_cli
    echo ""
    echo "Installed via cargo install. Binary is in ~/.cargo/bin/alchemy"
    exit 0
fi

# Add to PATH hint
if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
    echo ""
    echo "Add Alchemy to your PATH by adding this to your shell profile:"
    echo ""
    SHELL_NAME="$(basename "$SHELL")"
    case "$SHELL_NAME" in
        zsh)  echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc" ;;
        bash) echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc" ;;
        fish) echo "  set -Ux fish_user_paths ${INSTALL_DIR} \$fish_user_paths" ;;
        *)    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" ;;
    esac
fi

echo ""
echo "Alchemy installed successfully!"
echo "Run 'alchemy --help' to get started."
