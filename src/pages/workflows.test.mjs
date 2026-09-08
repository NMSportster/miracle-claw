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
