#!/usr/bin/env sh
set -e

# Detect OS
OS=$(uname -s)
case "$OS" in
  Darwin)  OS_TARGET="apple-darwin" ;;
  Linux)   OS_TARGET="unknown-linux-gnu" ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "On Windows, download the .zip files from:"
    echo "  https://github.com/hkd987/remix-agent-runtime/releases/latest"
    echo "  https://github.com/hkd987/remix-browser/releases/latest"
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

# Determine install directory (prefer ~/.local/bin, aligned with remix-browser)
INSTALL_DIR="$HOME/.local/bin"
if mkdir -p "$INSTALL_DIR" 2>/dev/null; then
  : # success
else
  INSTALL_DIR="/usr/local/bin"
  if [ ! -w "$INSTALL_DIR" ]; then
    echo "Error: Cannot write to $HOME/.local/bin or /usr/local/bin"
    exit 1
  fi
fi

# install_binary <repo> <binary_name>
install_binary() {
  REPO="$1"
  BINARY="$2"
  ARCHIVE="${BINARY}-${TARGET}.tar.gz"

  echo "Fetching latest ${BINARY} release..."
  TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

  if [ -z "$TAG" ]; then
    echo "Error: could not determine latest release for ${BINARY}."
    echo "Check https://github.com/$REPO/releases"
    exit 1
  fi

  URL="https://github.com/$REPO/releases/download/$TAG/$ARCHIVE"

  echo "Downloading ${BINARY} ${TAG} for ${TARGET}..."
  TMPDIR=$(mktemp -d)
  curl -fsSL "$URL" -o "$TMPDIR/$ARCHIVE"

  echo "Extracting..."
  tar xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"

  # If a symlink exists (e.g., from plugin install), remove it before copying
  if [ -L "$INSTALL_DIR/$BINARY" ]; then
    echo "Replacing existing symlink at $INSTALL_DIR/$BINARY with standalone binary..."
    rm -f "$INSTALL_DIR/$BINARY"
  fi

  cp "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
  chmod +x "$INSTALL_DIR/$BINARY"
  rm -rf "$TMPDIR"

  echo "${BINARY} ${TAG} installed to ${INSTALL_DIR}/${BINARY}"
  echo ""
}

# --- Install remix-browser (skip if already installed) ---
if command -v remix-browser >/dev/null 2>&1; then
  echo "remix-browser already installed: $(command -v remix-browser)"
  echo ""
else
  install_binary "hkd987/remix-browser" "remix-browser"
fi

# --- Install remix-agent ---
install_binary "hkd987/remix-agent-runtime" "remix-agent"

# --- Verify installation ---
echo "Setup complete!"
echo ""

# Check each binary
for bin in remix-agent remix-browser; do
  resolved=$(command -v "$bin" 2>/dev/null || true)
  if [ -n "$resolved" ]; then
    echo "  $bin -> $resolved"
  elif [ -f "$INSTALL_DIR/$bin" ]; then
    echo "  $bin -> $INSTALL_DIR/$bin (not on PATH)"
  else
    echo "  $bin -> NOT FOUND"
  fi
done
echo ""

# Check if install dir is in PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    echo "Usage:"
    echo "  export REMIX_LLM_API_KEY=sk-ant-your-key-here"
    echo "  remix-agent run \"Navigate to example.com and tell me what's on the page\""
    ;;
  *)
    echo "ACTION REQUIRED: $INSTALL_DIR is not in your PATH."
    echo ""
    echo "Run this now:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "To make it permanent, add the line above to your shell profile:"
    echo "  bash -> ~/.bashrc"
    echo "  zsh  -> ~/.zshrc"
    ;;
esac
echo ""
