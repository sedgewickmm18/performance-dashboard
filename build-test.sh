#!/usr/bin/env bash
# build-test.sh — fast Rust build & test for the Tauri dashboard (run outside Bob)
#
# Usage:
#   ./build-test.sh            # run all tests  (metrics crate only — no webkit2gtk)
#   ./build-test.sh check      # type-check only (metrics crate — fast, no linking)
#   ./build-test.sh build      # debug build of the full Tauri app (webkit2gtk required)
#   ./build-test.sh release    # release bundle (.deb / AppImage)
#   ./build-test.sh dev        # cargo tauri dev  (opens the desktop window)
#
# Why we split into two crates:
#   The `app` crate pulls in webkit2gtk-sys whose lib.rs is 255 KB of FFI bindings.
#   LLVM holds that entire translation unit in RAM (~55 GB observed) regardless of
#   any codegen-units setting.  The `metrics` crate has zero Tauri/GTK deps, so
#   `cargo check -p metrics` and `cargo nextest run -p metrics` finish in seconds.
#
# Speed-up stack:
#   • metrics-only check/test  — skips all webkit2gtk / WRY / GTK compilation
#   • mold linker              — faster parallel section merging vs lld/ld
#   • sccache                  — caches rustc output across builds/clean
#   • cargo-nextest            — parallel test runner (up to 3× faster than cargo test)
#   • codegen-units=4          — bounded LLVM parallelism for the full Tauri build
#   • jobs=8                   — 8 parallel rustc × ~2 GB each ≈ 16 GB peak RSS

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAURI_DIR="$SCRIPT_DIR/src-tauri"

export PATH="$HOME/.cargo/bin:$PATH"

if command -v sccache &>/dev/null; then
  export RUSTC_WRAPPER=sccache
  echo "ℹ  sccache enabled ($(sccache --version 2>&1 | head -1))"
else
  echo "⚠  sccache not found — install with: cargo install sccache --locked"
fi

CMD="${1:-test}"
cd "$TAURI_DIR"

case "$CMD" in
  check)
    echo "▶  cargo check -p metrics  (no webkit2gtk) …"
    cargo check -p metrics
    echo "✔  check passed"
    ;;

  build)
    echo "▶  cargo build  (full Tauri app — webkit2gtk will compile, expect ~10 min cold) …"
    cargo build
    echo "✔  debug build complete"
    ;;

  release)
    echo "▶  cargo tauri build …"
    cargo tauri build
    echo "✔  release bundle written to src-tauri/target/release/bundle/"
    ;;

  dev)
    echo "▶  cargo tauri dev  (Ctrl-C to quit) …"
    cargo tauri dev
    ;;

  test)
    echo "▶  running tests for metrics crate (no webkit2gtk) …"
    if command -v cargo-nextest &>/dev/null; then
      cargo nextest run -p metrics
    else
      echo "⚠  cargo-nextest not found — falling back to cargo test"
      echo "   Install with: cargo install cargo-nextest --locked"
      cargo test -p metrics
    fi
    echo "✔  all tests passed"
    ;;

  *)
    echo "Unknown command: $CMD"
    echo "Usage: $0 [check|build|release|dev|test]"
    exit 1
    ;;
esac
