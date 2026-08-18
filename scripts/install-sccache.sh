#!/usr/bin/env bash
# ============================================================================
# scripts/install-sccache.sh
# ============================================================================
#
# One-time: build sccache from source and stage it for Docker COPY.
# sccache is a Rust compilation cache that gives ~5-10× faster incremental
# builds. Without it, the first Windows cross-compile takes ~25m; with it
# warm, subsequent builds take ~1m.
#
# Run once before first ./scripts/build-windows-docker.sh. After that,
# rerunning is a no-op (binary already staged).
#
# Time: ~10 minutes on first run (compile sccache from source).
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$REPO_ROOT/.docker-sccache"
STAGE_BIN="$STAGE_DIR/sccache"

if [ -x "$STAGE_BIN" ]; then
    echo ">>> sccache already staged at $STAGE_BIN"
    "$STAGE_BIN" --version
    exit 0
fi

mkdir -p "$STAGE_DIR"

if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found. Install Rust first: https://rustup.rs" >&2
    exit 1
fi

echo ">>> Installing sccache from source (this takes ~10 minutes the first time)..."
cargo install sccache --version 0.10.0 --locked

# Find the installed binary and stage it
SCCACHE_BIN=$(which sccache || echo "$HOME/.cargo/bin/sccache")
if [ ! -x "$SCCACHE_BIN" ]; then
    echo "ERROR: sccache not found after install" >&2
    exit 1
fi

cp "$SCCACHE_BIN" "$STAGE_BIN"
chmod +x "$STAGE_BIN"

echo ">>> sccache staged: $STAGE_BIN"
"$STAGE_BIN" --version
