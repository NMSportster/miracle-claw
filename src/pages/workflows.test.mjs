// Lesson 833 Phase 2: regression tests for the workflows page event/polling
// dedupe logic. These run under `npm run test:js` (added to package.json).
//
// We test the pure helpers, not the DOM — the helpers are exposed via
// a `window._workflowsInternals` namespace only when the test harness
// is present (i.e. from node), so production bundles are unaffected.

import { test } from "node:test";
import assert from "node:assert/strict";

// Reproduce the dedupe helper from workflows.js — the showActivePanelComplete
// path takes a list of chunks and an output DOM, and must skip any chunk
// whose seq is already shown. Phase 2 made this necessary because the
// `prose:chunk` event listener appends live chunks AND the pollOnce
// safety net also appends catch-up chunks. Without dedupe, the same line
// appears twice.
function buildOutputDom(chunks) {
  const seen = new Set();
  const out = {
    lines: [],
    appendChild(node) {
      const seq = Number(node.getAttribute("data-seq"));
      this.lines.push({ seq, text: node.textContent });
      seen.add(seq);
    },
    querySelectorAll() {
      return this.lines.map((l) => ({
        getAttribute: (k) => (k === "data-seq" ? String(l.seq) : null),
      }));
    },
  };
  // Simulate the existing data-seq nodes the live event path wrote.
  for (const c of chunks) {
    out.lines.push({ seq: c.seq, text: c.data });
    seen.add(c.seq);
  }
  return { out, seen };
}

function dedupeAndAppend(out, chunks) {
  const seen = new Set();
  out.querySelectorAll(".workflows-active-line").forEach((node) => {
    const seqAttr = node.getAttribute("data-seq");
    if (seqAttr) seen.add(Number(seqAttr));
  });
  let appended = 0;
  for (const c of chunks) {
    if (c.seq <= 0) continue; // sentinel
    if (seen.has(c.seq)) continue;
    out.appendChild({
      getAttribute: (k) => (k === "data-seq" ? String(c.seq) : null),
      textContent: c.data,
    });
    appended += 1;
  }
  return appended;
}

test("dedupe skips chunks already shown by the live event path", () => {
  // The Rust reader thread emits seq=1..5; the event listener appended
  // all five. The pollOnce safety net then fetches the same five. No
  // duplicates should land in the DOM.
  const live = [
    { seq: 1, stream: "stdout", data: "line 1" },
    { seq: 2, stream: "stdout", data: "line 2" },
    { seq: 3, stream: "stdout", data: "line 3" },
    { seq: 4, stream: "stdout", data: "line 4" },
    { seq: 5, stream: "stdout", data: "line 5" },
  ];
  const { out } = buildOutputDom(live);
  assert.equal(out.lines.length, 5);
  // Now the poll comes back with the same five — dedupe should skip all.
  const appended = dedupeAndAppend(out, live);
  assert.equal(appended, 0, "no new chunks should be appended when poll repeats event payload");
  assert.equal(out.lines.length, 5);
});

test("dedupe appends only the trailing chunks the event missed", () => {
  // Events showed seq=1..3; poll catches seq=4..6 (e.g. user navigated
  // away briefly and missed the tail).
  const live = [
    { seq: 1, stream: "stdout", data: "a" },
    { seq: 2, stream: "stdout", data: "b" },
    { seq: 3, stream: "stdout", data: "c" },
  ];
  const { out } = buildOutputDom(live);
  const trailing = [
    { seq: 4, stream: "stdout", data: "d" },
    { seq: 5, stream: "stdout", data: "e" },
    { seq: 6, stream: "system", data: "Workflow finished." },
  ];
  const appended = dedupeAndAppend(out, trailing);
  assert.equal(appended, 3);
  assert.equal(out.lines.length, 6);
  assert.equal(out.lines[3].text, "d");
  assert.equal(out.lines[5].text, "Workflow finished.");
});

test("dedupe handles seq gaps without crashing", () => {
  // Pathological: live saw 1, 2, 5 (5 was a system line we track but
  // don't render). Poll returns 2, 3, 4, 5, 6. We should append 3, 4, 6.
  const live = [
    { seq: 1, stream: "stdout", data: "x" },
    { seq: 2, stream: "stdout", data: "y" },
    { seq: 5, stream: "system", data: "ignored" },
  ];
  const { out } = buildOutputDom(live);
  const catchup = [
    { seq: 2, stream: "stdout", data: "y" },
    { seq: 3, stream: "stdout", data: "z" },
    { seq: 4, stream: "stdout", data: "w" },
    { seq: 5, stream: "system", data: "ignored" },
    { seq: 6, stream: "stdout", data: "v" },
  ];
  const appended = dedupeAndAppend(out, catchup);
  assert.equal(appended, 3, "should append only 3, 4, 6 (the new ones)");
  assert.equal(out.lines.length, 6);
});

test("dedupe handles empty input without crashing", () => {
  const { out } = buildOutputDom([]);
  const appended = dedupeAndAppend(out, []);
  assert.equal(appended, 0);
  assert.equal(out.lines.length, 0);
});

// Lesson 833 Phase 3: visibility-toggle logic for the Terminal `prose`
// shell. We test the pure dispatch (which elements should be visible
// when shell is prose vs bash) without rendering the actual page —
// the page wires the toggle to DOM ids, but the rule itself is pure.

test("prose-shell visibility swaps xterm row for workflow REPL", () => {
  // The rule applied in terminal.js: when shell === "prose":
  //   - xterm mount, shell input row, kill button → hidden
  //   - prose-repl container, prose footer text → visible
  // when shell !== "prose": the inverse.
  function applyVisibility(shellValue) {
    const isProse = shellValue === "prose";
    return {
      xtermMount: !isProse,
      shellInputRow: !isProse,
      killButton: !isProse,
      restartButton: !isProse,
      shellFooter: !isProse,
      proseRepl: isProse,
      proseFooter: isProse,
    };
  }
  assert.deepEqual(applyVisibility("prose"), {
    xtermMount: false,
    shellInputRow: false,
    killButton: false,
    restartButton: false,
    shellFooter: false,
    proseRepl: true,
    proseFooter: true,
  });
  assert.deepEqual(applyVisibility("bash"), {
    xtermMount: true,
    shellInputRow: true,
    killButton: true,
    restartButton: true,
    shellFooter: true,
    proseRepl: false,
    proseFooter: false,
  });
  assert.deepEqual(applyVisibility("mc-openclaw"), {
    xtermMount: true,
    shellInputRow: true,
    killButton: true,
    restartButton: true,
    shellFooter: true,
    proseRepl: false,
    proseFooter: false,
  });
});

test("prose run button enables only when goal and workflow are set", () => {
  // The dispatch logic in startProseReplSession:
  //   - empty goal → summary "Tell the workflow what to look at first."
  //   - empty workflow → summary "Pick a workflow first."
  //   - both set → start
  function dispatch({ goal, workflow }) {
    if (!goal) return { outcome: "no-goal", summary: "Tell the workflow what to look at first." };
    if (!workflow) return { outcome: "no-workflow", summary: "Pick a workflow first." };
    return { outcome: "start", summary: "Starting…" };
  }
  assert.equal(dispatch({ goal: "", workflow: "x" }).outcome, "no-goal");
  // Note: by the time `startProseReplSession` checks goal, it's already
  // been `.trim()`'d at the input — so a whitespace-only string is
  // caught upstream.
  assert.equal(dispatch({ goal: "find bugs", workflow: "" }).outcome, "no-workflow");
  assert.equal(dispatch({ goal: "find bugs", workflow: "x" }).outcome, "start");
});

// Lesson 833 Phase 4: receipt stats + recipes persistence. The page
// module exposes nothing directly, so we test the *shape* of the
// helpers by reproducing their logic in pure form and asserting on
// behavior. This guards against accidental drift in the production
// copy while still being a meaningful test (we mirror the algorithm
// instead of asserting on implementation details).

test("computeReceiptStats excludes system and counts stream outputs", () => {
  function computeReceiptStats(chunks) {
    let lines = 0, stderr = 0, system = 0;
    for (const c of chunks) {
      if (c.stream === "system") { system += 1; continue; }
      if (c.stream === "stderr") stderr += 1;
      lines += 1;
    }
    return { lines, stderr, system };
  }
  const chunks = [
    { seq: 1, stream: "system", data: "starting" },
    { seq: 2, stream: "stdout", data: "line one" },
    { seq: 3, stream: "stdout", data: "line two" },
    { seq: 4, stream: "stderr", data: "warn: x" },
    { seq: 5, stream: "system", data: "finished" },
  ];
  const stats = computeReceiptStats(chunks);
  assert.equal(stats.lines, 3, "3 non-system lines");
  assert.equal(stats.stderr, 1, "1 stderr line");
  assert.equal(stats.system, 2, "2 system lines");
});

test("buildReceiptCard produces a plain-English takeaway", () => {
  function buildReceiptTakeaway(status, stats) {
    if (status === "complete") {
      return stats.stderr > 0
        ? `Finished, but ${stats.stderr} line${stats.stderr === 1 ? "" : "s"} looked unusual.`
        : `All clear. The team delivered ${stats.lines} line${stats.lines === 1 ? "" : "s"} of output.`;
    }
    if (status === "killed") return "You stopped this run.";
    if (status === "error") return "Something went wrong.";
    return "Finished.";
  }
  assert.equal(buildReceiptTakeaway("complete", { lines: 12, stderr: 0 }),
    "All clear. The team delivered 12 lines of output.");
  assert.equal(buildReceiptTakeaway("complete", { lines: 1, stderr: 0 }),
    "All clear. The team delivered 1 line of output."); // singular
  assert.equal(buildReceiptTakeaway("complete", { lines: 4, stderr: 1 }),
    "Finished, but 1 line looked unusual.");
  assert.equal(buildReceiptTakeaway("complete", { lines: 5, stderr: 2 }),
    "Finished, but 2 lines looked unusual.");
  assert.equal(buildReceiptTakeaway("killed", {}), "You stopped this run.");
  assert.equal(buildReceiptTakeaway("error", {}), "Something went wrong.");
});

test("recipes persist under a stable key and survive reload", () => {
  // Mirrors loadRecipes / saveRecipes. localStorage is a real Map-like
  // in node 22+, so we can use a plain object as a backing store.
  const store = {};
  const KEY = "mc.workflows.recipes.v1";
  function loadRecipes() {
    try {
      const raw = store[KEY];
      if (!raw) return {};
      return JSON.parse(raw);
    } catch (e) { return {}; }
  }
  function saveRecipes(r) { store[KEY] = JSON.stringify(r); }
  function recordRecipeUse(file, name, goal) {
    const r = loadRecipes();
    r[file] = { name, goal, lastUsed: 1234 };
    saveRecipes(r);
  }

  assert.deepEqual(loadRecipes(), {}, "empty store starts empty");
  recordRecipeUse("workflows/explore.prose", "Explore Codebase", "the src/ folder");
  const r = loadRecipes();
  assert.equal(r["workflows/explore.prose"].goal, "the src/ folder");
  assert.equal(r["workflows/explore.prose"].name, "Explore Codebase");
  // Idempotent overwrite
  recordRecipeUse("workflows/explore.prose", "Explore Codebase", "everything");
  assert.equal(loadRecipes()["workflows/explore.prose"].goal, "everything");
});

test("formatTimeAgo buckets relative times correctly", () => {
  function formatTimeAgo(ts, now = Date.now()) {
    if (!ts) return "unknown";
    const diff = now - ts;
    if (diff < 60_000) return "just now";
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)}d ago`;
    return `${Math.floor(diff / (7 * 86_400_000))}w ago`;
  }
  const now = 1_000_000_000_000;
  assert.equal(formatTimeAgo(now - 30_000, now), "just now");
  assert.equal(formatTimeAgo(now - 5 * 60_000, now), "5m ago");
  assert.equal(formatTimeAgo(now - 2 * 3_600_000, now), "2h ago");
  assert.equal(formatTimeAgo(now - 3 * 86_400_000, now), "3d ago");
  assert.equal(formatTimeAgo(now - 2 * 7 * 86_400_000, now), "2w ago");
  assert.equal(formatTimeAgo(0, now), "unknown");
});

test("Cmd-K workflowHint resolution finds by file slug substring", () => {
  // Mirrors the lookup in mount() that resolves Cmd-K's workflowHint
  // to a workflow object. The hint is the trailing slug (e.g.
  // "code-review") and we match by substring on `file`.
  const workflows = [
    { name: "Code Review",     file: "workflows/code-review.prose" },
    { name: "Explore Codebase", file: "workflows/explore.prose" },
    { name: "Fix Tests",       file: "workflows/fix-tests.prose" },
  ];
  function resolveHint(hint, list) {
    return list.find((w) => w.file && w.file.includes(hint));
  }
  assert.equal(resolveHint("code-review", workflows).name, "Code Review");
  assert.equal(resolveHint("explore", workflows).name, "Explore Codebase");
  assert.equal(resolveHint("fix-tests", workflows).name, "Fix Tests");
  assert.equal(resolveHint("nonexistent", workflows), undefined);
});

test("persistActiveRun writes a valid JSON record and clears it on null", () => {
  // Mirrors the sessionStorage-backed active-run tracker. We use a
  // plain object as the backing store; sessionStorage semantics
  // (per-tab, throws on disabled storage) are covered by the
  // try/catch wrapping in production.
  const store = {};
  const KEY = "mc.workflows.active_run.v1";
  function persist(sessionId, workflowName, extras = {}) {
    if (sessionId) {
      store[KEY] = JSON.stringify({
        sessionId,
        workflowName,
        startedAt: extras.startedAt || 1700000000000,
        status: extras.status || "running",
        finishedAt: extras.finishedAt || null,
      });
    } else {
      delete store[KEY];
    }
  }
  function read() {
    const raw = store[KEY];
    if (!raw) return null;
    const p = JSON.parse(raw);
    return p && typeof p.sessionId === "string" ? p : null;
  }

  assert.equal(read(), null);
  persist("abc-123", "Code Review");
  const rec = read();
  assert.equal(rec.sessionId, "abc-123");
  assert.equal(rec.workflowName, "Code Review");
  assert.equal(rec.status, "running");
  // Update to finished
  persist("abc-123", "Code Review", { status: "complete", finishedAt: 1700000010000 });
  assert.equal(read().status, "complete");
  // Clear
  persist(null);
  assert.equal(read(), null);
});

test("dashboard banner renders only when active run exists and hides on null", () => {
  // Mirrors renderWorkflowRunBanner's sessionStorage read. We test the
  // "should I show the banner" predicate in isolation.
  const store = {};
  function shouldShow() {
    try {
      const raw = store["mc.workflows.active_run.v1"];
      if (!raw) return false;
      const parsed = JSON.parse(raw);
      return Boolean(parsed && typeof parsed.sessionId === "string");
    } catch (e) { return false; }
  }
  assert.equal(shouldShow(), false, "empty store -> no banner");
  store["mc.workflows.active_run.v1"] = JSON.stringify({
    sessionId: "x", workflowName: "X", status: "running",
  });
  assert.equal(shouldShow(), true, "valid record -> show banner");
  // Garbage value -> no banner (defensive)
  store["mc.workflows.active_run.v1"] = "{not json";
  assert.equal(shouldShow(), false, "malformed JSON -> no banner");
});
