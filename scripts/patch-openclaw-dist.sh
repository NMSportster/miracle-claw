#!/usr/bin/env bash
# ============================================================================
# scripts/patch-openclaw-dist.sh
# ============================================================================
#
# Apply MiracleClaw-specific patches to the freshly-extracted OpenClaw dist
# that lives under src-tauri/resources/. Run AFTER bundle-runtime.sh has
# repopulated resources/ from the cached openclaw tarball — patches get
# wiped on every --force bundle-runtime.sh run, so this is a rebuild step.
#
# Patches live in depot/openclaw-patches/<subpath>/. The naming convention
# determines how each patch is applied:
#
#   <name>.html         APPEND to end of src-tauri/resources/<relpath>
#   <name>.html.insert  INSERT before </body> in src-tauri/resources/<relpath>
#                       (correct HTML placement; Lesson 511)
#   <name>.js           WRITE  to src-tauri/resources/<relpath>
#   <name>.json         WRITE  to src-tauri/resources/<relpath> (merge? skip)
#   <name>.delete       RECORD only (mark for future deletion)
#
# Idempotency for APPEND patches:
#   Each patch file must start with a marker line:
#     <!-- MC-PATCH: <id> -->
#   The patcher searches the target file for that exact marker. If present,
#   skip (already applied). If absent, append the marker + patch contents
#   to the end of the target file.
#
# Idempotency for WRITE patches:
#   For JS/CSS files we write, the patcher compares target mtime against a
#   record in .applied-patches.json (under resources/). If file content
#   matches the source, skip.
#
# Adding a new patch:
#   1. Drop a file under depot/openclaw-patches/<subpath>/.
#   2. For HTML appends: start the file with `<!-- MC-PATCH: <your-id> -->`
#      on its own line. The patcher will skip if it sees that marker.
#   3. Document it in depot/openclaw-patches/MANIFEST.md.
#
# Usage:
#   bash scripts/patch-openclaw-dist.sh                  # apply all patches
#   bash scripts/patch-openclaw-dist.sh --dry-run         # report only
#   bash scripts/patch-openclaw-dist.sh --list            # show what's patched
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RESOURCES_DIR="$REPO_ROOT/src-tauri/resources"
PATCHES_DIR="$REPO_ROOT/depot/openclaw-patches"
APPLIED_LOG="$RESOURCES_DIR/.mc-applied-patches.log"

DRY_RUN=false
LIST_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        --list)    LIST_ONLY=true ;;
        -h|--help)
            sed -n '3,40p' "$0"
            exit 0
            ;;
        *) echo "Unknown arg: $arg" >&2; exit 1 ;;
    esac
done

if [[ ! -d "$RESOURCES_DIR" ]]; then
    echo "FATAL: $RESOURCES_DIR not found. Run bundle-runtime.sh first." >&2
    exit 1
fi
if [[ ! -d "$PATCHES_DIR" ]]; then
    echo "FATAL: $PATCHES_DIR not found. Nothing to patch." >&2
    exit 1
fi

# Discover patches: every file under depot/openclaw-patches/ except MANIFEST.md.
# Maps to a target file under src-tauri/resources/<relpath>.
patch_files=$(find "$PATCHES_DIR" -type f ! -name MANIFEST.md ! -name '.*' | sort)

if $LIST_ONLY; then
    echo "Configured patches (source-of-truth under $PATCHES_DIR):"
    echo "$patch_files" | sed "s|$PATCHES_DIR/||"
    exit 0
fi

if [[ -z "$patch_files" ]]; then
    echo "[patch-openclaw-dist] no patches configured under $PATCHES_DIR"
    exit 0
fi

# Reset the applied-patches log on every run. If --dry-run, don't write it.
if ! $DRY_RUN; then
    : > "$APPLIED_LOG"
fi

echo "[patch-openclaw-dist] applying patches from $PATCHES_DIR → $RESOURCES_DIR"

applied=0
skipped=0

while IFS= read -r patch_file; do
    rel="${patch_file#$PATCHES_DIR/}"
    # *.html.insert patches target the same-named .html file (Lesson 511).
    # The .insert suffix is a patcher-internal mode marker, not part of
    # the destination filename.
    if [[ "$rel" == *.html.insert ]]; then
        rel="${rel%.insert}"
    fi
    target="$RESOURCES_DIR/$rel"
    target_dir="$(dirname "$target")"

    if [[ ! -d "$target_dir" ]]; then
        echo "  SKIP $rel — target dir $target_dir does not exist (openclaw tarball may have changed layout)"
        skipped=$((skipped + 1))
        continue
    fi

    case "$patch_file" in
        *.html.insert)
            # INSERT patch (Lesson 511). Inserts patch contents immediately
            # before `</body>`, which is the correct HTML placement for a
            # <script> tag. Append-only patches land AFTER </html>, which
            # is invalid HTML and can fail to load in stricter parsers.
            #
            # Idempotency: first line must be `<!-- MC-PATCH: <id> -->`.
            # If that marker is already in the target (anywhere), skip.
            first_line=$(head -n 1 "$patch_file")
            marker=""
            if [[ "$first_line" =~ ^\<!--[[:space:]]*MC-PATCH:[[:space:]]*([^[:space:]]+)[[:space:]]*--\>$ ]]; then
                marker="<!-- MC-PATCH: ${BASH_REMATCH[1]} -->"
            fi

            if [[ -z "$marker" ]]; then
                echo "  SKIP $rel — *.html.insert patches require first line `<!-- MC-PATCH: <id> -->`"
                skipped=$((skipped + 1))
                continue
            fi

            if grep -qF "$marker" "$target" 2>/dev/null; then
                echo "  SKIP $rel (marker $marker already present)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! grep -qF '</body>' "$target" 2>/dev/null; then
                echo "  SKIP $rel — target has no </body> sentinel (openclaw HTML structure changed?)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                # Strip the leading marker line from the patch file (the
                # patcher injects it as a marker before insertion).
                body="$(tail -n +2 "$patch_file")"
                tmp="$(mktemp)"
                {
                    echo "$marker"
                    echo "<!-- Lesson 511: inserted by scripts/patch-openclaw-dist.sh before </body> -->"
                    printf '%s\n' "$body"
                } > "$tmp"
                # Insert before </body>: split target, splice in tmp, recombine.
                awk -v ins="$tmp" '
                    /<\/body>/ && !inserted {
                        while ((getline line < ins) > 0) print line
                        close(ins)
                        inserted = 1
                    }
                    { print }
                ' "$target" > "$target.new" && mv "$target.new" "$target"
                rm -f "$tmp"
                echo "$rel $marker" >> "$APPLIED_LOG"
            fi
            echo "  INSERT $rel (marker $marker, before </body>)"
            applied=$((applied + 1))
            ;;

        *.html)
            # APPEND patch. Idempotency: first line must be `<!-- MC-PATCH: <id> -->`.
            # If that marker is in the target, skip.
            # NOTE: append-only patches land AFTER </html>, which is invalid
            # HTML placement for <script>. Prefer *.html.insert for new patches.
            first_line=$(head -n 1 "$patch_file")
            marker=""
            if [[ "$first_line" =~ ^\<!--[[:space:]]*MC-PATCH:[[:space:]]*([^[:space:]]+)[[:space:]]*--\>$ ]]; then
                marker="<!-- MC-PATCH: ${BASH_REMATCH[1]} -->"
            fi

            if [[ -n "$marker" ]] && grep -qF "$marker" "$target" 2>/dev/null; then
                echo "  SKIP $rel (marker $marker already present)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                # Ensure marker is present in the patch (auto-prepend if missing).
                if [[ -z "$marker" ]]; then
                    marker="<!-- MC-PATCH: $(basename "$rel" .html)-$(date +%s) -->"
                    tmp="$(mktemp)"
                    {
                        echo "$marker"
                        cat "$patch_file"
                    } > "$tmp"
                    cat "$tmp" >> "$target"
                    rm -f "$tmp"
                else
                    cat "$patch_file" >> "$target"
                fi
                echo "$rel $marker" >> "$APPLIED_LOG"
            fi
            echo "  APPEND $rel (marker $marker)"
            applied=$((applied + 1))
            ;;

        *.js|*.css|*.mjs|*.cjs)
            # WRITE patch — copy the patch file over the target. Idempotent
            # via content comparison. Match tarball perms (0600) so files
            # don't leak into the installer with looser ACLs than upstream.
            if [[ -f "$target" ]] && cmp -s "$patch_file" "$target"; then
                echo "  SKIP $rel (content already matches)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                cp "$patch_file" "$target"
                chmod 600 "$target" 2>/dev/null || true
                echo "$rel WRITE" >> "$APPLIED_LOG"
            fi
            echo "  WRITE  $rel"
            applied=$((applied + 1))
            ;;

        *.json)
            # WRITE patch (no merge logic yet — full overwrite).
            if [[ -f "$target" ]] && cmp -s "$patch_file" "$target"; then
                echo "  SKIP $rel (content already matches)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                cp "$patch_file" "$target"
                echo "$rel JSON-WRITE" >> "$APPLIED_LOG"
            fi
            echo "  JSON-WRITE $rel"
            applied=$((applied + 1))
            ;;

        *)
            echo "  SKIP $rel — unknown file type (only .html, .html.insert, .js, .css, .mjs, .cjs, .json supported)"
            skipped=$((skipped + 1))
            ;;
    esac
done <<< "$patch_files"

echo "[patch-openclaw-dist] done: $applied applied, $skipped skipped"
echo "[patch-openclaw-dist] applied log: $APPLIED_LOG"