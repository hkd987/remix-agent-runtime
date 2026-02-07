#!/usr/bin/env sh
set -e

REPO="hkd987/remix-agent-runtime"
BINARY="remix-agent"

# Detect OS
OS=$(uname -s)
case "$OS" in
  Darwin)  OS_TARGET="apple-darwin" ;;
  Linux)   OS_TARGET="unknown-linux-gnu" ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "On Windows, download the .zip from https://github.com/$REPO/releases/latest"
    exit 1
    ;;
  *)
    echo "Unsupported OS: $OS"
    exit 1
    ;;
esac

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  arm64|aarch64) ARCH_TARGET="aarch64" ;;
  x86_64)        ARCH_TARGET="x86_64" ;;
  *)
    echo "Unsupported architecture: $ARCH"
    exit 1
    ;;
esac

TARGET="${ARCH_TARGET}-${OS_TARGET}"
ARCHIVE="${BINARY}-${TARGET}.tar.gz"

# Get latest release tag
echo "Fetching latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$TAG" ]; then
  echo "Error: could not determine latest release. Check https://github.com/$REPO/releases"
  exit 1
fi

URL="https://github.com/$REPO/releases/download/$TAG/$ARCHIVE"

echo "Downloading $BINARY $TAG for $TARGET..."
TMPDIR=$(mktemp -d)
curl -fsSL "$URL" -o "$TMPDIR/$ARCHIVE"

echo "Extracting..."
tar xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"

# Install binary
INSTALL_DIR="/usr/local/bin"
if [ ! -w "$INSTALL_DIR" ]; then
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

cp "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
chmod +x "$INSTALL_DIR/$BINARY"
rm -rf "$TMPDIR"

echo ""
echo "$BINARY $TAG installed to $INSTALL_DIR/$BINARY"
echo ""
echo "Usage:"
echo "  export REMIX_LLM_API_KEY=sk-ant-your-key-here"
echo "  $BINARY run \"Navigate to example.com and tell me what's on the page\""
echo ""

# Check if install dir is in PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "Warning: $INSTALL_DIR is not in your PATH."
    echo "Add it with: export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
