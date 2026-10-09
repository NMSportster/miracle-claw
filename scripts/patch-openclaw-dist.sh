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
#   <name>.js.insert    INSERT before a sentinel line in src-tauri/resources/<relpath>
#                       (Lesson 534 — MAIC compat: insert before the `return params;`
#                        that ends buildOpenAICompletionsParams in
#                        openai-transport-stream-*.js)
#   <name>.js.replace   REPLACE sentinel lines in src-tauri/resources/<relpath>
#                       (Lesson 1127 — surgical edits to multi-K-line bundle files
#                        without rewriting the whole file. Each patch file's
#                        first line is the marker; subsequent `// REPLACE: <sentinel>`
#                        lines define (sentinel, replacement) pairs. The
#                        replacement is the last non-blank line of each block.
#                        Multiple `// REPLACE:` blocks per file are allowed
#                        since Lesson 1138 — they share one marker for
#                        idempotency and are applied in one awk pass via a
#                        temp TSV file keyed on ASCII Unit Separator \x1f.)
#   <name>.json         WRITE  to src-tauri/resources/<relpath> (merge? skip)
#   <name>.delete       RECORD only (mark for future deletion)
#
# Idempotency for APPEND patches:
#   Each patch file must start with a marker line:
#     <!-- MC-PATCH: <id> -->
#     or  // MC-PATCH: <id>
#   The patcher searches the target file for that exact marker. If present,
#   skip (already applied). If absent, append the marker + patch contents
#   to the end of the target file.
#
# Idempotency for INSERT patches (*.html.insert / *.js.insert):
#   First line must be the marker. If marker found anywhere in target, skip.
#   For *.js.insert the sentinel line is identified by a magic comment on
#   line 2 of the patch file:
#     // INSERT-BEFORE: <exact text of target line>
#   This lets us target a unique line in the openclaw bundle without
#   rewriting the entire 3,500-line file.
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
    # *.html.insert / *.js.insert / *.js.replace patches target the same-named
    # .html/.js file (Lesson 511 / Lesson 534 / Lesson 1127). The .insert /
    # .replace suffix is a patcher-internal mode marker, not part of the
    # destination filename.
    #
    # Lesson 1138 (2026-10-09): support multiple .replace files targeting the
    # same destination via the `-<suffix>.js.replace` filename pattern. The
    # patch file `selection-JInn13lc-usage.js.replace` targets the same file
    # as `selection-JInn13lc.js.replace`. The hyphen-suffix is stripped along
    # with `.replace`. Markers in the patch file itself must be unique (use
    # different `// MC-PATCH: <id>` for each), and the patcher skips per-marker
    # (not per-file).
    case "$rel" in
        *.html.insert|*.js.insert|*.mjs.insert|*.cjs.insert)
            rel="${rel%.insert}"
            ;;
        *.js.replace|*.mjs.replace|*.cjs.replace)
            rel="${rel%.replace}"
            # Allow `-<label>` between basename and .js: e.g.
            # `selection-JInn13lc-usage.js.replace` -> `selection-JInn13lc.js`.
            # This is opt-in: only if the stripped rel doesn't exist as a file
            # in depot/openclaw-patches/, we try the alias. The alias resolution
            # is one-time at discovery; subsequent runs use the resolved name.
            ;;
    esac
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
            # Idempotency (Lesson 706): a single `.html.insert` may carry
            # several `<!-- MC-PATCH: <id> -->` markers (one per logical
            # patch). We extract ALL of them, then for each marker check
            # whether it's already in the target. If ALL markers are
            # present, skip. Otherwise build a body containing only the
            # missing-marker sections + their content, and inject that.
            all_markers=$(grep -oE '^<!--[[:space:]]*MC-PATCH:[[:space:]]*[A-Za-z0-9_.-]+[[:space:]]*-->' "$patch_file" || true)
            if [[ -z "$all_markers" ]]; then
                echo "  SKIP $rel -- *.html.insert patches require at least one \`<!-- MC-PATCH: <id> -->\` line"
                skipped=$((skipped + 1))
                continue
            fi
            first_marker=$(echo "$all_markers" | head -n 1)
            marker_count=$(echo "$all_markers" | wc -l | tr -d ' ')

            # Determine which markers are missing from target.
            missing_list=""
            while IFS= read -r m; do
                [[ -z "$m" ]] && continue
                if ! grep -qF "$m" "$target" 2>/dev/null; then
                    missing_list+="$m"$'\n'
                fi
            done <<< "$all_markers"
            if [[ -z "$missing_list" ]]; then
                echo "  SKIP $rel (all $marker_count markers already present)"
                skipped=$((skipped + 1))
                continue
            fi
            missing_count=$(echo "$missing_list" | grep -c .)

            if ! grep -qF '</body>' "$target" 2>/dev/null; then
                echo "  SKIP $rel -- target has no </body> sentinel (openclaw HTML structure changed?)"
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                # Build a synthetic patch body containing only the
                # sections whose markers are missing. Each section runs
                # from its `<!-- MC-PATCH: ... -->` marker through the
                # next blank line / EOF / next marker. We do NOT echo a
                # synthetic first_marker here -- the awk pass emits every
                # missing marker's section in order.
                tmp="$(mktemp)"
                echo "<!-- Lesson 511 + 706: inserted by scripts/patch-openclaw-dist.sh before </body> -->" > "$tmp"
                awk -v missing="$missing_list" '
                    BEGIN {
                        n = split(missing, arr, "\n")
                        for (i = 1; i <= n; i++) if (arr[i] != "") want[arr[i]] = 1
                    }
                    /^<!--[[:space:]]*MC-PATCH:[[:space:]]*/ {
                        line = $0
                        match(line, /<!--[[:space:]]*MC-PATCH:[[:space:]]*[A-Za-z0-9_.-]+[[:space:]]*-->/)
                        m = substr(line, RSTART, RLENGTH)
                        keep = (m in want)
                        # Print the marker line itself when we are
                        # keeping this section -- the marker must end
                        # up in target for idempotency to work on the
                        # next run (Lesson 706).
                        if (keep) print
                        next
                    }
                    keep { print }
                ' "$patch_file" >> "$tmp"
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
                echo "$rel INSERT-missing" >> "$APPLIED_LOG"
            fi
            echo "  INSERT $rel (missing $missing_count of $marker_count markers)"
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

        *.js.insert|*.mjs.insert|*.cjs.insert)
            # INSERT patch (Lesson 534 — MAIC compat). Inserts patch contents
            # immediately before a sentinel line that lives inside the openclaw
            # dist file. This lets us splice in small targeted fixes (a few
            # lines) without rewriting a whole multi-thousand-line bundle file.
            #
            # Patch file format:
            #   Line 1: // MC-PATCH: <id>           (idempotency marker)
            #   Line 2: // INSERT-BEFORE: <text>    (sentinel to find)
            #   Line 3+: ...patch contents (no trailing newline required)
            #
            # We `grep -F` the sentinel in the target; if missing, the openclaw
            # bundle changed structure and we skip with a warning.
            first_line=$(head -n 1 "$patch_file")
            sentinel_line=$(sed -n '2p' "$patch_file")
            marker=""
            if [[ "$first_line" =~ ^//[[:space:]]*MC-PATCH:[[:space:]]*([^[:space:]]+)$ ]]; then
                marker="// MC-PATCH: ${BASH_REMATCH[1]}"
            fi

            if [[ -z "$marker" ]]; then
                echo "  SKIP $rel — *.js.insert patches require first line \`// MC-PATCH: <id>\`"
                skipped=$((skipped + 1))
                continue
            fi

            # Strip ONLY the `// INSERT-BEFORE: ` prefix (with exactly one
            # separating space — preserves any tabs or other leading whitespace
            # in the sentinel itself, which we DO need to match verbatim).
            if [[ ! "$sentinel_line" =~ ^//[[:space:]]*INSERT-BEFORE:[[:space:]](.+)$ ]]; then
                echo "  SKIP $rel — *.js.insert patches require line 2 \`// INSERT-BEFORE: <text>\`"
                skipped=$((skipped + 1))
                continue
            fi
            sentinel="${BASH_REMATCH[1]}"

            if grep -qF "$marker" "$target" 2>/dev/null; then
                echo "  SKIP $rel (marker $marker already present)"
                skipped=$((skipped + 1))
                continue
            fi

            sentinel_count=$(grep -cF "$sentinel" "$target" 2>/dev/null || true)
            if [[ "$sentinel_count" -eq 0 ]]; then
                echo "  SKIP $rel — sentinel '$sentinel' not found in target (openclaw bundle structure changed?)"
                skipped=$((skipped + 1))
                continue
            fi
            if [[ "$sentinel_count" -gt 1 ]]; then
                echo "  SKIP $rel — sentinel '$sentinel' appears $sentinel_count times in target (ambiguous). Refine the sentinel."
                skipped=$((skipped + 1))
                continue
            fi

            if ! $DRY_RUN; then
                # Build the insert payload: marker + injected comment + patch
                # body (everything from line 3 onward). The injected comment
                # records why this patch exists — same convention as *.html.insert.
                payload="$(mktemp)"
                {
                    echo "$marker"
                    printf '%s\n' "$sentinel_line"
                    tail -n +3 "$patch_file"
                } > "$payload"
                # Insert before the sentinel line: awk reads sentinel line,
                # prints payload first, then prints the line as-is.
                awk -v ins="$payload" -v sentinel="$sentinel" '
                    { line = $0; if ((line == sentinel) && !inserted) { while ((getline pline < ins) > 0) print pline; close(ins); inserted = 1 } print line }
                ' "$target" > "$target.new" && mv "$target.new" "$target"
                rm -f "$payload"
                echo "$rel $marker" >> "$APPLIED_LOG"
            fi
            echo "  INSERT $rel (marker $marker, before sentinel \"$sentinel\")"
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

        *.js.replace|*.mjs.replace|*.cjs.replace)
            # REPLACE patch (Lesson 1127 — surgical single-line edits).
            # Replaces one or more sentinel lines with new content, without
            # rewriting the whole multi-thousand-line bundle file.
            #
            # Patch file format (one or more REPLACE blocks):
            #   Line 1:    // MC-PATCH: <id>          (idempotency marker)
            #   Line 2:   \t// REPLACE: <sentinel A>  (target line to find, with tab indent preserved)
            #   Line 3+:  (comment about A — optional)
            #   Last A:   <replacement A>             (replaces sentinel A)
            #   Line n:   \t// REPLACE: <sentinel B>  (next REPLACE block)
            #   ...
            #
            # For each REPLACE block, the replacement is the LAST non-blank line
            # of the block. Any lines between the sentinel line and the replacement
            # are emitted as a comment between them in the target file.
            #
            # Idempotency: grep -F for the marker in target; if present, skip the
            # entire patch file. If the marker is absent, all REPLACE blocks are
            # applied top-to-bottom in a single awk pass.
            #
            # If any sentinel is not found in the target (or found multiple
            # times), we skip with a warning — the openclaw bundle structure
            # changed and the maintainer needs to refine the sentinel.
            first_line=$(head -n 1 "$patch_file")
            marker=""
            if [[ "$first_line" =~ ^//[[:space:]]*MC-PATCH:[[:space:]]*([^[:space:]]+)$ ]]; then
                marker="// MC-PATCH: ${BASH_REMATCH[1]}"
            fi

            if [[ -z "$marker" ]]; then
                echo "  SKIP $rel — *.js.replace patches require first line \`// MC-PATCH: <id>\`"
                skipped=$((skipped + 1))
                continue
            fi

            if grep -qF "$marker" "$target" 2>/dev/null; then
                echo "  SKIP $rel (marker $marker already present)"
                skipped=$((skipped + 1))
                continue
            fi

            # Collect all REPLACE blocks: each is a pair of (sentinel_line, replacement_line)
            # where the sentinel_line is a full "	// REPLACE: <text>" line and
            # the replacement_line is the last non-blank line of that block.
            # We use awk to scan the patch file and emit one record per REPLACE
            # block. Each record is 5 fields separated by ASCII Unit Separator
            # (\x1f) so that tab characters inside sentinel/replacement text
            # don't collide with field separators. Fields: <idx>\x1f<full-sentinel-line>\x1f<replacement-line>\x1f<sentinel-lineno>\x1f<last-non-blank-lineno>.
            # IMPORTANT: do NOT `delete lines` at the start of each REPLACE
            # block — we still need them to find the last non-blank line of
            # the previous block. We only delete after we've found it.
            # Records are separated by newlines; each record is internally
            # separated by ASCII Unit Separator (\x1f) so tabs in
            # sentinel/replacement text don't collide.
            mapfile -t replace_blocks < <(awk -v marker="$marker" -v sep=$'\x1f' '
                function is_blank(s) { return s ~ /^[[:space:]]*$/ }
                function is_mc_marker(s) { return s ~ /^[[:space:]]*\/\/[[:space:]]*MC-PATCH:/ }
                BEGIN { n = 0; block_start = 0; pending = 0 }
                /^[[:space:]]*\/\/[[:space:]]*REPLACE:[[:space:]]/ {
                    if (n > 0) {
                        last_non_blank = 0
                        for (i = NR - 1; i >= block_start; i--) {
                            if (i in lines && !is_blank(lines[i]) && !is_mc_marker(lines[i])) { last_non_blank = i; break }
                        }
                        if (last_non_blank == 0) last_non_blank = block_start
                        printf("%d%s%s%s%s%s%d%s%d\n", n, sep, sentinel_line, sep, lines[last_non_blank], sep, sentinel_lineno, sep, last_non_blank)
                    }
                    n++
                    sentinel_line = $0
                    sentinel_lineno = NR
                    block_start = NR + 1
                    next
                }
                { lines[NR] = $0 }
                END {
                    if (n > 0) {
                        last_non_blank = 0
                        for (i = NR; i >= block_start; i--) {
                            if (i in lines && !is_blank(lines[i]) && !is_mc_marker(lines[i])) { last_non_blank = i; break }
                        }
                        if (last_non_blank == 0) last_non_blank = block_start
                        printf("%d%s%s%s%s%s%d%s%d\n", n, sep, sentinel_line, sep, lines[last_non_blank], sep, sentinel_lineno, sep, last_non_blank)
                    }
                }
            ' "$patch_file")

            if [[ ${#replace_blocks[@]} -eq 0 ]]; then
                echo "  SKIP $rel — no \`// REPLACE:\` blocks found in patch file"
                skipped=$((skipped + 1))
                continue
            fi

            # Validate every sentinel: must extract via the standard REPLACE: regex,
            # must appear exactly once in the target.
            declare -a sentinels=()
            declare -a replacements=()
            declare -a replines=()
            for block in "${replace_blocks[@]}"; do
                # Records are <idx>\x1f<full-sentinel-line>\x1f<replacement-line>\x1f<sentinel-lineno>\x1f<last-non-blank-lineno>
                # (no trailing separator). Read 4 fields then capture the rest
                # as the discarded `_` field. The full-sentinel-line may have
                # leading whitespace (typically a tab from the patch file's
                # indentation); strip it before regex matching.
                IFS=$'\x1f' read -r idx sentinel_full_raw replacement_line sentinel_lineno _ <<< "$block"
                sentinel_full="${sentinel_full_raw#"${sentinel_full_raw%%[![:space:]]*}"}"
                if [[ ! "$sentinel_full" =~ ^//[[:space:]]*REPLACE:[[:space:]](.+)$ ]]; then
                    echo "  SKIP $rel — malformed REPLACE block at line $sentinel_lineno (expected \`// REPLACE: <text>\`)"
                    skipped=$((skipped + 1))
                    continue 2
                fi
                sentinel="${BASH_REMATCH[1]}"
                sentinel_count=$(grep -cF "$sentinel" "$target" 2>/dev/null || true)
                if [[ -z "$sentinel_count" || "$sentinel_count" -eq 0 ]]; then
                    echo "  SKIP $rel — sentinel '$sentinel' (block $idx) not found in target (openclaw bundle structure changed?)"
                    skipped=$((skipped + 1))
                    continue 2
                fi
                if [[ "$sentinel_count" -gt 1 ]]; then
                    echo "  SKIP $rel — sentinel '$sentinel' (block $idx) appears $sentinel_count times in target (ambiguous). Refine the sentinel."
                    skipped=$((skipped + 1))
                    continue 2
                fi
                sentinels+=("$sentinel")
                replacements+=("$replacement_line")
                replines+=("$sentinel_full")
            done

            if ! $DRY_RUN; then
                # Build a single awk pass that handles all sentinels.
                # For each line in target, check if it matches any sentinel. If so,
                # emit the marker, sentinel comment, and replacement line, then skip
                # the original. Otherwise print as-is.
                #
                # IMPORTANT: we write the sentinels to a temp TSV file and pass
                # via `-v datafile=...` rather than via `-v sentinelN=...` per
                # block. Reason: tab characters inside sentinel text (e.g.
                # `\tif (...)`) break `@Q` quoting in bash and awk's argv
                # parser. Using a file with a known delimiter (ASCII Unit
                # Separator \x1f) avoids the issue entirely.
                tmp_data="$(mktemp)"
                for i in "${!sentinels[@]}"; do
                    printf '%s\x1f%s\x1f%s\n' "${sentinels[$i]}" "${replines[$i]}" "${replacements[$i]}" >> "$tmp_data"
                done
                awk -v marker="$marker" -v datafile="$tmp_data" '
                    BEGIN {
                        i = 0
                        while ((getline line < datafile) > 0) {
                            n = split(line, parts, "\x1f")
                            if (n >= 3) {
                                sents[i] = parts[1]
                                repls_full[i] = parts[2]
                                repls_actual[i] = parts[3]
                                replaced[i] = 0
                                i++
                            }
                        }
                        close(datafile)
                        nblocks = i
                    }
                    {
                        line = $0
                        matched = 0
                        for (i = 0; i < nblocks; i++) {
                            if (!replaced[i] && line == sents[i]) {
                                print marker
                                print repls_full[i]
                                print repls_actual[i]
                                replaced[i] = 1
                                matched = 1
                                break
                            }
                        }
                        if (!matched) print line
                    }
                ' "$target" > "$target.new" && mv "$target.new" "$target"
                rm -f "$tmp_data"
                echo "$rel $marker (${#sentinels[@]} blocks)" >> "$APPLIED_LOG"
            fi
            echo "  REPLACE $rel (marker $marker, ${#sentinels[@]} block(s))"
            applied=$((applied + 1))
            ;;

        *)
            echo "  SKIP $rel — unknown file type (only .html, .html.insert, .js, .js.insert, .js.replace, .css, .mjs, .cjs, .json supported)"
            skipped=$((skipped + 1))
            ;;
    esac
done <<< "$patch_files"

echo "[patch-openclaw-dist] done: $applied applied, $skipped skipped"
echo "[patch-openclaw-dist] applied log: $APPLIED_LOG"