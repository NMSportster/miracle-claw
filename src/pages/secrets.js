// pages/secrets.js — rc53.6 friendly secrets vault UI.
//
// Single form, three lifetime choices. Backend canonicalizes
// user-typed labels ("Strip Key" -> STRIP_KEY) via Rust's
// canonicalize_label(). The UI shows the actual stored name so
// users know what reference to use ($STRIP_KEY) in chat.
//
// Three lifetimes:
//   🟠 Once      — gone after first read by bash_run
//   🟣 Per Session — lives until app closes
//   🔵 Vault     — saved encrypted, forever (rc54 — currently plaintext)
//
// Single Mutex<HashMap<String, EphemeralEntry>> in Rust, with
// a Lifetime enum dispatching persistence. UI fetches both the
// disk vault (mc_secret_list) AND the in-memory pool
// (mc_secret_list_ephemerals), merges for display, dedup by name.

import { invoke } from "@tauri-apps/api/core";

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
  if (iso.startsWith("epoch:")) {
    const secs = Number(iso.slice(6));
    if (Number.isFinite(secs)) {
      return new Date(secs * 1000).toLocaleString();
    }
  }
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

const KIND_LABELS = {
  username: "Username",
  password: "Password",
  key: "Key",
  token: "Token",
  url: "URL",
  generic: "Secret",
};

const LIFETIME_META = {
  once: {
    label: "Once",
    color: "#f59e0b",
    hint: "Used once by bash_run, then gone",
  },
  per_session: {
    label: "Per Session",
    color: "#8b5cf6",
    hint: "Lives until the app closes",
  },
  vault: {
    label: "Vault",
    color: "#3b82f6",
    hint: "Saved forever (encrypted in v1)",
  },
};

// Lifetime pill: small colored dot + label, used in the list rows.
function lifetimePill(lifetime) {
  const meta = LIFETIME_META[lifetime] || { label: lifetime, color: "#6b7280" };
  return `<span class="lifetime-pill" style="background:${meta.color}">${escapeHtml(meta.label)}</span>`;
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
          <button class="icon-link" id="back-btn" title="Back to dashboard" aria-label="Back">←</button>
          <h1>🔑 Secrets</h1>
          <span class="badge-ok" title="rc53.8 — AES-256-GCM at rest. Master key in OS keychain. Legacy plaintext vaults auto-migrate on first save.">🔒 Encrypted at rest</span>
        </header>

        <section class="secrets-add">
          <h2>Add a secret</h2>
          <p class="muted small">
            Type whatever you want — we'll normalize it. "Strip Key", "sudo password",
            "STRIPE_KEY" all become a clean shell-style name you can reference as
            <code>$NAME</code> in chat.
          </p>
          <div class="secrets-form">
            <label class="secrets-field">
              <span>What is it for?</span>
              <input id="sec-label" type="text" placeholder="e.g. Strip Key, sudo password, GitHub PAT" autocomplete="off" />
            </label>

            <div class="secrets-field">
              <span>Type</span>
              <div class="secrets-kinds" role="group" aria-label="Secret type (row 1)">
                <label class="kind-chip"><input type="checkbox" name="kind-1" value="username" /> <span>Username</span></label>
                <label class="kind-chip"><input type="checkbox" name="kind-1" value="password" /> <span>Password</span></label>
                <label class="kind-chip"><input type="checkbox" name="kind-1" value="key" checked /> <span>Key</span></label>
                <label class="kind-chip"><input type="checkbox" name="kind-1" value="token" /> <span>Token</span></label>
                <label class="kind-chip"><input type="checkbox" name="kind-1" value="url" /> <span>URL</span></label>
              </div>
            </div>

            <label class="secrets-field">
              <span>Value</span>
              <textarea id="sec-value" rows="3" placeholder="paste the secret here (multi-line OK)" autocomplete="off" spellcheck="false"></textarea>
            </label>

            <details class="secrets-row-2">
              <summary>Add a second value (optional — e.g. username + password pair)</summary>
              <div class="secrets-field">
                <span>Type</span>
                <div class="secrets-kinds" role="group" aria-label="Secret type (row 2)">
                  <label class="kind-chip"><input type="checkbox" name="kind-2" value="username" /> <span>Username</span></label>
                  <label class="kind-chip"><input type="checkbox" name="kind-2" value="password" checked /> <span>Password</span></label>
                  <label class="kind-chip"><input type="checkbox" name="kind-2" value="key" /> <span>Key</span></label>
                  <label class="kind-chip"><input type="checkbox" name="kind-2" value="token" /> <span>Token</span></label>
                  <label class="kind-chip"><input type="checkbox" name="kind-2" value="url" /> <span>URL</span></label>
                </div>
              </div>

              <label class="secrets-field">
                <span>Value (row 2)</span>
                <textarea id="sec-value-2" rows="3" placeholder="leave empty to skip row 2" autocomplete="off" spellcheck="false"></textarea>
              </label>
            </details>

            <div class="secrets-field">
              <span>How long should we keep it?</span>
              <div class="secrets-lifetimes" role="radiogroup" aria-label="Lifetime">
                <label class="lifetime-option" style="--pill-color:#f59e0b">
                  <input type="radio" name="lifetime" value="once" />
                  <span class="lifetime-dot"></span>
                  <span class="lifetime-body">
                    <strong>Once</strong>
                    <small>gone after first use</small>
                  </span>
                </label>
                <label class="lifetime-option" style="--pill-color:#8b5cf6">
                  <input type="radio" name="lifetime" value="per_session" />
                  <span class="lifetime-dot"></span>
                  <span class="lifetime-body">
                    <strong>Per Session</strong>
                    <small>until app closes</small>
                  </span>
                </label>
                <label class="lifetime-option" style="--pill-color:#3b82f6">
                  <input type="radio" name="lifetime" value="vault" checked />
                  <span class="lifetime-dot"></span>
                  <span class="lifetime-body">
                    <strong>Vault</strong>
                    <small>saved forever</small>
                  </span>
                </label>
              </div>
            </div>

            <div class="secrets-actions">
              <button class="primary" id="sec-set">Save secret</button>
              <span id="sec-set-status" class="muted small"></span>
            </div>
          </div>
        </section>

        <section class="secrets-list-section">
          <div class="secrets-list-header">
            <h2>Stored secrets</h2>
            <div class="secrets-list-actions">
              <button class="link-button" id="sec-clear-session" title="Remove all per-session entries">Clear Per Session</button>
              <button class="link-button" id="sec-refresh">Refresh</button>
            </div>
          </div>
          <div id="secrets-list" class="secrets-list">
            <p class="muted">Loading…</p>
          </div>
          <p class="muted small">
            Values are never shown here — only their length and lifetime.
            Use the Vault entries for permanent storage; Once and Per Session
            are gone after their lifetime ends.
          </p>
        </section>

        <footer class="secrets-footer muted small">
          <p>
            <strong>How this protects you:</strong> when you type
            <code>$STRIPE_KEY</code> in chat, MC replaces it with
            <code>&lt;&lt;secret:STRIPE_KEY&gt;&gt;</code> before the
            message is sent to MAIC. The AI only sees the placeholder.
            If it calls <code>bash_run</code>, MC expands the placeholder
            to the real value at exec time only.
          </p>
          <p>
            <strong>v0 limitations:</strong> Vault entries are stored
            in plaintext JSON on disk. Encryption (AES-256-GCM) ships
            in rc54. Once and Per Session entries live in memory only
            and never touch disk.
          </p>
        </footer>
      </div>
    `;

    // ----- Wire actions -----

    document.getElementById("back-btn").addEventListener("click", () => onBack());

    const labelInput = document.getElementById("sec-label");
    const valueInput = document.getElementById("sec-value");
    const valueInput2 = document.getElementById("sec-value-2");
    const setButton = document.getElementById("sec-set");
    const setStatus = document.getElementById("sec-set-status");

    // Lesson 564 (2026-08-24 17:32 MDT, David): row 1 + optional row 2
    // for paired entries (e.g. username + password for one service).
    // Row 1 keeps the original kind name `kind-1`; row 2 uses `kind-2`.
    // The frontend auto-suffixes the row-2 label with `_<kind>` so
    // both atoms get distinct canonical names (STRIPE_USERNAME /
    // STRIPE_PASSWORD) without needing a new backend API.
    function getSelectedKind(rowNum) {
      const name = rowNum === 2 ? "kind-2" : "kind-1";
      const checked = document.querySelector(`input[name="${name}"]:checked`);
      return checked ? checked.value : "generic";
    }
    function getSelectedLifetime() {
      const checked = document.querySelector('input[name="lifetime"]:checked');
      return checked ? checked.value : "vault";
    }

    async function doSet() {
      const label = labelInput.value.trim();
      const value1 = valueInput.value;
      const value2 = valueInput2 ? valueInput2.value : "";
      const kind1 = getSelectedKind(1);
      const kind2 = getSelectedKind(2);
      const lifetime = getSelectedLifetime();
      setStatus.textContent = "";
      if (!value1 && !value2) {
        setStatus.textContent = "❌ At least one row must have a value";
        return;
      }
      setButton.disabled = true;
      const origText = setButton.textContent;
      setButton.textContent = "Saving…";
      const stored = []; // collect FriendlySetResult for status display
      try {
        if (value1) {
          const r = await invoke("mc_secret_set_friendly", {
            label, value: value1, lifetime, kind: kind1,
          });
          stored.push(r);
        }
        if (value2) {
          // Auto-suffix row 2's label with the kind so the two entries
          // don't collide on the canonical name. Strip kind text up
          // case so the shell var reads naturally: STRIPE_PASSWORD.
          const kindSuffix = String(kind2 || "value").toUpperCase();
          const label2 = `${label}_${kindSuffix}`;
          const r = await invoke("mc_secret_set_friendly", {
            label: label2, value: value2, lifetime, kind: kind2,
          });
          stored.push(r);
        }
        const lifetimeMeta = LIFETIME_META[stored[0].lifetime] || { label: stored[0].lifetime };
        const namesList = stored
          .map((r) => `<code>${escapeHtml(r.name)}</code>`)
          .join(", ");
        const lts = stored.length > 1 ? "each" : "";
        setStatus.innerHTML = `✅ Stored ${lts} as ${namesList} <span class="lifetime-pill" style="background:${lifetimeMeta.color}">${escapeHtml(lifetimeMeta.label)}</span> <span class="muted">(${stored.reduce((a, r) => a + r.value_len, 0)} chars total)</span>`;
        valueInput.value = "";
        if (valueInput2) valueInput2.value = "";
        labelInput.value = "";
        await refreshList();
      } catch (e) {
        setStatus.textContent = `❌ ${escapeHtml(String(e))}`;
      } finally {
        setButton.disabled = false;
        setButton.textContent = origText;
      }
    }
    setButton.addEventListener("click", doSet);
    // Ctrl+Enter in either value textarea submits
    valueInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        doSet();
      }
    });
    if (valueInput2) {
      valueInput2.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          doSet();
        }
      });
    }

    document.getElementById("sec-refresh").addEventListener("click", refreshList);
    document.getElementById("sec-clear-session").addEventListener("click", async () => {
      if (!confirm("Clear all Per Session secrets? (Vault entries are kept.)")) return;
      try {
        const n = await invoke("mc_secret_clear_session_ephemerals");
        await refreshList();
        setStatus.textContent = `Cleared ${n} Per Session entries.`;
      } catch (e) {
        setStatus.textContent = `❌ ${escapeHtml(String(e))}`;
      }
    });

    // ----- Initial load -----
    refreshList();

    async function refreshList() {
      const list = document.getElementById("secrets-list");
      list.innerHTML = '<p class="muted">Loading…</p>';
      try {
        // Merge disk vault + in-memory pool. Pool entries for Vault
        // lifetime are mirror copies, so we dedup by name (in-memory
        // version wins because it's fresher and includes Once/PS).
        const [disk, ephem] = await Promise.all([
          invoke("mc_secret_list").catch(() => []),
          invoke("mc_secret_list_ephemerals").catch(() => []),
        ]);
        const merged = new Map();
        for (const s of disk || []) {
          merged.set(s.name, {
            name: s.name,
            lifetime: "vault",
            kind: "generic",
            value_len: s.value_len,
            created_at: s.created_at,
            source: "disk",
          });
        }
        for (const e of ephem || []) {
          merged.set(e.name, {
            name: e.name,
            lifetime: e.lifetime,
            kind: e.kind,
            value_len: e.value_len,
            created_at: e.created_at,
            source: "memory",
          });
        }

        const items = [...merged.values()].sort((a, b) =>
          (b.created_at || "").localeCompare(a.created_at || "")
        );

        if (items.length === 0) {
          list.innerHTML = '<p class="muted">No secrets stored yet. Add one above.</p>';
          return;
        }
        list.innerHTML = items.map((s) => {
          const kindLabel = KIND_LABELS[s.kind] || s.kind;
          return `
            <div class="secret-row" data-name="${escapeHtml(s.name)}">
              ${lifetimePill(s.lifetime)}
              <div class="secret-row-main">
                <div class="secret-name">${escapeHtml(s.name)}</div>
                <div class="secret-meta muted small">
                  ${escapeHtml(kindLabel)} · ${s.value_len} chars · created ${formatDate(s.created_at)}
                </div>
              </div>
              <div class="secret-row-actions">
                ${s.lifetime === "vault" ? `<button class="link-button secret-delete" data-name="${escapeHtml(s.name)}">Delete</button>` : ""}
              </div>
            </div>
          `;
        }).join("");

        // Wire per-row delete (vault only — Once/PS auto-clear)
        list.querySelectorAll(".secret-delete").forEach((btn) => {
          btn.addEventListener("click", async () => {
            const n = btn.dataset.name;
            if (!confirm(`Delete vault secret "${n}"? This cannot be undone.`)) return;
            try {
              await invoke("mc_secret_delete", { name: n });
              await refreshList();
            } catch (e) {
              alert(`Delete failed: ${e}`);
            }
          });
        });
      } catch (e) {
        list.innerHTML = `<p class="err">Failed to load: ${escapeHtml(String(e))}</p>`;
      }
    }
  },

  unmount() {
    // rc54+ will wipe any expanded-test value from the DOM here.
    // rc53.6: no teardown needed beyond default.
  },
};