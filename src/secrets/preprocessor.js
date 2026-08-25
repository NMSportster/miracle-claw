// secrets/preprocessor.js — Client-side secret reference rewriter.
//
// PURPOSE: When the user types `$STRIPE_KEY` in any chat input, this
// module rewrites it to `<<secret:STRIPE_KEY>>` BEFORE the message
// hits MAIC's /v1/chat/completions endpoint. The placeholder flows
// through MAIC, the model sees it, and any tool call the model makes
// (e.g. `bash_run`) has the placeholder in its arguments.
//
// On the Rust side, `mc_secret_expand` is called RIGHT BEFORE tool
// exec. It replaces `<<secret:NAME>>` with the real plaintext value.
// The plaintext is never seen by MAIC, never logged, never stored in
// conversation history.
//
// USAGE:
//   import { preprocessMessage } from "./secrets/preprocessor.js";
//   const { processed, referenced } = preprocessMessage(userInput);
//   send to MAIC: processed
//
// THREAT MODEL (locked-in, see notes/SECRETS-VAULT.md):
//   - Plaintext NEVER reaches MAIC. The JS preprocessor is the only
//     place that touches plaintext, and it scrubs it before any
//     network call.
//   - If the vault doesn't have a referenced secret, the `$NAME` is
//     left in the input. Model sees "user typed $X but X is not in
//     vault" and can prompt the user to add it.
//
// VALIDATION:
//   - $NAME must match [A-Z_][A-Z0-9_]* (shell-var rules).
//   - $ is treated as a shell-style reference ONLY when followed by
//     a valid name. Plain $ (e.g. in a regex or shell command) is
//     left alone.
//   - $$ (escaped dollar, shell convention) is left alone.
//
// This is v0: trusts the JS preprocessor to scrub before sending.
// Future hardening (rc5x): also scrub at the Rust IPC layer as a
// second line of defense (in case a future JS bug leaks plaintext).

import { invoke } from "@tauri-apps/api/core";
import {
  findSecretReferences,
  rewriteMessage,
  sanitizeForLog,
} from "./rewrite.js";

// Re-export pure functions for callers that want them.
export { findSecretReferences, rewriteMessage, sanitizeForLog };

/**
 * High-level entry point: take a user message, rewrite it, and
 * return the version safe to send to MAIC. v0 entry point.
 */
export function preprocessMessage(text) {
  return rewriteMessage(text);
}

/**
 * Expand a string with `<<secret:NAME>>` placeholders to real
 * values via the Rust side. Called by tools right before exec.
 *
 * NOT used on the chat message — that gets the placeholder
 * version. This is only for tool exec paths.
 */
export async function expandPlaceholders(text) {
  if (!text.includes("<<secret:")) {
    return { expanded: text, used: [] };
  }
  const expanded = await invoke("mc_secret_expand", { input: text });
  const used = [];
  const re = /<<secret:([A-Z_][A-Z0-9_]*)>>/g;
  let m;
  while ((m = re.exec(text)) !== null) {
    if (!used.includes(m[1])) used.push(m[1]);
  }
  return { expanded, used };
}
