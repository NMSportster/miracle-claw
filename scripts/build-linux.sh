#!/usr/bin/env bash
# ============================================================================
# scripts/build-linux.sh
# ============================================================================
#
# Build Miracle Claw Linux installers (.deb, .AppImage, .rpm) natively on
# this WSL2 host. No Docker needed — Tauri cross-compiles from Linux to
# Linux without external tooling.
#
# Output goes to dist-installers/linux/.
#
# Usage:
#   ./scripts/build-linux.sh                       # full build (cold ~5m, warm ~30s)
#   ./scripts/build-linux.sh --rust-only           # only compile the Rust binary
#   ./scripts/build-linux.sh --bundle-only         # only run Tauri bundling (skip rust)
#   ./scripts/build-linux.sh --bundles deb,appimage  # build only specific bundle types
#   ./scripts/build-linux.sh --with-sccache        # ensure sccache is installed
#
# Why native on WSL2:
#   Tauri 2's Linux backend is GTK3 + libsoup3 — both already installed on
#   this box. The .AppImage is bundled internally by Tauri using a freshly-
#   downloaded copy of linuxdeploy (no host install needed).
#
# First-time setup (one-time):
#   ./scripts/install-sccache.sh    # sccache persistent cache (~10min)
#
# Notes:
#   - .rpm requires `rpm-build` apt package; auto-installed via sudo if missing
#   - .deb requires `dpkg-deb` (always present on Debian/Ubuntu)
#   - .AppImage requires `fuse`; works without it but the AppImage won't run
#     on machines without fuse unless you ship --appimage-extract-and-run style
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

OUTPUT_DIR="$REPO_ROOT/dist-installers/linux"
SCCACHE_DIR="${SCCACHE_DIR:-$HOME/.cache/sccache}"

RUST_ONLY=false
BUNDLE_ONLY=false
WITH_SCCACHE=false
BUNDLES="deb,appimage,rpm"

for arg in "$@"; do
    case "$arg" in
        --rust-only)    RUST_ONLY=true ;;
        --bundle-only)  BUNDLE_ONLY=true ;;
        --with-sccache) WITH_SCCACHE=true ;;
        --bundles)      BUNDLES="$2"; shift 2 ;;
        --bundles=*)    BUNDLES="${1#*=}"; shift ;;
        -h|--help)
            sed -n '3,32p' "$0"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

if $RUST_ONLY && $BUNDLE_ONLY; then
    echo "Cannot pass both --rust-only and --bundle-only" >&2
    exit 1
fi

# Host deps check (idempotent, auto-install missing pieces).
need_pkg() {
    if ! dpkg -s "$1" >/dev/null 2>&1; then
        echo ">>> Installing missing host dep: $1"
        if command -v sudo >/dev/null 2>&1; then
            sudo apt-get install -y "$1"
        else
            apt-get install -y "$1"
        fi
    fi
}
# .rpm bundling needs rpm-build on Debian/Ubuntu.
case ",$BUNDLES," in
    *,rpm,*) need_pkg rpm-build ;;
esac
# Always need these for Tauri to find the webview at runtime.
for pkg in libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev; do
    need_pkg "$pkg"
done

if $WITH_SCCACHE && [[ -f "$REPO_ROOT/scripts/install-sccache.sh" ]]; then
    "$REPO_ROOT/scripts/install-sccache.sh" || true
fi

# Re-stage src-tauri/resources/ with Linux Node binary + Linux ELF layout.
echo ">>> Re-staging src-tauri/resources/ with --target linux..."
if ! bash "$REPO_ROOT/scripts/bundle-runtime.sh" --target linux --force; then
    echo "FATAL: bundle-runtime.sh --target linux failed" >&2
    exit 1
fi
echo ">>> bundle-runtime.sh complete. resources/node:"
file "$REPO_ROOT/src-tauri/resources/node" || true

# Apply MAIC patches (same as Windows build).
echo ">>> Applying openclaw-dist patches..."
if ! bash "$REPO_ROOT/scripts/patch-openclaw-dist.sh"; then
    echo "FATAL: patch-openclaw-dist.sh failed" >&2
    exit 1
fi

# Serialize builds so two parallel runs don't corrupt the target/ dir.
LOCK_FILE="$REPO_ROOT/.build-linux.lock"
exec 9>"$LOCK_FILE"
if ! flock -n 9; then
    echo "FATAL: another build-linux.sh is already running (held $LOCK_FILE)." >&2
    exit 1
fi

# Build the tools binary first (Lesson 528 — same pattern as Windows).
# Tauri's bundle.resources expects `miracle-claw-tools.exe` to exist as a
# pre-flight validation; we touch it before the build and overwrite after.
TOOLS_EXE="$REPO_ROOT/src-tauri/target/release/miracle-claw-tools"
if [[ -n "$BUNDLES" ]]; then
    touch "$REPO_ROOT/src-tauri/resources/miracle-claw-tools.exe"
fi

mkdir -p "$OUTPUT_DIR"

if $BUNDLE_ONLY; then
    if [[ ! -f "$REPO_ROOT/src-tauri/target/release/miracle-claw" ]]; then
        echo "ERROR: --bundle-only requires an existing target/release/miracle-claw binary" >&2
        exit 1
    fi
    if [[ ! -f "$TOOLS_EXE" ]]; then
        echo "ERROR: --bundle-only requires $TOOLS_EXE (run without --bundle-only first)" >&2
        exit 1
    fi
    echo ">>> Bundle-only (skipping Rust compile, ~30s)"
    ( cd "$REPO_ROOT" && npm run tauri -- build --bundles "$BUNDLES" )
else
    echo ">>> Running full native Linux build (cold ~5min, warm ~30s)"
    # Build tools binary first (no JS embed; fast). Then `npm run tauri build`
    # builds miracle-claw + bundles into the requested bundle types.
    ( cd "$REPO_ROOT/src-tauri && cargo build --release --bin miracle-claw-tools ) && \
    cp -f "$TOOLS_EXE" "$REPO_ROOT/src-tauri/resources/miracle-claw-tools.exe" && \
    ( cd "$REPO_ROOT" && npm run tauri -- build --bundles "$BUNDLES" )
fi

# Copy output bundles to dist-installers/linux/.
BUNDLE_ROOT="$REPO_ROOT/src-tauri/target/release/bundle"
declare -A BUNDLE_PATHS=(
    [deb]="deb"
    [appimage]="appimage"
    [rpm]="rpm"
)

# Figure out which bundle types we asked for.
IFS=',' read -ra REQUESTED <<< "$BUNDLES"
COPIED=()
for bt in "${REQUESTED[@]}"; do
    bt="${bt// /}"
    subdir="${BUNDLE_PATHS[$bt]:-$bt}"
    src="$BUNDLE_ROOT/$subdir"
    if [[ -d "$src" ]]; then
        # Copy anything new (force overwrite so multiple builds converge).
        cp -fv "$src"/* "$OUTPUT_DIR/" 2>/dev/null || true
        for f in "$src"/*; do
            [[ -f "$f" ]] && COPIED+=("$(basename "$f")")
        done
    else
        echo "  (no $bt bundle at $src — build may have skipped this format)"
    fi
done

if [[ ${#COPIED[@]} -gt 0 ]]; then
    echo ""
    echo "================================================================="
    echo "  Linux bundles copied to: $OUTPUT_DIR"
    echo "================================================================="
    ls -lh "$OUTPUT_DIR"/ 2>/dev/null
    echo ""
    # Read tauri version for the Desktop copy name (Lesson 229 pattern).
    TAURI_VERSION=$(grep -oE '"version": *"[^"]+"' "$REPO_ROOT/src-tauri/tauri.conf.json" | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?')

    # .deb goes to WSL Desktop so David can double-click to install on the
    # WSL side, OR scp to a Linux VM for testing. We don't try to make this
    # a portable portable installer — that's what the .AppImage is for.
    for ext in deb AppImage rpm; do
        case "$ext" in
            AppImage) stem="${TAURI_VERSION}" ;;
            *) stem="${TAURI_VERSION}" ;;
        esac
        # Find the most recently built artifact for this extension.
        latest=$(ls -t "$OUTPUT_DIR"/*."$ext" 2>/dev/null | head -1 || true)
        if [[ -n "$latest" && -f "$latest" ]]; then
            base=$(basename "$latest")
            echo ""
            echo "  Linux artifact: $base ($(du -h "$latest" | awk '{print $1}'))"
        fi
    done
    echo ""
    echo "  Smoke check: try one of these on a Linux host:"
    echo "    sudo apt install $OUTPUT_DIR/*.deb          # Debian/Ubuntu"
    echo "    sudo dnf install $OUTPUT_DIR/*.rpm          # Fedora/RHEL"
    echo "    chmod +x $OUTPUT_DIR/*.AppImage && \\"
    echo "        $OUTPUT_DIR/*.AppImage                  # portable"
else
    if $RUST_ONLY; then
        echo ""
        echo "================================================================="
        echo "  --rust-only: Linux binary built at:"
        echo "  src-tauri/target/release/miracle-claw"
        echo "  Run again without --rust-only to bundle into installers."
        echo "================================================================="
    else
        echo ""
        echo "  WARNING: no bundle artifacts produced."
        echo "  Check the build output above for errors."
        echo "================================================================="
        exit 1
    fi
fi
