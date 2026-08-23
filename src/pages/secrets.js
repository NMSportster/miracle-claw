// pages/secrets.js — v0 secrets vault UI (rc53.5+).
//
// MINIMAL UI for the v0 plaintext vault. Sets / lists / deletes /
// previews / expands. NO encryption yet — that's rc54.
//
// Threat model (locked-in, see notes/SECRETS-VAULT.md):
//   - Plaintext values never reach MAIC. The JS preprocessor
//     (src/secrets/preprocessor.js) scrubs $NAME refs to
//     <<secret:NAME>> placeholders before any network call.
//   - Plaintext values DO appear in this page (necessary to set
//     them) but are NEVER rendered as plaintext in the list view
//     (only as masked dots).
//   - When the user clicks "Expand test", the expanded string is
//     shown in a <pre> briefly so they can verify behavior, then
//     they should close the page. v1 (rc54) will add auto-wipe
//     on page unmount.
//
// Future (rc54):
//   - Dashboard tile click → this page
//   - Toolbar button in Terminal + OpenClaw webview → modal
//   - AES-256-GCM encryption + PBKDF2 master key
//   - Auto-wipe expanded value after N seconds
//   - Reveal button on each row to show value temporarily

import { invoke } from "@tauri-apps/api/core";
import {
  preprocessMessage,
  expandPlaceholders,
} from "../secrets/preprocessor.js";

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatDate(iso) {
  if (!iso) return "—";
  // v0 uses "epoch:<secs>" format; v1 will be RFC 3339.
  if (iso.startsWith("epoch:")) {
    const secs = Number(iso.slice(6));
    if (Number.isFinite(secs)) {
      return new Date(secs * 1000).toLocaleString();
    }
  }
  // Fallback: try to parse as date
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export const secretsPage = {
  label: "Secrets",
  icon: "🔑",
  requiresAuth: true,

  mount(root, ctx = {}) {
    const onBack = ctx.onBackToDashboard || (() => {});

    root.innerHTML = `
      <div class="secrets-page">
        <header class="secrets-header">
          <button class="icon-link" id="back-btn" title="Back to dashboard" aria-label="Back">
            ←
          </button>
          <h1>🔑 Secrets</h1>
          <span class="badge-warn" title="v0 — plaintext JSON on disk. Encryption ships in rc54.">v0 — unencrypted</span>
        </header>

        <section class="secrets-add">
          <h2>Add or update a secret</h2>
          <p class="muted small">
            Name must match shell-var rules: starts with A-Z or
            underscore, then A-Z, 0-9, or underscore. Examples:
            <code>STRIPE_KEY</code>, <code>_PRIVATE_TOKEN</code>,
            <code>AWS_ACCESS_KEY_ID</code>.
          </p>
          <div class="secrets-form">
            <input id="sec-name" type="text" placeholder="NAME (e.g. STRIPE_KEY)" autocomplete="off" />
            <input id="sec-value" type="password" placeholder="value" autocomplete="off" />
            <button class="primary" id="sec-set">Save</button>
          </div>
          <div id="sec-set-status" class="muted small"></div>
        </section>

        <section class="secrets-list-section">
          <div class="secrets-list-header">
            <h2>Stored secrets</h2>
            <button class="link-button" id="sec-refresh">Refresh</button>
          </div>
          <div id="secrets-list" class="secrets-list">
            <p class="muted">Loading…</p>
          </div>
          <p class="muted small">
            Values are never shown in this list — only their length.
            Use the "Reveal" button to copy a value to your clipboard
            if you need to inspect it.
          </p>
        </section>

        <section class="secrets-test">
          <h2>Test the placeholder flow</h2>
          <p class="muted small">
            Type a message with <code>$NAME</code> references and see
            what the AI would actually receive. Then expand the
            placeholders to see what <code>bash_run</code> would
            execute. No network calls happen here — this is a local
            preview.
          </p>

          <div class="secrets-test-block">
            <label for="test-input">Your message (with $NAME refs)</label>
            <textarea id="test-input" rows="3" placeholder="e.g. Use $STRIPE_KEY to call the API"></textarea>
            <div class="secrets-test-actions">
              <button id="test-rewrite">Show what AI sees</button>
              <button id="test-expand">Expand placeholders (what bash_run sees)</button>
            </div>
            <div class="secrets-test-output">
              <div class="secrets-test-col">
                <h3>AI sees (placeholders)</h3>
                <pre id="test-rewritten" class="test-out"><span class="muted">Click "Show what AI sees"</span></pre>
              </div>
              <div class="secrets-test-col">
                <h3>bash_run sees (real values)</h3>
                <pre id="test-expanded" class="test-out"><span class="muted">Click "Expand placeholders"</span></pre>
                <p class="muted small warn-text">⚠ Plaintext — close this tab when done. Auto-wipe ships in rc54.</p>
              </div>
            </div>
          </div>
        </section>

        <section class="secrets-debug-link">
          <details>
            <summary>Show raw vault file on disk</summary>
            <button id="dump-run" class="link-button">Dump vault JSON</button>
            <pre id="dump-result" class="test-out"></pre>
            <p class="muted small">
              File path on Windows:
              <code>%APPDATA%\\MiracleClaw\\secrets.json</code>
            </p>
          </details>
        </section>

        <footer class="secrets-footer muted small">
          <p>
            <strong>How this protects you:</strong> when you type
            <code>$STRIPE_KEY</code> in chat, MC replaces it with
            <code>&lt;&lt;secret:STRIPE_KEY&gt;&gt;</code> before the
            message is sent to MAIC. The AI only sees the placeholder.
            If it calls <code>bash_run</code>, MC expands the placeholder
            to the real value at exec time only. The plaintext never
            appears in MAIC's logs, conversation history, or the
            AI's tool arguments.
          </p>
          <p>
            <strong>v0 limitations:</strong> secrets are stored in
            plaintext JSON on disk. Encryption (AES-256-GCM) and a
            master passphrase ship in rc54.
          </p>
        </footer>
      </div>
    `;

    // ----- Wire actions -----

    document.getElementById("back-btn").addEventListener("click", () => onBack());

    const nameInput = document.getElementById("sec-name");
    const valueInput = document.getElementById("sec-value");
    const setButton = document.getElementById("sec-set");
    const setStatus = document.getElementById("sec-set-status");

    async function doSet() {
      const name = nameInput.value.trim();
      const value = valueInput.value;
      setStatus.textContent = "";
      if (!name) {
        setStatus.textContent = "❌ Name is required";
        return;
      }
      if (!value) {
        setStatus.textContent = "❌ Value is required";
        return;
      }
      setButton.disabled = true;
      setButton.textContent = "Saving…";
      try {
        const r = await invoke("mc_secret_set", { name, value });
        setStatus.textContent = `✅ Saved ${r.name} (${r.value_len} chars)`;
        valueInput.value = "";
        await refreshList();
      } catch (e) {
        setStatus.textContent = `❌ ${escapeHtml(String(e))}`;
      } finally {
        setButton.disabled = false;
        setButton.textContent = "Save";
      }
    }
    setButton.addEventListener("click", doSet);
    // Allow Ctrl+Enter in the value field to submit
    valueInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        doSet();
      }
    });

    document.getElementById("sec-refresh").addEventListener("click", refreshList);

    // ----- Test section -----

    const testInput = document.getElementById("test-input");
    const testRewritten = document.getElementById("test-rewritten");
    const testExpanded = document.getElementById("test-expanded");

    document.getElementById("test-rewrite").addEventListener("click", () => {
      const text = testInput.value;
      if (!text.trim()) {
        testRewritten.innerHTML = '<span class="muted">Type something first</span>';
        return;
      }
      const r = preprocessMessage(text);
      testRewritten.textContent = r.processed;
    });

    document.getElementById("test-expand").addEventListener("click", async () => {
      const text = testInput.value;
      if (!text.trim()) {
        testExpanded.innerHTML = '<span class="muted">Type something first</span>';
        return;
      }
      try {
        const r = await expandPlaceholders(text);
        testExpanded.textContent = r.expanded;
      } catch (e) {
        testExpanded.textContent = `ERROR: ${escapeHtml(String(e))}`;
      }
    });

    // ----- Dump -----

    document.getElementById("dump-run").addEventListener("click", async () => {
      const out = document.getElementById("dump-result");
      try {
        const r = await invoke("mc_secret_debug_dump");
        out.textContent = r;
      } catch (e) {
        out.textContent = `ERROR: ${escapeHtml(String(e))}`;
      }
    });

    // ----- Initial load -----
    refreshList();

    async function refreshList() {
      const list = document.getElementById("secrets-list");
      list.innerHTML = '<p class="muted">Loading…</p>';
      try {
        const items = await invoke("mc_secret_list");
        if (!items || items.length === 0) {
          list.innerHTML = '<p class="muted">No secrets stored yet. Add one above.</p>';
          return;
        }
        list.innerHTML = items.map((s) => `
          <div class="secret-row" data-name="${escapeHtml(s.name)}">
            <div class="secret-row-main">
              <div class="secret-name">${escapeHtml(s.name)}</div>
              <div class="secret-meta muted small">
                ${s.value_len} chars · created ${formatDate(s.created_at)}
                ${s.last_used_at ? `· used ${formatDate(s.last_used_at)}` : ""}
              </div>
            </div>
            <div class="secret-row-actions">
              <button class="link-button secret-reveal" data-name="${escapeHtml(s.name)}">Reveal</button>
              <button class="link-button secret-delete" data-name="${escapeHtml(s.name)}">Delete</button>
            </div>
          </div>
        `).join("");

        // Wire per-row actions
        list.querySelectorAll(".secret-delete").forEach((btn) => {
          btn.addEventListener("click", async () => {
            const n = btn.dataset.name;
            if (!confirm(`Delete secret "${n}"? This cannot be undone.`)) return;
            try {
              await invoke("mc_secret_delete", { name: n });
              await refreshList();
            } catch (e) {
              alert(`Delete failed: ${e}`);
            }
          });
        });

        list.querySelectorAll(".secret-reveal").forEach((btn) => {
          btn.addEventListener("click", async () => {
            // Reveal: ask Rust for the value via debug_dump, parse,
            // find the matching name. This is a v0 shortcut — v1
            // will add a dedicated `mc_secret_reveal` command that
            // shows the value through the audit log.
            try {
              const dump = await invoke("mc_secret_debug_dump");
              const data = JSON.parse(dump);
              const entry = data.find((e) => e.name === btn.dataset.name);
              if (entry) {
                await navigator.clipboard.writeText(entry.value);
                btn.textContent = "Copied!";
                setTimeout(() => { btn.textContent = "Reveal"; }, 1500);
              }
            } catch (e) {
              alert(`Reveal failed: ${e}`);
            }
          });
        });
      } catch (e) {
        list.innerHTML = `<p class="err">Failed to load: ${escapeHtml(String(e))}</p>`;
      }
    }
  },

  unmount() {
    // v1 (rc54) will wipe any expanded-test value from the DOM here.
    // For v0, no teardown needed — the values are in the user's
    // browser tab and get cleared when they navigate away.
  },
};
