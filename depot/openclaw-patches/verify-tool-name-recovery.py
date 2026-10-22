#!/usr/bin/env python3
"""
Verify the Lesson 1423 doubled-tool-name recovery patch.

Two things get checked, because the patch has two ways to be broken:

  1. REPRODUCIBILITY. A patch file that doesn't resolve to a real destination
     bundle makes patch-openclaw-dist.sh print a line and exit 0 while doing
     nothing (the bug the MANIFEST entry now documents). So: strip the
     generated target back to an unpatched state, run the patcher, and
     confirm the marker actually landed. Never trust patcher output alone.

  2. BEHAVIOR. Extract the real candidate-builder from the rebuilt bundle
     together with its transitive string/tool-policy helpers, and replay a
     name suite through it. The negative cases matter as much as the fix:
     the patch must stay gated by the allowlist so it cannot invent a tool.

Usage:  python3 verify-tool-name-recovery.py
Requires: node on PATH. Read-only except for the patcher run.
"""
import os
import re
import subprocess
import sys

# This file lives at depot/openclaw-patches/, so the repo root is two levels up.
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DIST = os.path.join(REPO, "src-tauri", "resources", "dist")
TARGET = os.path.join(DIST, "selection-JInn13lc.js")
PATCHER = os.path.join(REPO, "scripts", "patch-openclaw-dist.sh")
MARKER = "// MC-PATCH: maic-doubled-tool-name-recovery-v1"

# Resolved against the real bundle's allowlist in the JS suite below.
WANTED = [
    "normalizeStringEntries",
    "normalizeOptionalString",
    "normalizeOptionalLowercaseString",
    "normalizeLowercaseStringOrEmpty",
    "normalizeToolName",
    "buildStructuredToolNameCandidates",
]


def grab_fn(src, name):
    """Pull a top-level `function name(...) { ... }` out of minified JS."""
    m = re.search(r"function " + re.escape(name) + r"\(", src)
    if not m:
        return None
    j = src.find("{", m.start())
    depth = 0
    for k in range(j, len(src)):
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                return src[m.start(): k + 1]
    return None


SENTINEL_STARTSWITH = "const segments = normalizeStringEntries(normalizedDelimiter.split"


def strip_patch(path):
    """Return the target's contents with the recovery patch removed.

    Pure: reads and returns, never writes. The caller is responsible for
    writing the result back. (Doing the write inside here truncates the
    file before the read completes, because the caller is about to open
    the same path for writing.)

    The sentinel text also appears inside this patch's own `// INSERT-BEFORE:`
    comment line, so match on the line STARTING with the sentinel. A substring
    match hits that comment first and deletes the wrong region -- which makes
    the patcher report "sentinel not found in target" on the next run.
    """
    lines = open(path, encoding="utf-8").read().split("\n")
    out, i = [], 0
    while i < len(lines):
        if lines[i].startswith(MARKER):
            # drop marker + following comment/blank lines
            while i < len(lines) and (lines[i].startswith("//") or not lines[i].strip()):
                i += 1
            # drop inserted statements up to the untouched sentinel line
            while i < len(lines) and not lines[i].lstrip().startswith(SENTINEL_STARTSWITH):
                i += 1
            continue
        out.append(lines[i])
        i += 1
    return "\n".join(out)


def collect_helpers():
    """Gather WANTED functions from wherever they live in the dist tree."""
    found = {}
    for fname in os.listdir(DIST):
        if not fname.endswith(".js"):
            continue
        src = open(os.path.join(DIST, fname), encoding="utf-8").read()
        for want in WANTED:
            if want in found:
                continue
            body = grab_fn(src, want)
            if body:
                found[want] = body
    missing = [w for w in WANTED if w not in found]
    if missing:
        sys.exit(f"FATAL: could not extract {missing} from the dist tree — "
                 "the bundle layout changed; update WANTED.")
    return found


def aliases_const():
    tp = open(os.path.join(DIST, "tool-policy-BHUGxE3p.js"), encoding="utf-8").read()
    m = re.search(r"TOOL_NAME_ALIASES\s*=\s*\{", tp)
    if not m:
        return "const TOOL_NAME_ALIASES = {};"
    j = tp.find("{", m.start())
    depth = 0
    for k in range(j, len(tp)):
        if tp[k] == "{":
            depth += 1
        elif tp[k] == "}":
            depth -= 1
            if depth == 0:
                return "const TOOL_NAME_ALIASES = " + tp[j:k + 1] + ";"
    return "const TOOL_NAME_ALIASES = {};"


SUITE_JS = r"""
const ALLOWED = new Set([
  "web_search", "web_fetch", "read_file", "write_file", "edit_file",
  "list_dir", "bash_run", "apply_patch", "remember_fact",
]);
const { buildStructuredToolNameCandidates: C } = await import(MODULE_URL);
const cases = [
  ["web_search", true],
  ["functions.web_search", true],
  ["openclaw.web_search", true],
  ["web_searchweb_search", true],   // THE FIX (Lesson 1423)
  ["read_fileread_file", true],
  ["write_fileread_file", false],   // halves differ -> must NOT resolve
  ["edit_fileedit_file", true],
  ["web_fetchweb_fetch", true],
  ["", false],
  ["   ", false],
  ["functions.not_a_tool", false],
  ["bash_runbash_run", true],
  ["web_searchweb", false],         // odd length, not exact self-concat
  ["apply_patchapply_patch", true],
  ["remember_factremember_fact", true],
  ["list_dirlist_dir", true],
];
let pass = 0, fail = 0;
for (const [input, shouldResolve] of cases) {
  const cands = C(input);
  const resolved = cands.some((c) => ALLOWED.has(c));
  const ok = resolved === shouldResolve;
  if (ok) pass++; else fail++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${JSON.stringify(input).padEnd(32)} -> ` +
              `${JSON.stringify(cands).slice(0, 72)}  resolved=${resolved}`);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
"""


def write_target(text):
    """Write via a temp file + atomic replace, never a truncating open()."""
    tmp = TARGET + ".verify-tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.replace(tmp, TARGET)


def main():
    original = open(TARGET, encoding="utf-8").read()
    if not original.strip():
        sys.exit("FATAL: target is empty before the run -- refusing to touch it. "
                 "If a previous run truncated it, restore from a clean "
                 "bundle-runtime.sh extraction.")

    try:
        print("[1/3] stripping the generated target back to an unpatched state")
        stripped = strip_patch(TARGET)
        if MARKER in stripped:
            sys.exit("FATAL: strip_patch did not remove the marker")
        if SENTINEL_STARTSWITH not in stripped:
            sys.exit("FATAL: strip_patch removed too much -- the insert sentinel "
                     "is gone, so a re-apply could never succeed. Refusing to write.")
        write_target(stripped)

        print("[2/3] running patch-openclaw-dist.sh on the clean target")
        subprocess.run(["bash", PATCHER], cwd=REPO, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        rebuilt = open(TARGET, encoding="utf-8").read()
        if MARKER not in rebuilt:
            sys.exit("FAIL: patcher did not re-apply the recovery patch. "
                     "Check that the patch filename resolves to a real destination bundle.")
        print("      marker re-applied by patcher: OK")
        subprocess.run(["node", "--check", TARGET], check=True)
        print("      node --check on rebuilt bundle: OK")

        print("[3/3] replaying the name suite against the rebuilt bundle")
        helpers = collect_helpers()
        module = "\n".join(helpers[w] for w in WANTED) + "\n" + aliases_const() + \
                 "\nexport { buildStructuredToolNameCandidates };"
        mod_path = "/tmp/fn-cand-verify.mjs"
        open(mod_path, "w", encoding="utf-8").write(module)
        suite_path = "/tmp/suite-verify.mjs"
        open(suite_path, "w", encoding="utf-8").write(
            SUITE_JS.replace("MODULE_URL", mod_path))
        rc = subprocess.run(["node", suite_path]).returncode
        if rc != 0:
            sys.exit("FAIL: name suite failed")
    finally:
        # Always hand back a file that at minimum parses, whatever happened.
        current = open(TARGET, encoding="utf-8").read() if os.path.exists(TARGET) else ""
        if not current.strip():
            write_target(original)
            print("\n[restore] target was empty; restored the original")

    print("\nALL CHECKS PASS — patch reproduces from clean and behaves correctly")


if __name__ == "__main__":
    main()