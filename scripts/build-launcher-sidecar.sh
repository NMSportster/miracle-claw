#!/usr/bin/env bash
# ============================================================================
# scripts/build-launcher-sidecar.sh
# ============================================================================
#
# Builds the `miracle-claw-launcher` binary and copies it to the Tauri-
# expected location with the host target triple appended.
#
# Tauri 2's sidecar model requires the binary to exist on disk before the
# `cargo tauri build` command runs. Tauri's build script (which we trigger
# via `beforeBuildCommand` in tauri.conf.json) takes the file from
# `src-tauri/binaries/miracle-claw-launcher-<target-triple>[.exe]`
#
# This wrapper is called twice: once for dev (cargo tauri dev) and once for
# prod (cargo tauri build). Both rely on the same file being present.
#
# Idempotent: a rebuild only happens if the launcher source has changed since
# the last invocation.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$REPO_ROOT/src-tauri"
BINARIES_DIR="$SRC_DIR/binaries"
mkdir -p "$BINARIES_DIR"

TARGET_TRIPLE="$(rustc --print host-tuple)"
EXT=""
[[ "$(uname -s 2>/dev/null || echo Windows)" == "Windows" || "${OS:-}" == "Windows_NT" ]] && EXT=".exe"

LAUNCHER_NAME="miracle-claw-launcher"
LAUNCHER_TARGET="$BINARIES_DIR/${LAUNCHER_NAME}-${TARGET_TRIPLE}${EXT}"

# Tauri's build.rs (tauri-build) validates externalBin paths at the start of
# every `cargo` invocation. We pre-stage a 0-byte placeholder at the exact
# expected filename so the validate passes before our real launcher is
# compiled; the launcher build immediately overwrites it. This avoids
# touching every cargo invocation to disable tauri-build.
mkdir -p "$BINARIES_DIR"
[ -e "$LAUNCHER_TARGET" ] || touch "$LAUNCHER_TARGET"

# Build the launcher. --quiet keeps the log clean; --manifest-path avoids
# any future workspace confusion. We pass --target by host triple so the
# resulting binary matches Tauri's expectation on the local dev box.
echo "[build-launcher-sidecar] target triple: $TARGET_TRIPLE"
echo "[build-launcher-sidecar] building $LAUNCHER_NAME..."
( cd "$SRC_DIR" && cargo build --bin "$LAUNCHER_NAME" --quiet )

# Locate the cargo-built binary. Cargo's default profile puts it at
# target/<profile>/<binary-name>; for host triples it's the same dir.
PROFILE="debug"
[ -x "$SRC_DIR/target/release/$LAUNCHER_NAME$EXT" ] && PROFILE="release"
BUILT="$SRC_DIR/target/$PROFILE/${LAUNCHER_NAME}${EXT}"

if [ ! -f "$BUILT" ]; then
    echo "[build-launcher-sidecar] error: built launcher not found at $BUILT" >&2
    exit 1
fi

# Copy with the target triple suffix Tauri expects.
cp -f "$BUILT" "$LAUNCHER_TARGET"
chmod +x "$LAUNCHER_TARGET"
echo "[build-launcher-sidecar] sidecar ready: $LAUNCHER_TARGET"
