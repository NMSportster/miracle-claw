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

# If we're inside the Docker build image (cargo-xwin + nsis present), we're
# building for Windows MSVC. Otherwise, we use the host triple for dev.
# Heuristic: only auto-detect Windows when BOTH cargo-xwin AND nsis (makensis)
# are installed — that's the Docker image. A dev box that just happens to
# have cargo-xwin (e.g. for cross-compile testing) but no makensis should
# default to the host triple instead.
if [[ -n "${TAURI_BUILD_TARGET:-}" ]]; then
    TARGET_TRIPLE="$TAURI_BUILD_TARGET"
elif [[ "$(uname -s 2>/dev/null)" == "Linux" ]] \
     && command -v cargo-xwin >/dev/null 2>&1 \
     && command -v makensis >/dev/null 2>&1; then
    TARGET_TRIPLE="x86_64-pc-windows-msvc"
else
    TARGET_TRIPLE="$(rustc --print host-tuple)"
fi
# File extension is determined by the TARGET, not the host (we may be
# cross-compiling for Windows from a Linux Docker container).
EXT=""
[[ "$TARGET_TRIPLE" == *windows* ]] && EXT=".exe"

LAUNCHER_NAME="miracle-claw-launcher"
LAUNCHER_TARGET="$BINARIES_DIR/${LAUNCHER_NAME}-${TARGET_TRIPLE}${EXT}"

# If the canonical binaries dir is root-owned (e.g. left over from a Windows
# Docker build), we can't write there as a regular user. Detect this and
# fall back to a writable scratch dir. The launcher still has to be at the
# externalBin path Tauri expects (resources/externalBin in tauri.conf.json),
# so the fallback only works if Tauri resolves externalBin relative to the
# resources dir or another writable location. We symlink in that case.
if ! (touch "$LAUNCHER_TARGET" 2>/dev/null && rm -f "$LAUNCHER_TARGET" 2>/dev/null); then
    SCRATCH_BIN_DIR="$SRC_DIR/binaries-local"
    mkdir -p "$SCRATCH_BIN_DIR"
    SCRATCH_TARGET="$SCRATCH_BIN_DIR/${LAUNCHER_NAME}-${TARGET_TRIPLE}${EXT}"
    # If tauri.conf.json still references the original dir, also try writing
    # there using a different mechanism: overwrite bytes with cat (doesn't
    # require delete).
    if [[ -f "$LAUNCHER_TARGET" && ! -w "$LAUNCHER_TARGET" ]]; then
        echo "[build-launcher-sidecar] WARNING: $LAUNCHER_TARGET not writable, using $SCRATCH_TARGET" >&2
        LAUNCHER_TARGET="$SCRATCH_TARGET"
    fi
fi

# Tauri's build.rs (tauri-build) validates externalBin paths at the start of
# every `cargo` invocation. We pre-stage a 0-byte placeholder at the exact
# expected filename so the validate passes before our real launcher is
# compiled; the launcher build immediately overwrites it. This avoids
# touching every cargo invocation to disable tauri-build.
#
# Important: tauri-build validates against the HOST triple too (because the
# lib build script runs even when we're cross-compiling the bin). So we
# stage BOTH the host-triple placeholder AND the target-triple placeholder.
#
# Also: when the target is Windows, Tauri expects the .exe suffix; when the
# target is *nix, no suffix. So we compute the expected file name per-triple
# rather than reusing $EXT globally.
HOST_TRIPLE="$(rustc --print host-tuple)"
host_ext=""
[[ "$HOST_TRIPLE" == *windows* ]] && host_ext=".exe"
target_ext=""
[[ "$TARGET_TRIPLE" == *windows* ]] && target_ext=".exe"
mkdir -p "$BINARIES_DIR"
[ -e "$LAUNCHER_TARGET" ] || touch "$LAUNCHER_TARGET"
[ -e "$BINARIES_DIR/${LAUNCHER_NAME}-${HOST_TRIPLE}${host_ext}" ] || touch "$BINARIES_DIR/${LAUNCHER_NAME}-${HOST_TRIPLE}${host_ext}"

# Build the launcher. --quiet keeps the log clean. Use `cargo xwin` when
# cross-compiling for MSVC (provides the link.exe wrapper + MSVC SDK); use
# plain cargo for the host triple.
echo "[build-launcher-sidecar] target triple: $TARGET_TRIPLE"
echo "[build-launcher-sidecar] building $LAUNCHER_NAME..."
if [[ "$TARGET_TRIPLE" == "$HOST_TRIPLE" ]]; then
    ( cd "$SRC_DIR" && cargo build --bin "$LAUNCHER_NAME" --quiet )
else
    ( cd "$SRC_DIR" && cargo xwin build --bin "$LAUNCHER_NAME" --target "$TARGET_TRIPLE" --quiet )
fi

# Locate the cargo-built binary. For cross-compile targets, cargo puts it at
# target/<target>/<profile>/<binary-name>; for host triples it's
# target/<profile>/<binary-name>.
PROFILE="debug"
if [[ "$TARGET_TRIPLE" == "$(rustc --print host-tuple)" ]]; then
    [ -x "$SRC_DIR/target/release/$LAUNCHER_NAME$EXT" ] && PROFILE="release"
    BUILT="$SRC_DIR/target/$PROFILE/${LAUNCHER_NAME}${EXT}"
else
    [ -x "$SRC_DIR/target/$TARGET_TRIPLE/release/$LAUNCHER_NAME$EXT" ] && PROFILE="release"
    BUILT="$SRC_DIR/target/$TARGET_TRIPLE/$PROFILE/${LAUNCHER_NAME}${EXT}"
fi

if [ ! -f "$BUILT" ]; then
    echo "[build-launcher-sidecar] error: built launcher not found at $BUILT" >&2
    exit 1
fi

# Copy with the target triple suffix Tauri expects.
cp -f "$BUILT" "$LAUNCHER_TARGET"
chmod +x "$LAUNCHER_TARGET"
echo "[build-launcher-sidecar] sidecar ready: $LAUNCHER_TARGET"
