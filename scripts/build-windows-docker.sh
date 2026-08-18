#!/usr/bin/env bash
# ============================================================================
# scripts/build-windows-docker.sh
# ============================================================================
#
# Build a Windows NSIS installer for Miracle Claw using the cross-compile
# Docker image. Output goes to dist-installers/windows/.
#
# Usage:
#   ./scripts/build-windows-docker.sh                 # full build (cold ~25m, warm ~5m)
#   ./scripts/build-windows-docker.sh --rebuild-image # rebuild image even if it exists
#   ./scripts/build-windows-docker.sh --no-cache      # force rebuild image without cache
#   ./scripts/build-windows-docker.sh --rust-only     # only compile the Rust binary
#   ./scripts/build-windows-docker.sh --bundle-only   # only run NSIS bundling
#
# First-time setup:
#   ./scripts/install-sccache.sh                        # one-time, ~10min
#   ./scripts/build-windows-docker.sh
#
# Why Docker: cargo-xwin needs Windows MSVC linker + SDK + headers. Installing
# those on the host is invasive (hundreds of MB). The Docker image bakes them
# in and amortizes the cost across every build.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

IMAGE_NAME="miracle-claw-build"
DOCKERFILE="Dockerfile.build"
OUTPUT_DIR="$REPO_ROOT/dist-installers/windows"
SCCACHE_DIR="${SCCACHE_DIR:-$HOME/.cache/sccache}"

REBUILD_IMAGE=false
NO_CACHE=false
RUST_ONLY=false
BUNDLE_ONLY=false

for arg in "$@"; do
    case "$arg" in
        --rebuild-image) REBUILD_IMAGE=true ;;
        --no-cache)      NO_CACHE=true ;;
        --rust-only)     RUST_ONLY=true ;;
        --bundle-only)   BUNDLE_ONLY=true ;;
        -h|--help)
            sed -n '3,30p' "$0"
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

if ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1 || $REBUILD_IMAGE; then
    echo ">>> Building Docker image $IMAGE_NAME (first time ~6m)"
    if [ -f "$REPO_ROOT/scripts/install-sccache.sh" ]; then
        "$REPO_ROOT/scripts/install-sccache.sh" || true
    fi
    EXTRA=""
    $NO_CACHE && EXTRA="--no-cache"
    docker build $EXTRA -t "$IMAGE_NAME" -f "$DOCKERFILE" "$REPO_ROOT"
else
    echo ">>> Using existing Docker image $IMAGE_NAME"
fi

mkdir -p "$OUTPUT_DIR" "$SCCACHE_DIR"

DOCKER_ENV=(
    -e CARGO_NET_GIT_FETCH_WITH_CLI=true
    -e SCCACHE_DIR=/usr/local/cargo/sccache
    -e XWIN_CACHE_DIR=/usr/local/cargo/xwin-cache
    -e TAURI_BUILD_TARGET=x86_64-pc-windows-msvc
)

# CRITICAL: re-stage src-tauri/resources/ with the WINDOWS Node binary before
# the cross-compile runs. If we don't, bundle-runtime.sh's default --target
# host (Linux) leaves a 0-byte node.exe placeholder + a 124MB Linux ELF, and
# the installer fails at first run with "%1 is not a valid Win32 application"
# (os error 193). The --force flag re-downloads the Windows Node tarball even
# if the Linux one is cached.
echo ">>> Re-staging src-tauri/resources/ with --target windows Node..."
if ! bash "$REPO_ROOT/scripts/bundle-runtime.sh" --target windows --force; then
    echo "FATAL: bundle-runtime.sh --target windows failed" >&2
    exit 1
fi
echo ">>> bundle-runtime.sh complete. resources/node.exe:"
file "$RESOURCES_DIR/node.exe" 2>/dev/null || true

DOCKER_VOLUMES=(
    -v "$REPO_ROOT:/io"
    -v "$HOME/.cargo/registry:/usr/local/cargo/registry"
    -v "$HOME/.cargo/git:/usr/local/cargo/git"
    -v "$SCCACHE_DIR:/usr/local/cargo/sccache"
    -v "miracle-claw-xwin-cache:/usr/local/cargo/xwin-cache"
)

if $RUST_ONLY; then
    echo ">>> Rust-only build (compiles the .exe, no NSIS bundling)"
    CMD='cd /io/src-tauri && cargo xwin build --release --target x86_64-pc-windows-msvc --bin miracle-claw'
elif $BUNDLE_ONLY; then
    EXE_PATH="$REPO_ROOT/src-tauri/target/x86_64-pc-windows-msvc/release/miracle-claw.exe"
    if [[ ! -f "$EXE_PATH" ]]; then
        echo "ERROR: --bundle-only requires an existing $EXE_PATH" >&2
        exit 1
    fi
    echo ">>> Bundle-only rebuild (uses existing .exe, ~30s)"
    CMD='cd /io && npm run tauri -- build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis'
else
    echo ">>> Running full cross-compile (5-15m cold, ~1m with sccache warm)"
    CMD='cd /io && npm run tauri -- build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis'
fi

docker run --rm \
    "${DOCKER_VOLUMES[@]}" \
    "${DOCKER_ENV[@]}" \
    "$IMAGE_NAME" \
    bash -c "$CMD"

BUNDLE_SRC="$REPO_ROOT/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis"
if [[ -d "$BUNDLE_SRC" && -n "$(ls -A "$BUNDLE_SRC" 2>/dev/null)" ]]; then
    cp -v "$BUNDLE_SRC"/*.exe "$OUTPUT_DIR/" 2>/dev/null || true
    echo ""
    echo "================================================================="
    echo "  Windows installers copied to: $OUTPUT_DIR"
    echo "================================================================="
    ls -lh "$OUTPUT_DIR"/*.exe 2>/dev/null || echo "(no .exe files found — check build output above)"

    # v1.7.3: also copy the latest installer to C:\Users\Adeal\Desktop
    # with the underscore format David uses (MiracleClaw_<ver>+<n>_x64-setup.exe).
    # The format matches the v1.2.0+2 baseline installer so we don't end up
    # with a version on Desktop that the user can't find.
    # v1.7.10 bugfix: don't use `ls -t | head -1` to find the latest
    # installer — when multiple installers get rebuilt in the same
    # second (NSIS bundle touches all *.exe files at once), mtimes
    # tie and `ls -t` falls back to inode/lexicographic order, which
    # doesn't match semver. We were shipping v1.7.9 installers with
    # v1.7.10 filenames. Match the actual TAURI_VERSION we just built
    # (read from tauri.conf.json) instead. See Lesson 229 in MEMORY.md.
    TAURI_VERSION=$(grep -oE '"version": *"[^"]+"' "$REPO_ROOT/src-tauri/tauri.conf.json" | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
    LATEST_INSTALLER="$OUTPUT_DIR/MiracleClaw_${TAURI_VERSION}_x64-setup.exe"
    if [[ -n "$TAURI_VERSION" && -f "$LATEST_INSTALLER" ]]; then
        DESKTOP_NAME="MiracleClaw_${TAURI_VERSION}_x64-setup.exe"
        DESKTOP_PATH="/mnt/c/Users/Adeal/Desktop/${DESKTOP_NAME}"
        if cp -v "$LATEST_INSTALLER" "$DESKTOP_PATH" 2>/dev/null; then
            echo "  Desktop installer: $DESKTOP_PATH"
            md5sum "$DESKTOP_PATH" 2>/dev/null
        else
            echo "  (could not write to /mnt/c/Users/Adeal/Desktop — run from WSL2 with Desktop mounted)"
        fi
    fi
else
    if $RUST_ONLY; then
        echo ""
        echo "================================================================="
        echo "  --rust-only: Rust binary built at:"
        echo "  src-tauri/target/x86_64-pc-windows-msvc/release/miracle-claw.exe"
        echo "  Run again without --rust-only to bundle into an installer."
        echo "================================================================="
    else
        echo ""
        echo "  WARNING: $BUNDLE_SRC not found or empty"
        echo "  Build may have failed. Check output above."
        echo "================================================================="
        exit 1
    fi
fi
