// preprocessor.test.mjs — unit tests for the JS preprocessor.
//
// Run: node src/secrets/preprocessor.test.mjs
//
// v0: tests the pure-JS functions (findSecretReferences, rewriteMessage).
// The Tauri-side expand is tested in Rust.

import {
  findSecretReferences,
  rewriteMessage,
  sanitizeForLog,
} from "./rewrite.js";

let passed = 0;
let failed = 0;
const failures = [];

function assertEq(actual, expected, label) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    passed++;
  } else {
    failed++;
    failures.push(`${label}: expected ${e}, got ${a}`);
  }
}

// --- findSecretReferences ---

assertEq(
  findSecretReferences("hello $STRIPE_KEY world"),
  [{ name: "STRIPE_KEY", start: 6, end: 17 }],
  "finds single $STRIPE_KEY"
);

assertEq(
  findSecretReferences("use $A and $B for testing"),
  [
    { name: "A", start: 4, end: 6 },
    { name: "B", start: 11, end: 13 },
  ],
  "finds two references"
);

assertEq(
  findSecretReferences("price: $$5.00 (literal $)"),
  [],
  "$$ is shell escape, not a ref"
);

assertEq(
  findSecretReferences("no refs here at all"),
  [],
  "no refs returns empty array"
);

assertEq(
  findSecretReferences("end with $ at the end $"),
  [],
  "trailing $ with no name returns nothing"
);

assertEq(
  findSecretReferences("$1NUM can't start with digit"),
  [],
  "digit-start name is rejected"
);

assertEq(
  findSecretReferences("$lowercase rejected"),
  [],
  "lowercase start is rejected (shell var rules)"
);

assertEq(
  findSecretReferences("$_UNDERSCORE_OK and $ALSO_OK_123"),
  [
    { name: "_UNDERSCORE_OK", start: 0, end: 15 },
    { name: "ALSO_OK_123", start: 20, end: 32 },
  ],
  "underscore start and digits in name are OK"
);

assertEq(
  findSecretReferences("$LONG_NAME is fine"),
  [{ name: "LONG_NAME", start: 0, end: 10 }],
  "long name with underscores is found"
);

// --- rewriteMessage ---

assertEq(
  rewriteMessage("use $STRIPE_KEY here").processed,
  "use <<secret:STRIPE_KEY>> here",
  "rewrites $STRIPE_KEY"
);

assertEq(
  rewriteMessage("use $A and $B").processed,
  "use <<secret:A>> and <<secret:B>>",
  "rewrites multiple refs"
);

assertEq(
  rewriteMessage("no refs here").processed,
  "no refs here",
  "no refs returns input unchanged"
);

// $$5 is the shell escape $$ followed by literal 5 (the 5 is just
// text, not a name). $NOT_VALID IS a valid name (all uppercase, no
// special chars), so it IS rewritten. To test "trailing $ with no
// name" we need a $ at end of string with no name character after.
assertEq(
  rewriteMessage("price $$5 stays $NOT_VALID").processed,
  "price $$5 stays <<secret:NOT_VALID>>",
  "$$5 not ref, $NOT_VALID IS ref (valid shell-var name)"
);

assertEq(
  rewriteMessage("$A then $A again").processed,
  "<<secret:A>> then <<secret:A>> again",
  "same ref twice is rewritten twice"
);

assertEq(
  rewriteMessage("use $A here").referenced,
  ["A"],
  "referenced list contains unique names"
);

assertEq(
  rewriteMessage("$A and $B and $A again").referenced,
  ["A", "B"],
  "referenced list is deduped, preserves order"
);

// --- sanitizeForLog ---

assertEq(
  sanitizeForLog("command: <<secret:STRIPE_KEY>>"),
  "command: [SECRET:STRIPE_KEY]",
  "sanitize replaces placeholder"
);

assertEq(
  sanitizeForLog("plain text no secrets"),
  "plain text no secrets",
  "sanitize leaves normal text alone"
);

assertEq(
  sanitizeForLog("<<secret:FOO>> and <<secret:BAR>>"),
  "[SECRET:FOO] and [SECRET:BAR]",
  "sanitize replaces multiple"
);

// --- summary ---

console.log(`\n${passed + failed} tests: ${passed} passed, ${failed} failed`);
if (failed > 0) {
  console.log("\nFAILURES:");
  for (const f of failures) console.log("  - " + f);
  process.exit(1);
}
console.log("OK");
