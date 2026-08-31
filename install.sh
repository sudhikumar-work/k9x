#!/usr/bin/env bash
# k9x installer script
# Supports macOS (Apple Silicon & Intel) and Linux (x86_64 & ARM64)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash
# or with specific version:
#   curl -fsSL https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash -s -- --version v0.2.0

set -euo pipefail

OWNER="sudhikumar-work"
REPO="k9x"
BINARY="k9x"
INSTALL_DIR=""
VERSION=""

# Colors
BOLD='\033[1m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

info() {
    echo -e "${BLUE}==>${NC} ${BOLD}$1${NC}"
}

success() {
    echo -e "${GREEN}==>${NC} ${BOLD}$1${NC}"
}

warn() {
    echo -e "${YELLOW}WARNING:${NC} $1"
}

error() {
    echo -e "${RED}ERROR:${NC} $1" >&2
    exit 1
}

# Parse flags
while [[ $# -gt 0 ]]; do
    case "$1" in
        -v|--version)
            VERSION="$2"
            shift 2
            ;;
        -d|--dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        -h|--help)
            echo "k9x Installer"
            echo "Options:"
            echo "  -v, --version <tag>    Install specific version (default: latest)"
            echo "  -d, --dir <path>       Target install directory (default: auto-detect /usr/local/bin or ~/.local/bin)"
            echo "  -h, --help             Display this help message"
            exit 0
            ;;
        *)
            error "Unknown argument: $1"
            ;;
    esac
done

# Detect OS & Architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
    darwin)
        TARGET_OS="darwin"
        ;;
    linux)
        TARGET_OS="linux"
        ;;
    *)
        error "Unsupported operating system: $OS. Please download pre-built binaries manually."
        ;;
esac

case "$ARCH" in
    x86_64|amd64)
        TARGET_ARCH="amd64"
        ;;
    arm64|aarch64)
        TARGET_ARCH="arm64"
        ;;
    *)
        error "Unsupported architecture: $ARCH"
        ;;
esac

# Find latest release tag if not specified
if [ -z "$VERSION" ]; then
    info "Querying latest release from GitHub (${OWNER}/${REPO})..."
    LATEST_JSON=$(curl -fsSL "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" 2>/dev/null || true)
    if [ -n "$LATEST_JSON" ]; then
        VERSION=$(echo "$LATEST_JSON" | grep '"tag_name":' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    fi
    if [ -z "$VERSION" ]; then
        # Fallback to direct redirect header lookup if GitHub API rate limit is reached
        VERSION=$(curl -sI "https://github.com/${OWNER}/${REPO}/releases/latest" | grep -i "location:" | sed -E 's/.*tag\/(.*)/\1/' | tr -d '\r\n')
    fi
fi

if [ -z "$VERSION" ]; then
    error "Could not determine latest version. Specify manually with --version <tag>."
fi

# Strip leading 'v' if present for asset naming if standard
TAG_NAME="${VERSION}"
CLEAN_VER="${VERSION#v}"

# Target asset name
# Format: k9x-<version>-<os>-<arch>.tar.gz or k9x-darwin-universal.tar.gz for macOS
if [ "$TARGET_OS" = "darwin" ]; then
    ASSET_NAME="k9x-${CLEAN_VER}-darwin-universal.tar.gz"
    FALLBACK_ASSET="k9x-${CLEAN_VER}-darwin-${TARGET_ARCH}.tar.gz"
else
    ASSET_NAME="k9x-${CLEAN_VER}-${TARGET_OS}-${TARGET_ARCH}.tar.gz"
    FALLBACK_ASSET=""
fi

DOWNLOAD_BASE="https://github.com/${OWNER}/${REPO}/releases/download/${TAG_NAME}"
DOWNLOAD_URL="${DOWNLOAD_BASE}/${ASSET_NAME}"
CHECKSUMS_URL="${DOWNLOAD_BASE}/checksums.txt"

info "Selected version: ${TAG_NAME} for ${TARGET_OS}/${TARGET_ARCH}"

TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'k9x-install')"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# Download archive
info "Downloading ${ASSET_NAME}..."
if ! curl -fsSL -o "${TMP_DIR}/${ASSET_NAME}" "$DOWNLOAD_URL"; then
    if [ -n "$FALLBACK_ASSET" ]; then
        info "Universal package not found, trying arch-specific ${FALLBACK_ASSET}..."
        ASSET_NAME="$FALLBACK_ASSET"
        DOWNLOAD_URL="${DOWNLOAD_BASE}/${ASSET_NAME}"
        if ! curl -fsSL -o "${TMP_DIR}/${ASSET_NAME}" "$DOWNLOAD_URL"; then
            error "Failed to download ${ASSET_NAME} from ${DOWNLOAD_URL}"
        fi
    else
        error "Failed to download ${ASSET_NAME} from ${DOWNLOAD_URL}"
    fi
fi

# Verify Checksum
info "Verifying SHA-256 checksum..."
if curl -fsSL -o "${TMP_DIR}/checksums.txt" "$CHECKSUMS_URL" 2>/dev/null; then
    EXPECTED_SHA=$(grep "${ASSET_NAME}" "${TMP_DIR}/checksums.txt" | awk '{print $1}')
    if [ -n "$EXPECTED_SHA" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL_SHA=$(sha256sum "${TMP_DIR}/${ASSET_NAME}" | awk '{print $1}')
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL_SHA=$(shasum -a 256 "${TMP_DIR}/${ASSET_NAME}" | awk '{print $1}')
        else
            warn "Neither sha256sum nor shasum available; skipping checksum verification."
            ACTUAL_SHA="$EXPECTED_SHA"
        fi

        if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
            error "Checksum verification failed!\nExpected: $EXPECTED_SHA\nGot:      $ACTUAL_SHA"
        fi
        info "Checksum verified OK."
    else
        warn "Asset ${ASSET_NAME} not found in checksums.txt, skipping strict check."
    fi
else
    warn "checksums.txt not available for this release, skipping integrity verification."
fi

# Extract binary
info "Extracting..."
tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "$TMP_DIR"

if [ ! -f "${TMP_DIR}/${BINARY}" ]; then
    # In case archive extracts into a subfolder
    FOUND_BIN=$(find "$TMP_DIR" -type f -name "$BINARY" | head -n 1)
    if [ -n "$FOUND_BIN" ]; then
        cp "$FOUND_BIN" "${TMP_DIR}/${BINARY}"
    else
        error "Executable '${BINARY}' not found in downloaded archive."
    fi
fi
chmod +x "${TMP_DIR}/${BINARY}"

# Remove macOS quarantine bit if applicable to bypass Gatekeeper warning
if [ "$TARGET_OS" = "darwin" ]; then
    xattr -d com.apple.quarantine "${TMP_DIR}/${BINARY}" 2>/dev/null || true
fi

# Determine install location
if [ -z "$INSTALL_DIR" ]; then
    if [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
    elif [ "$(id -u)" -eq 0 ]; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="${HOME}/.local/bin"
        mkdir -p "$INSTALL_DIR"
    fi
else
    mkdir -p "$INSTALL_DIR"
fi

DEST_PATH="${INSTALL_DIR}/${BINARY}"
info "Installing to ${DEST_PATH}..."

if [ -w "$INSTALL_DIR" ]; then
    mv "${TMP_DIR}/${BINARY}" "$DEST_PATH"
else
    info "Escalating privileges to install to ${INSTALL_DIR} (requires sudo)..."
    sudo mv "${TMP_DIR}/${BINARY}" "$DEST_PATH"
fi

success "Successfully installed k9x ${TAG_NAME} to ${DEST_PATH}!"

# Check PATH
case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        warn "${INSTALL_DIR} is not in your PATH."
        echo -e "Add it to your shell configuration (e.g. ~/.zshrc or ~/.bashrc):"
        echo -e "  ${BOLD}export PATH=\"${INSTALL_DIR}:\$PATH\"${NC}\n"
        ;;
esac

echo -e "Run ${BOLD}k9x --version${NC} to get started."
