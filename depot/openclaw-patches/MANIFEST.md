# OpenClaw dist patches

Each file in this directory (except MANIFEST.md) is applied by
`scripts/patch-openclaw-dist.sh` to the freshly-extracted openclaw dist
in `src-tauri/resources/dist/`. bundle-runtime.sh blows those files
away every time it re-extracts the openclaw tarball, so this is the
only durable home for our edits to vendored openclaw code.

## How to add a new patch

1. Place the patched file under `dist/<subpath>/<filename>`.
   `scripts/patch-openclaw-dist.sh` copies it over
   `src-tauri/resources/<subpath>/<filename>` (full overwrite, content
   compared for idempotency).
2. Document it here with the Lesson number, rc version, and a brief
   rationale.
3. Test the patch with `bash scripts/patch-openclaw-dist.sh --dry-run`.
4. Commit the patch file (and MANIFEST entry). The patch files live in
   `depot/`, which is .gitignored at the project root, but the
   patches ARE tracked. If you find they aren't, fix `.gitignore`
   exception rules — the patches are part of the source tree.

## Index

| Patch | Lesson | rc | Description |
|---|---|---|---|
| `dist/control-ui/mc-back-button.js` | 502/511 | rc53.4 | Adds a "← Dashboard" button to the top-left of the chat UI when hosted in MC's Tauri webview. Detection: `window.location.host === '127.0.0.1:28789'` (Lesson 511 IPC-independent gate). Click: `window.location.href = 'tauri://localhost/index.html'`. Previously inlined as a script block in `index.html` (append-patch); migrated to a separate file in Lesson 978 so the patcher can also wipe it on `bundle-runtime.sh --force` and the `<script src=...>` reference works. |
| `dist/control-ui/mc-voice-native.js` | 1001/1095 | rc57.5 | Chat voice button pivot from SAPI 5 (`mc_voice_native_capture`, dead on this Windows install) to `mc_module_call('mc_voice_transcribe', ...)` -> whisper.cpp sidecar. Auto-checks model, auto-downloads ggml-tiny.en.bin (~75 MB) on first click via friendly toast. Returns {text, duration_ms, transcribe_ms, rms, diagnostics} to JS via dispatcher (envelope unwrapped). Error mapping: NotInstalled|sidecar not found|module 'voice' regex -> "Voice module not installed. Open Settings -> Modules to install it." toast. Lesson 1095 (2026-10-01, David): replaced vague 'No speech detected' overlay with diagnostics-aware copy (peak amplitude + VAD stats disambiguate mic-silent vs VAD-too-strict vs whisper-rejected), classified download errors (HTTP/TLS/DNS/SHA/fs/sidecar), reset voiceReadyCache on failure so next click retries. Replaces Lesson 706 (rc54.3).
| `dist/control-ui/mc-toolbar.js` | 1001/1109 | rc57.7 | Top-right floating toolbar in the OpenClaw chat UI. Attach / Secrets / Modules Run (from Lesson 978). Voice button (was hardcoded to call broken SAPI5 path in Lesson 978) now uses the same `mc_module_call('mc_voice_transcribe', ...)` pivot as mc-voice-native.js above (Lesson 1001), sharing a `ensureVoiceReady()` cached helper. Self-contained IIFE; gates on `location.host === '127.0.0.1:28789'`. Emoji rendered as text ([attach] [key] [mic] [modules]) so the file stays ASCII for the Lesson-998 patch-tool byte-match. Lesson 1109 (2026-10-02, David): fetchInstalledModulesMerged() now also calls mc_module_list (in-memory registry) when MAIC fails or returns empty, so the dropdown never goes blank. Local-only items get an (offline copy) tag in the meta line. Replaces Lesson 975. **rc57.21 (Lesson 1145)**: added `<details class="mc-oc-modal-instructions">` block to `showRunForm()` reading `m._raw.client_manifest.instructions`, 2000-char cap, HTML-escaped. Mirrors the Terminal toolbar's `.mc-modal-instructions` (modules-menu.js, rc57.20, Lesson 1144) for surface parity. Dark-theme CSS in `mc-toolbar.css` because the openclaw chat composer panel is dark.
| `dist/control-ui/mc-toolbar.css` | 978 | rc56.8 | Styles for the chat toolbar. All classes prefixed `mc-oc-` to scope under our toolbar. **rc57.21 (Lesson 1145)**: added `.mc-oc-modal-instructions` dark-theme rule (rgba(255,255,255,0.04) bg, rgba(255,255,255,0.18) border, 💡 ::before glyph on summary, `white-space: pre-line` on `<p>`). |
| `dist/control-ui/index.html.insert` | 978 | rc56.8 | `<script>` + `<link>` tags for the toolbar and the existing `mc-back-button.js`, inserted before `</body>`. Migrated the back-button script tag from append to insert so all control-ui additions share one idempotent path. |
| `dist/stream-BcRkg2P0.js` | 802 | rc55.13 | When neither caller nor model configures a `requestTimeoutMs`, fall back to 60s in `resolveModelRequestTimeoutMs` instead of returning `undefined`. Stops the cosmetic `timeoutMs=undefined` log spam on every chat. |

| Patch | Lesson | rc | Description |
|---|---|---|---|
| `dist/selection-JInn13lc.js.replace` | 1127 | rc57.10 | Extend `OLLAMA_INCOMPLETE_TURN_PROVIDER_ID_PATTERN` from `/^ollama(?:-|$)/` to `/^(?:ollama\|maic)(?:-|$)/` so the embedded-agent retry guards (resolveReasoningOnlyRetryInstruction, resolveEmptyResponseRetryInstruction, isIncompleteTurnRecoverySupportedProviderModel) fire for `maic/*` providers when `stopReason=length` happens. Required `scripts/patch-openclaw-dist.sh` to learn a new `.replace` mode (one-line sed-equivalent via awk: sentinel+replacement, marker-based idempotency). Used by RC-7's MAIC provider; without this, every `stopReason=length` falls through to the "⚠️ Agent couldn't generate a response" error path with zero retries. Replaces Lesson 509 (rc54) on paths the MAIC provider exercises. **v2 (Lesson 1138, rc57.17)**: extended to multi-block `.replace` format. Added a second `// REPLACE:` block that gates the empty-response-retry suppression in `resolveEmptyResponseRetryInstruction` on `assistant.usage && assistant.usage.output === 0` instead of `!hasPositiveOutputTokenUsage(assistant)`. The MAIC proxy does not always populate `usage.output` on the assistant message, so the upstream guard suppressed retries for every MAIC chat with a real answer (causing `stopReason=stop, payloads=0` to fall through to the "⚠️ Agent couldn't generate a response" error). Patch file is now multi-block (2 sentinel+replacement pairs, one shared marker). patch-openclaw-dist.sh learned to parse multi-block `.replace` files (uses ASCII Unit Separator `\x1f` for field delimiter inside each record, newline between records; sentinels written to a temp file and read in awk's BEGIN block to avoid bash `@Q` quoting issues with tab characters in the sentinel text).  **v3 (Lesson 1138 debug, rc57.18-debug)**: added 3 more `// REPLACE:` blocks under a SECOND marker (`// MC-PATCH: maic-empty-retry-debug-v3`) for diagnostic logging: ENTER (function entry with all params + assistant stopReason/usage/phase), GATE1 (shouldSkipNonVisibleTurnRetry), GATE2 (isEmptyResponseAssistantTurn), GATE4 (shouldApplyNonVisibleTurnRetryGuard + isZeroUsageEmptyStopAssistantTurn eval), FINAL (return null). The const-line block bundles ENTER + GATE1 + GATE2 into a single super-long line. The 5-line `if (shouldApplyNonVisibleTurnRetryGuard({...})) || ...` block uses the orphan-absorber pattern: opening line opens `if (false) { const _dbg_orphan = {`, the 4 param lines become part of a never-executed object literal, closing line closes the orphan block and adds GATE4 + FINAL. **Patcher fix (Lesson 1138)**: `scripts/patch-openclaw-dist.sh` was finding the v3 marker `// MC-PATCH: maic-empty-retry-debug-v3` as the "last non-blank line" of block 2, dropping the v2 gate 3 fix. Fix: added `is_mc_marker()` helper to the awk script and skip `// MC-PATCH:` lines when finding the last non-blank line of a REPLACE block. **v3 ROOT CAUSE FOUND (Lesson 1143, rc57.19)**: the v3 debug logs revealed the actual bug. The v2 fix at the second `// REPLACE:` block was equivalent to the original buggy `!hasPositiveOutputTokenUsage(assistant)` check for MAIC (because MAIC's transformed `usage.output` is also `0` for empty responses). The REAL fix is to keep GATE3's safety guard ollama-only (use `^/ollama(?:-|$)/.test(provider)` instead of `OLLAMA_INCOMPLETE_TURN_PROVIDER_ID_PATTERN.test(provider)`). The `shouldApplyNonVisibleTurnRetryGuard` (GATE4) correctly needs MAIC in the pattern so the retry instruction gets returned — but GATE3's safety guard needs to stay ollama-only. The corrected second `// REPLACE:` block in the v2 marker section now uses an inline ollama-only regex. The v3 marker (debug logs) is KEPT in rc57.19 so we can verify the fix produces ENTER + GATE4 EVAL + retry-instruction-returned in production. **REMOVE v3 blocks in rc57.20** once the fix is confirmed working in production. **Lesson 1143 anti-overengineering rules**: (1) Don't add new patch layers when the existing patch is wrong — revert to the upstream behavior, then make a minimal correct change. (2) Don't claim a fix works without running the patched function in isolation with the exact params from production logs. (3) Node isolation testing (`import('./selection-JInn13lc.js').then(mod => mod.I(params))`) is the gold standard for verifying embedded-agent fix correctness before building. |
| `dist/selection-JInn13lc.js.insert` | 1423 | rc57.27 | Recover doubled tool names (`web_searchweb_search`). David saw this verbatim in MC Terminal on the Paid tier: *"I can't use the tool \"web_searchweb_search\" here because it isn't available."* That sentence is generated by `rewriteUnknownToolLoopMessage()` (~line 9977) from the literal name the MODEL emitted — so the model produced the doubled identifier; the app never contained it. **Why it was unrecoverable**: `buildStructuredToolNameCandidates()` (~line 9542) is the only repair path and it splits on `/` and `.`, taking every suffix. A *prefixed* name recovers (`openclaw.web_search` → `web_search`) but a *repeated* name cannot — `web_searchweb_search` has no delimiter, yields a single segment, and no suffix matches. **This patch** is additive only: if `trimmed` is exactly some string concatenated with ITSELF, also try the single half. Flows through the existing allowlist check in `resolveStructuredAllowedToolName()`, so it cannot match anything that isn't already a registered tool. **Ruled out as cause** (verified, do not re-investigate): MAIC dedups by function name server-wins (`api/routes/chat.py:189-195`); Nemotron Super and GLM both echo clean tool lists (`get_weather`/`web_search`/`get_current_time`/`calculate`); maic-plugin's merge is `model.tools \|\| provider.tools \|\| []`, and `\|\|` short-circuits so it can never concatenate. NOT a duplicate-registration problem — that would recreate Lesson 1101's collision (`web_search`/`web_fetch` are working OpenClaw host built-ins). **Safety**: replayed 18 names through before/after — purely additive (new candidates appended, none removed or reordered), fires only on exact self-concatenation (even length, byte-identical halves, not a legal tool name), still allowlist-gated so no name can be invented (`write_fileread_file` stays unknown), empty/whitespace still yields nothing. 1 behavior change, 0 regressions. **Why `.insert` and not `.js.replace`**: `.replace` is strictly one-line-in/one-line-out (awk keeps only the last non-blank line per block), and this fix needs several statements AND must land OUTSIDE the suffix loop — inside it would be dead code, since a doubled name never enters that loop. An earlier `.replace` attempt silently produced that broken result and `node --check` still passed. Sentinel = the `const segments = ...` line opening the suffix loop; unique in the bundle (`grep -cF`). **Naming gotcha**: a patch file must resolve to a real destination bundle — `selection-JInn13lc-toolname.js.insert` resolved to a nonexistent `dist/selection-JInn13lc-toolname.js` and **silently no-oped on a clean build** while the patcher printed success and exited 0. One file per destination; confirm by grepping the GENERATED target for the marker after `--dry-run`, not by trusting patcher output. **OPEN RISK, deliberately unaddressed**: this repairs the name AFTER the model emits it. Root cause of the doubling is still unknown (candidate trigger: Nemotron Super under `thinking=medium` — burns 11-12K reasoning tokens, can return `finish_reason=length`, per memory/2026-10-12). Per David: if the symptom continues after rc57.27, investigate that instead of stacking more name patches. |
| `dist/gateway-CvsJ0gY0.js.replace` | 1424 | rc57.28 | One-flag fix for the `write` tool rejecting empty-string content with "Missing required parameter: content (received: path, content=<empty-string>)". Reproduced in the rc57.27 sidecar log while the model was scaffolding a Python package: `email_assistant/__init__.py` content:"". An empty file is a legitimate request (empty `__init__.py`, placeholder, template stub) — the model was CORRECT, the validator was wrong. **Why this is the right fix, not a workaround**: the upstream framework ALREADY has an `allowEmpty` flag on required-param groups, honored in three places in this same bundle (`assertRequiredParams`, `formatReceivedParamHint`, `describeReceivedParamValue`). It was simply never set for the `write` group. So this is a one-flag opt-in, not new logic. **Blast radius** (10/10 assertions passing against the real patched bundle via `verify-write-empty-content.mjs`): `content:""`/`"\n"`/`"# docstring"` now ACCEPTED; `content` missing/null still REJECTED; `path:""`/missing still REJECTED; `read`+`edit` validators completely unchanged. `path` is deliberately NOT `allowEmpty` — an empty path is never valid. **Companion test**: `depot/openclaw-patches/verify-write-empty-content.mjs` extracts `assertRequiredParams` + `REQUIRED_PARAM_GROUPS` from the real generated bundle and replays the exact production params (`{path:"C:\\Users\\Adeal\\AppData\\Local\\miracle-claw\\workspace\\email_assistant\\__init__.py", content:""}`) plus 9 regression cases. **Sentinel uniqueness**: `grep -cF 'label: "content"' gateway-CvsJ0gY0.js` = 1 (other `gateway-*.js` bundles do not define `write` at all). **Naming gotcha** (carried over from Lesson 1423): always confirm the patch landed by grepping the GENERATED target for the `// MC-PATCH: write-empty-content-allowed-v1` marker after `patch-openclaw-dist.sh`, not by trusting patcher exit code. **Related but separate**: the "Tool loop did not produce an answer" 3-retry failure in the same log is a DIFFERENT bug (Lesson 1423 territory / MAIC `tool_execution: "client"` body) and is NOT addressed here. |

## Lesson 1426 (rc57.28) — "Tool loop did not produce an answer" is MAIC-SERVER-SIDE, not a client patch

**Status**: root-caused, NOT yet fixed. Deliberately no `.replace`/`.insert` patch filed for
this one, because **there is nothing to patch in the client**. Recording it here so the next
person does not spend time grepping MC's bundles.

**Customer symptom** (rc57.27, `chat-nemotron-super`, run `034659fa-5c0b-47d8-9747-39a245290949`):
```
[provider-transport-fetch] response provider=maic status=200 elapsedMs=1579 contentType=text/event-stream
[agent/embedded] embedded run agent end: isError=true error=LLM request failed. rawError=Tool loop did not produce an answer
[agent/embedded] [empty-error-retry] stopReason=error non-visible-output; resubmitting attempt=1/3
... attempt=2/3 ... attempt=3/3
```
Note the shape: **HTTP 200, response in ~1.2s, then the error ~12s later, 3 identical retries.**
Retries changed nothing, because the failure is deterministic, not flaky.

**Where the string comes from**: NOT the client. Verified by exhaustive grep:
- `grep -rl "produce an answer"` over the installed `resources/dist/` → 0 hits
- same over the repo's generated `src-tauri/resources/dist/` → 0 hits
- same over `resources/src/`, `resources/maic-plugin/` → 0 hits
- same over `/home/adeal/.npm-global/lib/node_modules/openclaw/dist/` → 0 hits

It is emitted by the MAIC server:
```
/opt/maic/api/routes/chat.py:1036
    yield f"data: {json.dumps(_structured_error('Tool loop did not produce an answer', 'api_error'))}\n\n".encode()
```

**Root cause** (`chat.py` streaming tool path, ~lines 860–1040):
1. Model streams a tool call → `tool_call_seen = True`, buffered chunks withheld from the user.
2. MC always stamps `params.tool_execution = "client"` (`src-tauri/src/lib.rs:1120`,
   `write_tier_gated_tool_execution_and_tools`) so MC's own file/bash tools are advertised.
3. MAIC hits `if tool_execution == "client":` (~line 886) and calls
   `partition_tool_calls(accumulated_tool_calls)`.
4. **The handoff only fires when `client_calls` is non-empty** (~line 889: `if client_calls:`).
5. When the model calls a *server* tool only (e.g. `web_search`), `client_calls == []`, so the
   handoff is skipped and control **falls through** into the server-side tool loop.
6. That loop runs up to `MAX_TOOL_ITERATIONS = 5`, and only sets `final_answer` when the
   follow-up returns a message with **neither** `tool_calls` **nor** content.
7. `final_answer` stays `None` → the `else:` branch at line ~1035 emits the error string.

**Confirmed empirically** inside the MAIC container:
```
REGISTERED: {'get_weather': 'server', 'web_search': 'server',
             'get_current_time': 'server', 'calculate': 'server'}
write          server=0 client=1
read_file      server=0 client=1
web_search     server=1 client=0   <-- lands in the dead branch
list_dir       server=0 client=1
bash_run       server=0 client=1
```
Every MC local tool partitions to `client` (good — that is the handoff working). But any of
the 4 MAIC server tools partitions to `server`, and if the model's turn contains **only**
those, the request walks into the dead branch and the user gets the error after paying for
a model round-trip.

**The fix belongs in `/opt/maic/api/routes/chat.py`**, not in MC. Two candidate changes:
- **(a) Treat "client mode, but nothing for the client to do" as a normal server loop.**
  Change `if client_calls:` to also proceed when `server_calls` is non-empty — i.e. only
  short-circuit the handoff when there is genuinely at least one client call. Simplest
  correct form: keep the loop, but skip the partition/return unless `client_calls` is non-empty
  (which is what the code already intends) **and** guard the `final_answer is None` case so it
  falls back to a plain non-tool completion instead of erroring.
- **(b) When `final_answer is None` after the loop, make ONE final non-tool completion call**
  with `tools` removed, so the model is forced to answer in prose. This is the robust fix and
  also covers the "model burned all 5 iterations calling tools" case, which is a second,
  independent way to reach the same error.

Recommended: **(b)**, with **(a)** as belt-and-suspenders. Both are server-side; neither
requires an MC release.

**Do NOT** try to fix this from the MC side by flipping `tool_execution` to `"server"`.
That would hide MC's own file/bash tools from the model — the exact regression described in
MEMORY.md Lesson 169 ("the single highest-leverage thing in MAIC integrations"), which
reproduced the "I don't have file tools" refusal. The flag is correct; the MAIC branch is not.

## Lesson 1427 — MilagroCloud publish path + "Exec failed" is a render-time string

**Publish path (found 2026-10-22).** mc-lagroCloud serves the installers
itself — there is no separate CDN or upload script. The route is
`@router.get("/downloads/")` in `/opt/maic/api/routes/install.py`, reading
`_DOWNLOADS_DIR = "/app/dist"` (production). Files are **baked into the
gateway image** by `deploy/Dockerfile` (`COPY dist ./dist`), so publishing is:

```bash
scp <artifact> root@hetzner-prod:/opt/maic/dist/
ssh hetzner-prod 'cd /opt/maic && docker compose build gateway && docker compose up -d gateway'
```

A plain `docker compose restart gateway` does NOT publish — the old bytes
stay in the image. This is the same restart-vs-rebuild trap as Lesson 1428.

**Why the rc57.27 URL 404'd.** Live downloads had never been updated past
rc55.25 (files dated Sep 9 in `/opt/maic/dist/`). rc57.27 was built locally,
verified, installed locally — and never uploaded. Not a routing problem.

**`HEAD` returns 405 on `/downloads/<file>`.** The route only handles GET.
Verify uploads with a ranged GET (`curl -r 0-2047`), not `curl -I`, or you
will think a working file is broken.

**Verify integrity end-to-end.** After publishing, pull the artifact back
through the public URL and compare md5 against the locally verified build.
rc57.28: `a0362ffc6a66340c822423ea792d456f` matched exactly.

## Lesson 1428 — `docker compose restart` does NOT pick up code changes (bit us twice)

`/opt/maic/api/` and `/opt/maic/dist/` are **baked into the gateway image**.
Only these are bind-mounted (verified via `docker inspect --format
"{{range .Mounts}}"`):

- `/opt/maic/static` -> `/app/static`
- steeler `/home/steeler/milagro_ai_cloud/data` -> `/app/data`
- steeler `/home/steeler/milagro_ai_cloud/scripts` -> `/app/scripts`
- `/opt/maic/backups/daily` -> `/app/backups/daily`

So after editing anything under `api/` or `dist/` you MUST
`docker compose build gateway && docker compose up -d gateway`. A `restart`
brings the container back **healthy on the old code** and looks like success.

Always verify inside the running container:

```bash
docker exec milagro_ai_cloud_dev-gateway-1 grep -c "<marker>" /app/api/routes/<file>.py
```

Prod also spans TWO boxes: `data/` and `scripts/` come from **steeler**.
`/opt/maic` is not a git checkout — that is how prod drifted 93 lines ahead
of the repo (reconciled in MAIC commit `b6db39a`).

## Lesson 1429 — AppImage build fails on WSL2: libfuse.so.2 missing

`scripts/build-linux.sh --bundles=appimage` dies with a bare
`failed to run linuxdeploy` (no underlying error). Real cause:

```
dlopen(): error loading libfuse.so.2
AppImages require FUSE to run.
```

WSL2 has `libfuse3` but not FUSE2, and `linuxdeploy-x86_64.AppImage` is an
AppImage itself. Fix WITHOUT sudo (installing `libfuse2` needs a password —
do not prompt, per the 2026-10-16 rule):

```bash
cd /tmp && rm -rf ldtest && mkdir ldtest && cd ldtest
/home/adeal/.cache/tauri/linuxdeploy-x86_64.AppImage --appimage-extract
export PLUGIN_PATH=/tmp/ldtest/squashfs-root/plugins
/tmp/ldtest/squashfs-root/usr/bin/linuxdeploy \
  --appdir <repo>/src-tauri/target/release/bundle/appimage/MiracleClaw.AppDir \
  -o appimage
# writes MiracleClaw-x86_64.AppImage into $PWD
```

Gotchas: `--bundles deb,appimage` (space form) is rejected by the script's
arg parser — use `--bundles=deb,appimage`. The plugin lives at
`squashfs-root/plugins/linuxdeploy-plugin-appimage/` and supplies
`appimagetool`; without `PLUGIN_PATH` it errors
`/usr/bin/appimagetool: No such file`. Output is named
`MiracleClaw-x86_64.AppImage`, so rename to
`MiracleClaw_<version>_amd64.AppImage`. Verify with
`xxd -s 8 -l 2` expecting `4149` ("AI" magic).

`--bundles=rpm` additionally needs the `rpm-build` apt package (sudo) —
skip it unless an .rpm is actually wanted.

## Lesson 1430 — "Exec failed" is generated at render time, not a loggable string

Searching MC source, `openclaw/dist/`, and the installed bundle for the
literal `Exec failed` returns **zero hits**. The `⚠️ Exec failed: \`python .main.py\``
text is composed when a tool result is rendered, from a non-zero exit code.

Consequences:
- Grepping for it will never find an emitter to patch.
- The exec's **stderr is not persisted** to `miracle-claw.log` or the session
  JSONL. MC logs `[tools] <name> failed` only for schema-validation failures
  (e.g. `Missing required parameter: content`).
- So diagnosing an exec failure requires re-running the command by hand.

Real 2026-10-22 case: `python .main.py` (model typo — leading dot) →
`can't open file '...\.main.py': [Errno 2] No such file or directory`, RC 2.
`python main.py stats` worked fine. The `⚠️` label makes a transient model
typo look like a tool outage.

**Opportunity:** log exec stderr on failure. Not yet implemented.
