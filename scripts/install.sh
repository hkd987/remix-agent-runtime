#!/usr/bin/env sh
set -e

# This script can be sourced for testing. Set REMIX_INSTALL_SOURCE_ONLY=1 to define the
# functions without performing an install, so scripts/test-install.sh can exercise the
# real implementations rather than its own copies of them.

# Map `uname -s` to the target triple's OS component.
detect_os_target() {
  case "$1" in
    Darwin)  echo "apple-darwin" ;;
    Linux)   echo "unknown-linux-gnu" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
    *) return 1 ;;
  esac
}

# Map `uname -m` to the target triple's architecture component.
detect_arch_target() {
  case "$1" in
    arm64|aarch64) echo "aarch64" ;;
    x86_64)        echo "x86_64" ;;
    *) return 1 ;;
  esac
}

# Remove a symlink at $1 so a real binary can take its place.
# Returns 0 if a symlink was removed, 1 if there was nothing to do.
symlink_guard() {
  if [ -L "$1" ]; then
    rm -f "$1"
    return 0
  fi
  return 1
}

# Choose an install directory: prefer $1/.local/bin, fall back to $2 (or /usr/local/bin).
resolve_install_dir() {
  home_dir="$1"
  fallback="${2:-/usr/local/bin}"
  local_bin="$home_dir/.local/bin"
  if mkdir -p "$local_bin" 2>/dev/null; then
    echo "$local_bin"
    return 0
  fi
  if [ -w "$fallback" ]; then
    echo "$fallback"
    return 0
  fi
  return 1
}

# Whether directory ($1) appears in PATH ($2) as a whole entry.
path_contains() {
  dir="$1"
  path_var="$2"
  case ":$path_var:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

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
  if symlink_guard "$INSTALL_DIR/$BINARY"; then
    echo "Replaced existing symlink at $INSTALL_DIR/$BINARY with standalone binary."
  fi

  cp "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
  chmod +x "$INSTALL_DIR/$BINARY"
  rm -rf "$TMPDIR"

  echo "${BINARY} ${TAG} installed to ${INSTALL_DIR}/${BINARY}"
  echo ""
}

main() {
  OS=$(uname -s)
  if ! OS_TARGET=$(detect_os_target "$OS"); then
    echo "Unsupported OS: $OS"
    exit 1
  fi
  if [ "$OS_TARGET" = "windows" ]; then
    echo "On Windows, download the .zip files from:"
    echo "  https://github.com/hkd987/remix-agent-runtime/releases/latest"
    echo "  https://github.com/hkd987/remix-browser/releases/latest"
    exit 1
  fi

  ARCH=$(uname -m)
  if ! ARCH_TARGET=$(detect_arch_target "$ARCH"); then
    echo "Unsupported architecture: $ARCH"
    exit 1
  fi

  TARGET="${ARCH_TARGET}-${OS_TARGET}"

  if ! INSTALL_DIR=$(resolve_install_dir "$HOME"); then
    echo "Error: Cannot write to $HOME/.local/bin or /usr/local/bin"
    exit 1
  fi


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
  if path_contains "$INSTALL_DIR" "$PATH"; then
    echo "Usage:"
    echo "  export REMIX_LLM_API_KEY=sk-ant-your-key-here"
    echo "  remix-agent run \"Navigate to example.com and tell me what's on the page\""
  else
    echo "ACTION REQUIRED: $INSTALL_DIR is not in your PATH."
    echo ""
    echo "Run this now:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "To make it permanent, add the line above to your shell profile:"
    echo "  bash -> ~/.bashrc"
    echo "  zsh  -> ~/.zshrc"
  fi
  echo ""

}

# Only install when executed directly, so tests can source this file for its functions.
if [ "${REMIX_INSTALL_SOURCE_ONLY:-0}" != "1" ]; then
  main "$@"
fi
