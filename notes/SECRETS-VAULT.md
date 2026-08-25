# Local Encrypted Secrets Vault — Design Doc (rc53+)

## Why

Today, AI coders paste API keys into ChatGPT/Claude/MAIC chat to do work. **Those keys then sit in conversation history, training data, and tool logs forever.** Anyone with access to the conversation (or the model provider) has the key.

The "secure" alternatives are friction-heavy: paste the key into a `.env` file, add it to gitignore, remember to unset it. Most people go with the lazy option and leak the key.

MC's vault makes the safe option the easy option: store once, reference by `$NAME` in chat, plaintext never leaves the machine, never appears in conversation history.

## Threat model (locked-in v1)

| Property | How we ensure it |
|---|---|
| Secret never leaves the machine | No network code path touches plaintext |
| Secret never reaches MAIC servers | Chat preprocessor replaces `$NAME` → `<<secret:NAME>>` before `/v1/chat/completions` call |
| Secret never in conversation history | History stores placeholder, not plaintext |
| Secret never in tool args sent to MAIC | bash_run expands placeholders locally at exec time |
| Secret never in MC logs | Log scrubber strips placeholder + plaintext allowlist |
| Per-install encryption | Master key derived from passphrase + machine ID + salt |
| Strong passphrase enforced | Min 12 chars, upper + lower + digit |
| Local admin compromise | Out of scope (mitigated by full-disk encryption) |

## What the model sees

**Input** (user typed in MC):
> "Fix the deploy script. Use `$GITHUB_TOKEN` for the git push."

**Sent to MAIC** (after preprocessor):
> "Fix the deploy script. Use `<<secret:github_token>>` for the git push."

**Model's tool call** (bash_run):
```
git push https://x-access-token:<<secret:github_token>>@github.com/me/repo.git
```

**MC's bash_run expander** (exec time, never logged with plaintext):
```
git push https://x-access-token:ghp_actualRealTokenHere1234@github.com/me/repo.git
```

**Model's response** (after seeing result):
> "Pushed to main. Used $GITHUB_TOKEN."

The model NEVER sees `ghp_actualRealTokenHere1234`. MAIC NEVER sees it. Logs NEVER have it.

## v0 scope (rc53 — first build, no crypto)

To validate the architecture before adding encryption:
1. Plaintext JSON vault at `<MC_DATA>/secrets.json`
2. Three Tauri commands: `mc_secret_list`, `mc_secret_get`, `mc_secret_set`, `mc_secret_delete`
3. Chat preprocessor: scans input for `$[A-Z_][A-Z0-9_]*`, replaces with `<<secret:NAME>>`
4. bash_run expander: scans command for `<<secret:NAME>>`, replaces with stored value at exec time
5. Manual seeding: user can `mc_secret_set("STRIPE_KEY", "sk_live_...")` via a debug command
6. NO UI (just verify the wiring works via test prompt)

**Why v0 first**: Crypto bugs are the worst kind to ship. Verify the data flow works, THEN add encryption in v1. If v0 leaks, we lose nothing (plaintext was always the baseline). If v1 leaks because we rushed the design, we lose real keys.

## v1 scope (rc54 — encrypted vault + UI)

1. AES-256-GCM with PBKDF2(passphrase + machine_id, salt, 100k iters)
2. Per-install encryption (moving to new machine = re-enter secrets)
3. Dashboard tile: 🔑 Secrets (list, add, edit, delete with masked reveal)
4. Toolbar buttons in Terminal page + OpenClaw webview (via Tauri script injection)
5. First-run tip: "💡 Tip: Store API keys here so the agent can use them safely"
6. Export/import for backup (deferred to v2)

## Naming convention (locked)

`$[A-Z_][A-Z0-9_]*` — matches shell variable syntax. Why this matters:
- What coders actually type in commands
- Prevents shell-injection via a name like `$PATH; rm -rf /` (which IS valid as a name per naive checks)
- Reads naturally in conversation: "Use $STRIPE_KEY for the API call"

Multi-line secrets (PEM keys, JSON blobs) deferred to v2. v1 is single-line only.

## Tool integration (v1: bash_run only)

Why bash_run first: it's the highest-value target (coders use it for `curl`, `git push`, `psql`, `aws`, etc.). Other tools (read_file, write_file, fetch) deferred to v2.

bash_run command string is scanned for `<<secret:NAME>>` tokens immediately before exec. Each is expanded in-memory. The expanded command is passed to the OS shell. After exec returns, the plaintext expansion is wiped from MC's local memory.

## System prompt for the model

Added once per session (server-side or client-side, TBD):

```
The user has stored some secrets locally. When you see `<<secret:NAME>>`
in tool results, that's a reference to a user-stored secret you can use
via bash_run. NEVER echo the literal value. NEVER write it to a file
visible to the user. When you use one, briefly acknowledge ("Used
$NAME.") and move on.
```

## Open questions for David

1. **Auto-add on first detection?** If user types `$STRIPE_KEY` and it's not in vault, MC could prompt "Add $STRIPE_KEY to your vault?" with a paste field. Useful UX or annoying nag? Defer to v2.
2. **Per-session vs persistent unlock?** Vault unlocks on login, locks on logout. Or unlock per-window-focus with auto-lock after N min idle? v1 = login-based.
3. **Audit log?** Track every secret use (timestamp, which model session, which tool call). Privacy tradeoff: helpful for "did anyone access my keys?", but also a target. v1 = no audit log, v2 = optional.
4. **Sharing across machines?** v1 = single-machine. v2 = encrypted export with passphrase (separate from vault passphrase) for backup/transfer.
