#!/usr/bin/env bash
# build-rpm.sh — build a distributable RPM of the Tauri dashboard
#
# Usage:
#   ./build-rpm.sh                 # release build + RPM bundle
#   ./build-rpm.sh -- --frozen     # extra args are passed to `cargo tauri build`
#
# How it works:
#   Tauri v2 ships a native RPM bundler (`cargo tauri build --bundles rpm`).
#   It links the release binary against the system webkit2gtk, generates the
#   .desktop launcher + hicolor icons, and records the runtime requirements
#   (webkit2gtk4.1, gtk3, …) — no .spec file or rpmbuild tree needed.
#
# Output:
#   src-tauri/target/release/bundle/rpm/system-dashboard_<ver>_<arch>.rpm
#   Install with:  sudo dnf install ./<rpm>

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAURI_DIR="$SCRIPT_DIR/src-tauri"
BUNDLE_DIR="$TAURI_DIR/target/release/bundle/rpm"

export PATH="$HOME/.cargo/bin:$PATH"

# ── Prerequisite checks ────────────────────────────────────────────────────────
fail() { echo "✘  $*" >&2; exit 1; }

command -v cargo      &>/dev/null || fail "cargo not found — install Rust: https://rustup.rs"
command -v cargo-tauri &>/dev/null || fail "cargo-tauri not found — install with: cargo install tauri-cli --locked"

if ! pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
  fail "webkit2gtk-4.1 development files missing — install with: sudo dnf install webkit2gtk4.1-devel librsvg2-devel"
fi

if command -v sccache &>/dev/null; then
  export RUSTC_WRAPPER=sccache
  echo "ℹ  sccache enabled ($(sccache --version 2>&1 | head -1))"
else
  echo "⚠  sccache not found — install with: cargo install sccache --locked"
fi

# ── Build ──────────────────────────────────────────────────────────────────────
echo "▶  cargo tauri build --bundles rpm  (release, webkit2gtk stack — expect ~10 min cold) …"
cd "$TAURI_DIR"
cargo tauri build --bundles rpm "$@"

# ── Report the artifact ────────────────────────────────────────────────────────
# Pick the most recently built RPM (the dir may contain older versions).
RPM_FILE="$(ls -t "$BUNDLE_DIR"/*.rpm 2>/dev/null | head -1 || true)"
[ -n "$RPM_FILE" ] || fail "no RPM found under $BUNDLE_DIR — inspect the build log above"

echo
echo "✔  RPM built successfully:"
echo "   $(realpath "$RPM_FILE")"
echo "   size: $(du -h "$RPM_FILE" | cut -f1)"

if command -v rpm &>/dev/null; then
  echo
  echo "── Package info ──────────────────────────────────────────────"
  rpm -qip "$RPM_FILE" | sed 's/^/   /'
  echo
  echo "── Contents ──────────────────────────────────────────────────"
  rpm -qlp "$RPM_FILE" | sed 's/^/   /'
fi

echo
echo "   checksum:"
sha256sum "$RPM_FILE" | sed 's/^/   /'
echo
echo "   install with:   sudo dnf install $(realpath "$RPM_FILE")"
echo "   uninstall with: sudo dnf remove system-dashboard"
