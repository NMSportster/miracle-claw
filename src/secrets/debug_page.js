// secrets/debug_page.js — v0 test page for the secrets vault.
//
// NOT in the main page registry. Mounted only via direct URL or
// a debug flag. Lets David test:
//   1. Set a secret
//   2. List secrets (shows length, not value)
//   3. Delete a secret
//   4. Preprocess a message containing $NAME refs
//   5. Expand <<secret:NAME>> placeholders via Rust
//
// Used to verify the round-trip works before we wire it into the
// OpenClaw webview chat input (v1).
//
// v0 has NO UI polish. This is a developer-facing test page.

import { invoke } from "@tauri-apps/api/core";
import {
  preprocessMessage,
  expandPlaceholders,
  sanitizeForLog,
} from "./preprocessor.js";

export const secretsDebugPage = {
  label: "Secrets Debug",
  icon: "🔑",
  requiresAuth: true, // dev-only, but gate it anyway
  mount(root, ctx) {
      root.innerHTML = `
        <div class="secrets-debug">
          <h1>🔑 Secrets Vault — Debug (rc53 v0)</h1>
          <p class="warn">
            <strong>v0 — plaintext JSON vault, no encryption.</strong>
            Threat model: secrets never reach MAIC, but they're stored
            in plaintext on disk. v1 (rc54) adds AES-256-GCM.
          </p>

          <section>
            <h2>1. Add a secret</h2>
            <input id="sec-name" placeholder="NAME (e.g. STRIPE_KEY)" />
            <input id="sec-value" type="password" placeholder="value" />
            <button id="sec-set">Set</button>
            <pre id="sec-set-result" class="result"></pre>
          </section>

          <section>
            <h2>2. List secrets</h2>
            <button id="sec-list">List</button>
            <pre id="sec-list-result" class="result"></pre>
          </section>

          <section>
            <h2>3. Delete a secret</h2>
            <input id="sec-del-name" placeholder="NAME to delete" />
            <button id="sec-del">Delete</button>
            <pre id="sec-del-result" class="result"></pre>
          </section>

          <section>
            <h2>4. Preprocess a message (JS)</h2>
            <p>Type a message with $NAME references. See the rewritten version below.</p>
            <textarea id="preprocess-input" rows="3" placeholder="e.g. Use $STRIPE_KEY to test the API"></textarea>
            <button id="preprocess-run">Preprocess</button>
            <pre id="preprocess-result" class="result"></pre>
          </section>

          <section>
            <h2>5. Expand placeholders (Rust)</h2>
            <p>Type a string with &lt;&lt;secret:NAME&gt;&gt; placeholders. Rust expands to real values.</p>
            <textarea id="expand-input" rows="3" placeholder="e.g. export TOKEN=<<secret:stripe_key>>"></textarea>
            <button id="expand-run">Expand</button>
            <pre id="expand-result" class="result"></pre>
          </section>

          <section>
            <h2>6. Raw vault dump (debug only)</h2>
            <button id="dump-run">Dump</button>
            <pre id="dump-result" class="result"></pre>
          </section>

          <p>
            <button id="back" class="secondary">← Back to dashboard</button>
          </p>
        </div>
      `;

      // --- 1. Set ---
      root.querySelector("#sec-set").addEventListener("click", async () => {
        const name = root.querySelector("#sec-name").value.trim();
        const value = root.querySelector("#sec-value").value;
        const out = root.querySelector("#sec-set-result");
        if (!name) {
          out.textContent = "ERROR: name is required";
          return;
        }
        if (!value) {
          out.textContent = "ERROR: value is required";
          return;
        }
        try {
          const r = await invoke("mc_secret_set", { name, value });
          out.textContent = "OK: " + JSON.stringify(r, null, 2);
        } catch (e) {
          out.textContent = "ERROR: " + e;
        }
      });

      // --- 2. List ---
      root.querySelector("#sec-list").addEventListener("click", async () => {
        const out = root.querySelector("#sec-list-result");
        try {
          const r = await invoke("mc_secret_list");
          out.textContent = JSON.stringify(r, null, 2);
        } catch (e) {
          out.textContent = "ERROR: " + e;
        }
      });

      // --- 3. Delete ---
      root.querySelector("#sec-del").addEventListener("click", async () => {
        const name = root.querySelector("#sec-del-name").value.trim();
        const out = root.querySelector("#sec-del-result");
        if (!name) {
          out.textContent = "ERROR: name is required";
          return;
        }
        try {
          const r = await invoke("mc_secret_delete", { name });
          out.textContent = "OK: " + JSON.stringify(r);
        } catch (e) {
          out.textContent = "ERROR: " + e;
        }
      });

      // --- 4. Preprocess (JS only) ---
      root.querySelector("#preprocess-run").addEventListener("click", () => {
        const input = root.querySelector("#preprocess-input").value;
        const out = root.querySelector("#preprocess-result");
        const r = preprocessMessage(input);
        out.textContent = `Processed: ${r.processed}\n\nReferenced: ${JSON.stringify(r.referenced)}`;
      });

      // --- 5. Expand (Rust) ---
      root.querySelector("#expand-run").addEventListener("click", async () => {
        const input = root.querySelector("#expand-input").value;
        const out = root.querySelector("#expand-result");
        try {
          const r = await expandPlaceholders(input);
          out.textContent = `Expanded: ${r.expanded}\n\nUsed: ${JSON.stringify(r.used)}`;
        } catch (e) {
          out.textContent = "ERROR: " + e;
        }
      });

      // --- 6. Dump ---
      root.querySelector("#dump-run").addEventListener("click", async () => {
        const out = root.querySelector("#dump-result");
        try {
          const r = await invoke("mc_secret_debug_dump");
          out.textContent = r;
        } catch (e) {
          out.textContent = "ERROR: " + e;
        }
      });

      // --- Back ---
      root.querySelector("#back").addEventListener("click", () => {
        if (ctx && ctx.onBackToDashboard) ctx.onBackToDashboard();
      });
    },
    unmount() {
      // no-op
    },
};
