// secrets/rewrite.js — Pure string rewriting for secret references.
//
// No Tauri imports. Safe to import from any JS context, including
// node tests. The Tauri-aware preprocessor.js wraps this module
// and adds the IPC call to expand placeholders at exec time.
//
// See preprocessor.js for the high-level usage. This module is
// split out so the pure functions are unit-testable without
// mocking Tauri.

/**
 * Find all `$NAME` references in text where NAME matches the
 * shell-var rules: starts with A-Z or underscore, followed by
 * A-Z, 0-9, or underscore.
 *
 * Returns an array of { name, start, end } in source-order. Indices
 * are character positions in the input string (NOT byte offsets —
 * JS strings are UTF-16 so they're the same for ASCII names).
 */
export function findSecretReferences(text) {
  const refs = [];
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (ch !== "$") {
      i++;
      continue;
    }
    // Skip $$ (shell escape for literal $)
    if (text[i + 1] === "$") {
      i += 2;
      continue;
    }
    // Skip $ at end of string
    if (i + 1 >= text.length) {
      i++;
      continue;
    }
    // Check first char of name: must be A-Z or _
    const first = text[i + 1];
    if (!isNameStart(first)) {
      i++;
      continue;
    }
    // Find end of name (greedy: longest valid match)
    let j = i + 2;
    while (j < text.length && isNameCont(text[j])) {
      j++;
    }
    const name = text.slice(i + 1, j);
    refs.push({ name, start: i, end: j });
    i = j;
  }
  return refs;
}

function isNameStart(c) {
  return (c >= "A" && c <= "Z") || c === "_";
}

function isNameCont(c) {
  return (c >= "A" && c <= "Z") || (c >= "0" && c <= "9") || c === "_";
}

/**
 * Rewrite a message, replacing `$NAME` references with
 * `<<secret:NAME>>` placeholders. Returns the rewritten string
 * and a list of names that were referenced (deduped, source order).
 *
 * Does NOT throw if a referenced name is not in the vault — the
 * $NAME is left in place. The caller (Rust expander) handles the
 * "unknown name" case at exec time.
 */
export function rewriteMessage(text) {
  const refs = findSecretReferences(text);
  if (refs.length === 0) {
    return { processed: text, referenced: [] };
  }
  const names = [];
  let out = "";
  let cursor = 0;
  for (const ref of refs) {
    out += text.slice(cursor, ref.start);
    // Preserve the case the user typed. Vault validation requires
    // uppercase + underscore per shell-var rules.
    out += `<<secret:${ref.name}>>`;
    cursor = ref.end;
    if (!names.includes(ref.name)) names.push(ref.name);
  }
  out += text.slice(cursor);
  return { processed: out, referenced: names };
}

/**
 * Sanitize a string for logging. Replaces `<<secret:NAME>>`
 * placeholders with `[SECRET:NAME]` so logs show that a secret
 * was referenced but not the placeholder format (which MAIC may
 * treat as syntax).
 *
 * Defensive: this should NEVER see plaintext because the
 * preprocessor should have scrubbed it. If it does see plaintext
 * (bug), we still don't log it.
 */
export function sanitizeForLog(text) {
  return text.replace(/<<secret:([A-Z_][A-Z0-9_]*)>>/g, "[SECRET:$1]");
}
