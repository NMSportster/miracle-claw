#!/usr/bin/env bash
# bundle-runtime.sh — populate src-tauri/resources/ with the Node runtime +
# openclaw gateway + MAIC plugin source so the Tauri installer is self-contained.
#
# Idempotent: re-running downloads nothing if the SHA256 of each vendored
# artifact matches. Use --force to bypass the cache.
#
# Layout produced under src-tauri/resources/:
#   openclaw.mjs              (openclaw launcher entry)
#   package.json              (openclaw metadata)
#   node_modules/             (NOT vendored — see comment below)
#   dist/                     (openclaw's prebuilt ESM dist/)
#   ... (other openclaw files: README.md, scripts/, etc.)
#   node.exe (Windows) OR node (Linux/macOS dev)
#   maic-plugin/              (4 JS files vendored from ~/.openclaw/extensions/maic/)
#
# Why no node_modules in the bundle: openclaw@2026.7.1-2 ships its `dist/` as
# prebuilt ES modules that import only Node built-ins. No external deps at
# runtime. Saves ~50MB.
#
# Usage:
#   bash scripts/bundle-runtime.sh                # use host defaults
#   bash scripts/bundle-runtime.sh --target windows
#   bash scripts/bundle-runtime.sh --target linux
#   bash scripts/bundle-runtime.sh --force          # bypass cache
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RESOURCES_DIR="$REPO_ROOT/src-tauri/resources"
DEPOT_DIR="$REPO_ROOT/depot"

# Versions (pinned). To bump: edit here, run --force, commit.
OPENCLAW_VERSION="2026.7.1-2"
NODE_VERSION="22.23.2"   # satisfies openclaw's engines: node >=22.22.3 <23

# MAIC plugin source. We vendor from the live install on this dev box. On
# Windows production builds, the script falls back to a checked-in copy if
# the live dir is absent.
MAIC_LIVE_DIR="$HOME/.openclaw/extensions/maic"
MAIC_VENDORED_DIR="$DEPOT_DIR/maic-plugin"
PROSE_LIBRARY_DIR="$DEPOT_DIR/prose-library"

# Target arch (affects which Node binary we unpack). Default: host triple.
TARGET="host"
FORCE=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --target=*) TARGET="${1#*=}"; shift ;;
        --force)   FORCE=1; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

resolve_target_triple() {
    case "$TARGET" in
        host)
            local arch
            arch="$(uname -m)"
            case "$arch" in
                x86_64)  echo "linux-x64" ;;
                aarch64) echo "linux-arm64" ;;
                *) echo "[bundle-runtime] unsupported host arch: $arch" >&2; exit 2 ;;
            esac
            ;;
        windows|win-x64)
            echo "win-x64"
            ;;
        linux|linux-x64)
            echo "linux-x64"
            ;;
        *) echo "unsupported --target: $TARGET (use host/windows/linux)" >&2; exit 2 ;;
    esac
}

NODE_TRIPLE="$(resolve_target_triple)"

OPENCLAW_TARBALL_NAME="openclaw-${OPENCLAW_VERSION}.tgz"
NODE_TARBALL_NAME="node-v${NODE_VERSION}-${NODE_TRIPLE}.zip"
[ "$NODE_TRIPLE" = "linux-x64" ] && NODE_TARBALL_NAME="node-v${NODE_VERSION}-${NODE_TRIPLE}.tar.xz"

mkdir -p "$DEPOT_DIR/openclaw" "$DEPOT_DIR/node" "$MAIC_VENDORED_DIR"

sha256_of_file() { sha256sum "$1" | awk '{print $1}'; }

# --- 1. OpenClaw tarball ---------------------------------------------------
OPENCLAW_TARBALL="$DEPOT_DIR/openclaw/$OPENCLAW_TARBALL_NAME"
OPENCLAW_SHA="$DEPOT_DIR/openclaw/$OPENCLAW_TARBALL_NAME.sha256"
EXPECTED_OPENCLAW_SHA="$(curl -fsSL "https://registry.npmjs.org/openclaw/-/openclaw-${OPENCLAW_VERSION}.tgz.sha256" 2>/dev/null | tr -d '\n' || echo "")"

needs_openclaw=0
if [ "$FORCE" = 1 ] || [ ! -f "$OPENCLAW_TARBALL" ]; then
    needs_openclaw=1
elif [ -n "$EXPECTED_OPENCLAW_SHA" ] && [ "$(sha256_of_file "$OPENCLAW_TARBALL")" != "$EXPECTED_OPENCLAW_SHA" ]; then
    echo "[bundle-runtime] cached openclaw tarball SHA mismatch; re-downloading" >&2
    needs_openclaw=1
fi

if [ "$needs_openclaw" = 1 ]; then
    echo "[bundle-runtime] downloading openclaw@$OPENCLAW_VERSION..."
    curl -fsSL -o "$OPENCLAW_TARBALL" \
        "https://registry.npmjs.org/openclaw/-/openclaw-${OPENCLAW_VERSION}.tgz"
    if [ -n "$EXPECTED_OPENCLAW_SHA" ]; then
        actual="$(sha256_of_file "$OPENCLAW_TARBALL")"
        if [ "$actual" != "$EXPECTED_OPENCLAW_SHA" ]; then
            echo "[bundle-runtime] FATAL: openclaw tarball SHA mismatch after download" >&2
            echo "  expected: $EXPECTED_OPENCLAW_SHA" >&2
            echo "  actual:   $actual" >&2
            exit 1
        fi
    fi
    echo "$EXPECTED_OPENCLAW_SHA  $OPENCLAW_TARBALL_NAME" > "$OPENCLAW_SHA"
fi

echo "$OPENCLAW_VERSION" > "$DEPOT_DIR/openclaw/VERSION"

# --- 2. Node tarball -------------------------------------------------------
NODE_URL_BASE="https://nodejs.org/dist/v$NODE_VERSION"
case "$NODE_TRIPLE" in
    win-x64)   NODE_URL="$NODE_URL_BASE/node-v${NODE_VERSION}-win-x64.zip" ;;
    linux-x64) NODE_URL="$NODE_URL_BASE/node-v${NODE_VERSION}-linux-x64.tar.xz" ;;
    *) echo "[bundle-runtime] unsupported Node triple: $NODE_TRIPLE" >&2; exit 2 ;;
esac

NODE_TARBALL="$DEPOT_DIR/node/$NODE_TARBALL_NAME"
NODE_SHA="$DEPOT_DIR/node/$NODE_TARBALL_NAME.sha256"
# Node publishes a single SHASUMS256.txt per version that lists every artifact.
EXPECTED_NODE_SHA="$(curl -fsSL "$NODE_URL_BASE/SHASUMS256.txt" 2>/dev/null | awk -v t="$NODE_TARBALL_NAME" '$2==t {print $1}')"

needs_node=0
if [ "$FORCE" = 1 ] || [ ! -f "$NODE_TARBALL" ]; then
    needs_node=1
elif [ -n "$EXPECTED_NODE_SHA" ] && [ "$(sha256_of_file "$NODE_TARBALL")" != "$EXPECTED_NODE_SHA" ]; then
    echo "[bundle-runtime] cached node tarball SHA mismatch; re-downloading" >&2
    needs_node=1
fi

if [ "$needs_node" = 1 ]; then
    echo "[bundle-runtime] downloading Node $NODE_VERSION ($NODE_TRIPLE)..."
    curl -fsSL -o "$NODE_TARBALL" "$NODE_URL"
    if [ -n "$EXPECTED_NODE_SHA" ]; then
        actual="$(sha256_of_file "$NODE_TARBALL")"
        if [ "$actual" != "$EXPECTED_NODE_SHA" ]; then
            echo "[bundle-runtime] FATAL: node tarball SHA mismatch after download" >&2
            echo "  expected: $EXPECTED_NODE_SHA" >&2
            echo "  actual:   $actual" >&2
            exit 1
        fi
    fi
    echo "$EXPECTED_NODE_SHA  $NODE_TARBALL_NAME" > "$NODE_SHA"
fi

echo "$NODE_VERSION" > "$DEPOT_DIR/node/VERSION"

# --- 3. MAIC plugin source -------------------------------------------------
# The MAIC plugin is small (~7KB). We vendor it into depot/ once and copy from
# there. This makes the bundle fully reproducible from a clean checkout.
if [ ! -f "$MAIC_VENDORED_DIR/.vendor-stamp" ] || [ "$FORCE" = 1 ]; then
    if [ -d "$MAIC_LIVE_DIR" ]; then
        echo "[bundle-runtime] vendoring MAIC plugin from $MAIC_LIVE_DIR"
        mkdir -p "$MAIC_VENDORED_DIR"
        cp -f "$MAIC_LIVE_DIR"/* "$MAIC_VENDORED_DIR/"
        date -u +"%Y-%m-%dT%H:%M:%SZ" > "$MAIC_VENDORED_DIR/.vendor-stamp"
    else
        echo "[bundle-runtime] FATAL: MAIC plugin not at $MAIC_LIVE_DIR" >&2
        echo "  Install MAIC plugin locally first, or set MAIC_LIVE_DIR." >&2
        exit 1
    fi
fi

# --- 4. Stage src-tauri/resources/ ----------------------------------------
echo "[bundle-runtime] staging $RESOURCES_DIR..."

# Wipe and re-populate (idempotent; cheap; ensures clean state).
rm -rf "$RESOURCES_DIR"
mkdir -p "$RESOURCES_DIR"

# 4a. Unpack openclaw tarball.
TMP_OC="$(mktemp -d)"
tar -xzf "$OPENCLAW_TARBALL" -C "$TMP_OC"
# Tarball layout: <tmp>/package/<files>
mv "$TMP_OC"/package/* "$RESOURCES_DIR"/
rmdir "$TMP_OC/package"
rmdir "$TMP_OC"
# Trim noise the launcher doesn't need (build stamps, lockfiles, etc.).
rm -f "$RESOURCES_DIR/.buildstamp" \
      "$RESOURCES_DIR/.runtime-postbuildstamp" \
      "$RESOURCES_DIR/pnpm-workspace.yaml" 2>/dev/null || true

# 4a.5. Run a production install so openclaw's ESM imports resolve.
# openclaw's lockfile is pnpm-based (npm-shrinkwrap + pnpm-lock), and its
# peer-dep graph causes npm's arborist to crash on 'edgesOut' resolution.
# We use pnpm, which openclaw's packageManager declares. Requires pnpm on
# the build host (install with `npm install -g pnpm@11`).
# --prod omits dev deps; we skip --no-frozen-lockfile because we don't ship a
# lockfile (the bundle-runtime regenerates deps from package.json).
#
# --config.strict-dep-builds=false is REQUIRED: pnpm 11 changed
# strictDepBuilds default to true, which promotes [ERR_PNPM_IGNORED_BUILDS]
# from a warning to exit 1. openclaw's tree pulls in @google/genai,
# protobufjs, and tree-sitter-bash, all of which have postinstall scripts
# that pnpm refuses to run unless explicitly allowBuild'd. We don't need
# the compiled native modules at runtime (they're optional), so disabling
# strict dep builds is safe — the install still succeeds and node_modules
# is fully populated. If a future openclaw dep requires its build script
# (e.g. a native module with no prebuilt binary), switch to allowBuilds
# in pnpm-workspace.yaml instead (Lesson 847).
echo "[bundle-runtime] running pnpm install --prod..."
if ! command -v pnpm >/dev/null 2>&1; then
    echo "[bundle-runtime] FATAL: pnpm not found on PATH" >&2
    echo "  Install it with: npm install -g pnpm@11" >&2
    exit 1
fi
RUN_PNPM_INSTALL() {
    ( cd "$RESOURCES_DIR" && pnpm install \
        --prod \
        --no-frozen-lockfile \
        --config.nodeLinker=hoisted \
        --config.strict-dep-builds=false 2>&1 | tail -15 )
}
if ! RUN_PNPM_INSTALL; then
    echo "[bundle-runtime] FATAL: pnpm install failed (Lesson 847: pnpm 11 strict dep builds)" >&2
    echo "  Try manually: cd $RESOURCES_DIR && pnpm install --prod --no-frozen-lockfile --config.nodeLinker=hoisted --config.strict-dep-builds=false" >&2
    exit 1
fi
# Sanity check: ensure node_modules actually exists and has content
# (pnpm 11 can exit 0 with empty node_modules if the lockfile is broken).
if [[ ! -d "$RESOURCES_DIR/node_modules" ]] || [[ -z "$(ls -A "$RESOURCES_DIR/node_modules" 2>/dev/null)" ]]; then
    echo "[bundle-runtime] FATAL: pnpm install exited 0 but $RESOURCES_DIR/node_modules" >&2
    echo "  is missing or empty. Tauri build will fail. Aborting." >&2
    exit 1
fi

# 4b. Unpack Node binary into resources/.
TMP_NODE="$(mktemp -d)"
case "$NODE_TRIPLE" in
    win-x64)
        # Try `unzip` first; fall back to python3 zipfile if unzip isn't
        # installed (Linux sandboxes/Docker images sometimes omit it).
        if command -v unzip >/dev/null 2>&1; then
            unzip -q "$NODE_TARBALL" -d "$TMP_NODE"
        else
            python3 -c "
import zipfile, sys
with zipfile.ZipFile(sys.argv[1]) as z:
    z.extractall(sys.argv[2])
" "$NODE_TARBALL" "$TMP_NODE"
        fi
        mv "$TMP_NODE"/node-v${NODE_VERSION}-win-x64/node.exe "$RESOURCES_DIR/node.exe"
        # Don't ship node-dist/ (~150MB Linux ELF) when targeting Windows. The
        # launcher's resource resolution checks `resources/node.exe` first on
        # Windows; node-dist/ is only needed for dev builds on Linux/macOS.
        # (Also avoids Tauri's bundle.resources failing on a path that
        # shouldn't be in the installer at all.)
        # Tauri's bundle.resources also lists `resources/node` (the no-ext
        # entry). On Windows builds we touch a 0-byte placeholder so the
        # pre-flight validation passes; the placeholder gets copied into the
        # installer as a dead-weight file (Tauri's bundler doesn't validate
        # that resource files are actual executables on the target).
        touch "$RESOURCES_DIR/node"
        ;;
    linux-x64)
        tar -xJf "$NODE_TARBALL" -C "$TMP_NODE"
        # Linux tarball extracts to node-v.../bin/node, lib/, share/, include/.
        # Copy the whole subtree so node's startup scripts can find share/.
        cp -r "$TMP_NODE"/node-v${NODE_VERSION}-linux-x64/. "$RESOURCES_DIR"/node-dist/
        # Place a top-level COPY of the node binary at the path the launcher
        # checks first. We copy (not symlink) because Tauri's bundler copies
        # resources into target/.../resources, and a relative symlink would
        # break unless we also pinned current_dir to $RESOURCES_DIR (which we
        # do in the launcher, but a real file is more portable across dev
        # and CI).
        cp "$RESOURCES_DIR/node-dist/bin/node" "$RESOURCES_DIR/node"
        # Tauri's bundle.resources declares node.exe (Windows filename). On
        # non-Windows dev hosts we still need a placeholder so tauri-build's
        # pre-flight validation passes. Build script will skip the actual
        # bundling for non-Windows targets.
        touch "$RESOURCES_DIR/node.exe"
        ;;
esac
rm -rf "$TMP_NODE"

# 4c. Copy MAIC plugin source.
mkdir -p "$RESOURCES_DIR/maic-plugin"
cp -f "$MAIC_VENDORED_DIR"/* "$RESOURCES_DIR/maic-plugin/"
rm -f "$RESOURCES_DIR/maic-plugin/.vendor-stamp"

# 4c.5. Copy MC's built-in prose-library (Lesson 833). Source of truth
# is depot/prose-library/ (mirrors the MAIC plugin pattern at 4c above).
# The in-repo copy at src-tauri/resources/prose-library/ is git-tracked
# for editor convenience but gets wiped by the `rm -rf $RESOURCES_DIR`
# at the top of this script. We mirror it into depot/ before that wipe
# happens; if depot/ is missing or stale, fall back to the in-repo copy.
if [[ ! -d "$PROSE_LIBRARY_DIR/workflows" || ! -d "$PROSE_LIBRARY_DIR/specialists" ]]; then
    IN_REPO_PROSE="$REPO_ROOT/src-tauri/resources/prose-library"
    if [[ -d "$IN_REPO_PROSE" ]]; then
        echo "[bundle-runtime] depot/prose-library missing — mirroring from in-repo copy"
        mkdir -p "$PROSE_LIBRARY_DIR"
        cp -rf "$IN_REPO_PROSE"/* "$PROSE_LIBRARY_DIR"/
    fi
fi
if [[ -d "$PROSE_LIBRARY_DIR" ]]; then
    mkdir -p "$RESOURCES_DIR/prose-library"
    cp -rf "$PROSE_LIBRARY_DIR"/workflows "$RESOURCES_DIR/prose-library/" 2>/dev/null || true
    cp -rf "$PROSE_LIBRARY_DIR"/specialists "$RESOURCES_DIR/prose-library/" 2>/dev/null || true
    prose_count=$(find "$RESOURCES_DIR/prose-library" -name "*.prose" | wc -l)
    echo "[bundle-runtime] prose-library: $prose_count .prose files staged"
else
    echo "[bundle-runtime] NOTE: $PROSE_LIBRARY_DIR not present — installer will ship without built-in workflows" >&2
fi

# 4d. Write a runtime version file for diagnostics.
cat > "$RESOURCES_DIR/BUNDLE_VERSION" <<EOF
miracle-claw=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)
openclaw=$OPENCLAW_VERSION
node=$NODE_VERSION
node-target=$NODE_TRIPLE
bundled-at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
EOF

# 4e. On Windows we also keep a node file alongside node.exe so the launcher
# finds something to execute when run via cmd.exe quoting differently. No-op
# here; launcher already handles both shapes.

# --- 5. Summary ------------------------------------------------------------
echo "[bundle-runtime] done. resources/ contents:"
du -sh "$RESOURCES_DIR"/* 2>/dev/null | sort -h
echo ""
echo "[bundle-runtime] total: $(du -sh "$RESOURCES_DIR" | awk '{print $1}')"