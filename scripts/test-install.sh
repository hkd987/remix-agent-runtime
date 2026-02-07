#!/usr/bin/env bash
set -euo pipefail

PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }

# --- Functions under test (extracted from install.sh logic) ---

symlink_guard() {
  local path="$1"
  if [ -L "$path" ]; then
    rm -f "$path"
    return 0
  fi
  return 1
}

resolve_install_dir() {
  local home_dir="$1"
  local local_bin="$home_dir/.local/bin"
  if mkdir -p "$local_bin" 2>/dev/null; then
    echo "$local_bin"
    return 0
  fi
  local fallback="/usr/local/bin"
  if [ -w "$fallback" ]; then
    echo "$fallback"
    return 0
  fi
  return 1
}

path_contains() {
  local dir="$1"
  local path_var="$2"
  case ":$path_var:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# --- Tests ---

echo "Running install script tests..."
echo ""

# Test 1: symlink guard removes symlink
echo "Test 1: symlink guard removes symlink"
TMPDIR1=$(mktemp -d)
ln -s /nonexistent "$TMPDIR1/remix-agent"
if symlink_guard "$TMPDIR1/remix-agent" && [ ! -e "$TMPDIR1/remix-agent" ]; then
  pass "symlink removed"
else
  fail "symlink not removed"
fi
rm -rf "$TMPDIR1"

# Test 2: symlink guard noop on regular file
echo "Test 2: symlink guard noop on regular file"
TMPDIR2=$(mktemp -d)
echo "real binary" > "$TMPDIR2/remix-agent"
if ! symlink_guard "$TMPDIR2/remix-agent" && [ -f "$TMPDIR2/remix-agent" ]; then
  pass "regular file preserved"
else
  fail "regular file was incorrectly removed"
fi
rm -rf "$TMPDIR2"

# Test 3: symlink guard noop on missing file
echo "Test 3: symlink guard noop on missing file"
TMPDIR3=$(mktemp -d)
if ! symlink_guard "$TMPDIR3/nonexistent"; then
  pass "returns 1 for missing file"
else
  fail "should return 1 for missing file"
fi
rm -rf "$TMPDIR3"

# Test 4: cp after symlink guard creates regular file
echo "Test 4: cp after symlink guard creates regular file"
TMPDIR4=$(mktemp -d)
SRCDIR4=$(mktemp -d)
echo "binary content" > "$SRCDIR4/remix-agent"
ln -s /nonexistent "$TMPDIR4/remix-agent"
symlink_guard "$TMPDIR4/remix-agent"
cp "$SRCDIR4/remix-agent" "$TMPDIR4/remix-agent"
if [ -f "$TMPDIR4/remix-agent" ] && [ ! -L "$TMPDIR4/remix-agent" ] && [ "$(cat "$TMPDIR4/remix-agent")" = "binary content" ]; then
  pass "regular file with correct content"
else
  fail "expected regular file with 'binary content'"
fi
rm -rf "$TMPDIR4" "$SRCDIR4"

# Test 5: install dir prefers ~/.local/bin
echo "Test 5: install dir prefers ~/.local/bin"
TMPDIR5=$(mktemp -d)
result=$(resolve_install_dir "$TMPDIR5")
if [ "$result" = "$TMPDIR5/.local/bin" ] && [ -d "$TMPDIR5/.local/bin" ]; then
  pass "~/.local/bin preferred and created"
else
  fail "expected $TMPDIR5/.local/bin, got $result"
fi
rm -rf "$TMPDIR5"

# Test 6: install dir fails when unwritable
echo "Test 6: install dir fails when unwritable"
TMPDIR6=$(mktemp -d)
chmod 000 "$TMPDIR6"
if ! resolve_install_dir "$TMPDIR6" >/dev/null 2>&1; then
  pass "returns error for unwritable dir"
else
  fail "should fail when dir is unwritable"
fi
chmod 755 "$TMPDIR6"
rm -rf "$TMPDIR6"

# Test 7: PATH check detects missing dir
echo "Test 7: PATH check detects missing dir"
if ! path_contains "/nonexistent/dir" "/usr/bin:/usr/local/bin"; then
  pass "returns false for missing dir"
else
  fail "should return false for dir not in PATH"
fi

# Test 8: PATH check detects present dir
echo "Test 8: PATH check detects present dir"
if path_contains "/usr/local/bin" "/usr/bin:/usr/local/bin:/home/user/bin"; then
  pass "returns true for present dir"
else
  fail "should return true for dir in PATH"
fi

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed, $((PASS + FAIL)) total"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
