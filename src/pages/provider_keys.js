// pages/provider_keys.js — rc54 BYO provider keys UI (Lesson 713, 2026-08-28).
//
// Why this exists:
//   MiracleClaw ships with MAIC as the default provider. But users may
//   already have an OpenAI / Anthropic / Ollama Cloud account and want
//   to use their own quota instead of MAIC's subscription. This page
//   lets them paste their API key once, save it encrypted in the vault,
//   and the openclaw runtime picks it up via process.env on the next
//   chat request.
//
// What it does NOT do:
//   - No key validation against the upstream API (we'd need network
//     calls + provider-specific libraries; defer to a v2 smoke test).
//   - No model picker per provider (the openclaw plugin catalog handles
//     model discovery automatically once the key is set).
//   - No usage tracking (MAIC's quota endpoint already covers that).
//
// Auth:
//   requiresAuth = true (gated by page_registry). If a user without a
//   JWT somehow reaches this URL, they bounce to the login page.

import { invoke } from "@tauri-apps/api/core";

function escapeHtml(s) {
  return String(s ?? "")
    .replace(/[&<>"']/g, (c) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[c]);
}

// Provider metadata. Mirrors the Rust PROVIDER_REGISTRY in
// src-tauri/src/provider_keys.rs. When adding a provider there, also
// add it here so the UI renders the right label + help text.
const PROVIDERS = [
  {
    id: "openai",
    label: "OpenAI",
    icon: "🟢",
    help: "Powers GPT-5.3, o3, o4, and the Codex line.",
    envVar: "OPENAI_API_KEY",
    placeholder: "sk-proj-…",
    keyHint: "OpenAI keys start with <code>sk-</code>. Get one at platform.openai.com.",
    docsUrl: "https://platform.openai.com/api-keys",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    icon: "🟣",
    help: "Powers Claude Opus 4.5, Sonnet 4.5, Haiku 4.5.",
    envVar: "ANTHROPIC_API_KEY",
    placeholder: "sk-ant-…",
    keyHint: "Anthropic keys start with <code>sk-ant-</code>. Get one at console.anthropic.com.",
    docsUrl: "https://console.anthropic.com/settings/keys",
  },
  {
    id: "ollama-cloud",
    label: "Ollama Cloud",
    icon: "🦙",
    help: "Hosted open models (kimi, glm, qwen, deepseek, nemotron) via ollama.com. Free tier OK.",
    envVar: "OLLAMA_API_KEY",
    placeholder: "ollama.com API key (any value works for local Ollama)",
    keyHint: "For Ollama Cloud: get a key at ollama.com/settings. For local Ollama (127.0.0.1:11434), any non-empty value works.",
    docsUrl: "https://ollama.com/settings",
    isLocal: true,
  },
  {
    id: "mistral",
    label: "Mistral",
    icon: "🌀",
    help: "Mistral Large, Codestral, Pixtral.",
    envVar: "MISTRAL_API_KEY",
    placeholder: "your Mistral API key",
    keyHint: "Get one at console.mistral.ai.",
    docsUrl: "https://console.mistral.ai/",
  },
  {
    id: "cohere",
    label: "Cohere",
    icon: "🔷",
    help: "Command R+, Embed v3.",
    envVar: "COHERE_API_KEY",
    placeholder: "your Cohere API key",
    keyHint: "Get one at dashboard.cohere.ai.",
    docsUrl: "https://dashboard.cohere.ai/api-keys",
  },
  {
    id: "openrouter",
    label: "OpenRouter",
    icon: "🔀",
    help: "Single key, every model. Great for power users.",
    envVar: "OPENROUTER_API_KEY",
    placeholder: "sk-or-…",
    keyHint: "OpenRouter keys start with <code>sk-or-</code>. Get one at openrouter.ai/keys.",
    docsUrl: "https://openrouter.ai/keys",
  },
  {
    id: "groq",
    label: "Groq",
    icon: "⚡",
    help: "Ultra-fast Llama / Mixtral inference.",
    envVar: "GROQ_API_KEY",
    placeholder: "gsk_…",
    keyHint: "Groq keys start with <code>gsk_</code>. Get one at console.groq.com.",
    docsUrl: "https://console.groq.com/keys",
  },
  {
    id: "together",
    label: "Together AI",
    icon: "🤝",
    help: "Open-source models at low cost.",
    envVar: "TOGETHER_API_KEY",
    placeholder: "your Together API key",
    keyHint: "Get one at api.together.xyz.",
    docsUrl: "https://api.together.xyz/settings/api-keys",
  },
  {
    id: "xai",
    label: "xAI (Grok)",
    icon: "𝕏",
    help: "Grok 2, Grok 2 Vision.",
    envVar: "XAI_API_KEY",
    placeholder: "xai-…",
    keyHint: "Get one at console.x.ai.",
    docsUrl: "https://console.x.ai",
  },
  {
    id: "google",
    label: "Google AI",
    icon: "🔴",
    help: "Gemini 2.5 Pro / Flash via Google AI Studio.",
    envVar: "GOOGLE_API_KEY",
    placeholder: "AIza…",
    keyHint: "Get one at aistudio.google.com.",
    docsUrl: "https://aistudio.google.com/app/apikey",
  },
  {
    id: "voyage",
    label: "Voyage AI",
    icon: "🧭",
    help: "Embeddings (good for retrieval).",
    envVar: "VOYAGE_API_KEY",
    placeholder: "pa-…",
    keyHint: "Get one at dash.voyageai.com.",
    docsUrl: "https://dash.voyageai.com/api-keys",
  },
  {
    id: "deepgram",
    label: "Deepgram",
    icon: "🎙️",
    help: "Speech-to-text (used by Voice module).",
    envVar: "DEEPGRAM_API_KEY",
    placeholder: "your Deepgram API key",
    keyHint: "Get one at console.deepgram.com.",
    docsUrl: "https://console.deepgram.com/",
  },
  {
    id: "elevenlabs",
    label: "ElevenLabs",
    icon: "🔊",
    help: "Premium TTS voices.",
    envVar: "ELEVENLABS_API_KEY",
    placeholder: "your ElevenLabs API key",
    keyHint: "Get one at elevenlabs.io/app/settings/api-keys.",
    docsUrl: "https://elevenlabs.io/app/settings/api-keys",
  },
];

export const providerKeysPage = {
  label: "Provider Keys",
  icon: "🔑",
  requiresAuth: true,

  mount(root, ctx = {}) {
    const onBack = ctx.onBackToDashboard || (() => {});
    let summaries = [];

    root.innerHTML = `
      <div class="provider-keys-page">
        <header class="provider-keys-header">
          <button class="icon-link" id="pk-back-btn" title="Back to dashboard" aria-label="Back">←</button>
          <h1>🔑 Provider Keys</h1>
          <span class="badge-ok" title="Lesson 713 — your keys are stored AES-256-GCM in the local vault. They are set in the Tauri process env so the openclaw runtime can use them; they never leave your machine.">🔒 Encrypted at rest</span>
        </header>

        <section class="provider-keys-intro">
          <p>
            <strong>Bring your own API key</strong> for any provider below. Once saved,
            MiracleClaw will route requests for that provider through your account
            instead of MAIC's billing. Keys are stored encrypted in the local vault and
            never sent to MAIC.
          </p>
          <p class="muted small">
            You can mix and match — use MAIC for the default chat, your own OpenAI key
            for code generation, your own Ollama Cloud key for open models. Each
            provider's requests go to whichever account owns the key.
          </p>
        </section>

        <section id="pk-list" class="provider-keys-list">
          <p class="muted">Loading…</p>
        </section>

        <footer class="provider-keys-footer muted small">
          <p>
            <strong>How this works:</strong> when you save a key, MC writes it to the
            encrypted vault AND sets the matching env var (<code>OPENAI_API_KEY</code>,
            <code>ANTHROPIC_API_KEY</code>, …) in the Tauri process. The openclaw
            runtime reads <code>process.env.X</code> at request time and routes
            accordingly. Restarting MC restores the env from the vault automatically.
          </p>
          <p>
            <strong>Tip:</strong> if you only run Ollama locally (127.0.0.1:11434),
            set <code>OLLAMA_API_KEY</code> to any non-empty value — no real key
            needed. For Ollama Cloud, paste your real ollama.com key.
          </p>
        </footer>
      </div>
    `;

    document.getElementById("pk-back-btn").addEventListener("click", () => onBack());

    async function refresh() {
      const list = document.getElementById("pk-list");
      list.innerHTML = '<p class="muted">Loading…</p>';
      try {
        summaries = await invoke("mc_list_provider_keys");
        renderList();
      } catch (e) {
        list.innerHTML = `<p class="err">Failed to load provider keys: ${escapeHtml(String(e))}</p>`;
      }
    }

    function summaryFor(providerId) {
      return summaries.find((s) => s.provider_id === providerId);
    }

    function renderList() {
      const list = document.getElementById("pk-list");
      list.innerHTML = PROVIDERS.map((p) => {
        const s = summaryFor(p.id);
        const isSet = s && s.is_set;
        const baseUrl = s && s.base_url;
        const valueLen = s ? s.value_len : 0;
        const statusPill = isSet
          ? `<span class="pk-pill pk-pill-ok" title="Stored in vault as ${escapeHtml(s.env_var)} (${valueLen} chars)">✓ Set</span>`
          : `<span class="pk-pill pk-pill-none">Not set</span>`;
        const baseUrlRow = p.isLocal
          ? `
            <div class="pk-field pk-field-base-url">
              <label>
                <span>Base URL (optional — leave blank for default)</span>
                <input type="text" class="pk-base-url" data-provider="${p.id}"
                       placeholder="https://ollama.com/v1  (or http://localhost:11434/v1)"
                       value="${escapeHtml(baseUrl || "")}" />
              </label>
            </div>
          `
          : "";
        return `
          <div class="pk-card" data-provider="${p.id}">
            <div class="pk-card-header">
              <div class="pk-card-title">
                <span class="pk-icon">${p.icon}</span>
                <div>
                  <div class="pk-label">${escapeHtml(p.label)}</div>
                  <div class="pk-env muted small">${escapeHtml(p.envVar)}</div>
                </div>
              </div>
              ${statusPill}
            </div>
            <div class="pk-card-body">
              <div class="pk-help">${p.help}</div>
              <div class="pk-field">
                <label>
                  <span>API key</span>
                  <div class="pk-input-row">
                    <input type="password" class="pk-key-input" data-provider="${p.id}"
                           placeholder="${escapeHtml(p.placeholder)}"
                           autocomplete="off" spellcheck="false" />
                    <button type="button" class="pk-toggle-visibility" data-provider="${p.id}"
                            title="Show / hide key">👁</button>
                  </div>
                  <small class="muted">${p.keyHint} <a href="${p.docsUrl}" target="_blank" rel="noopener">Get one →</a></small>
                </label>
              </div>
              ${baseUrlRow}
              <div class="pk-actions">
                <button type="button" class="primary pk-save" data-provider="${p.id}">
                  ${isSet ? "Update key" : "Save key"}
                </button>
                ${isSet ? `<button type="button" class="link-button pk-clear" data-provider="${p.id}">Remove</button>` : ""}
              </div>
              <div class="pk-status muted small" data-provider="${p.id}"></div>
            </div>
          </div>
        `;
      }).join("");

      // Wire actions
      list.querySelectorAll(".pk-toggle-visibility").forEach((btn) => {
        btn.addEventListener("click", () => {
          const providerId = btn.dataset.provider;
          const input = list.querySelector(`.pk-key-input[data-provider="${providerId}"]`);
          if (input) {
            input.type = input.type === "password" ? "text" : "password";
          }
        });
      });

      list.querySelectorAll(".pk-save").forEach((btn) => {
        btn.addEventListener("click", () => saveProvider(btn.dataset.provider));
      });

      list.querySelectorAll(".pk-clear").forEach((btn) => {
        btn.addEventListener("click", () => clearProvider(btn.dataset.provider));
      });
    }

    async function saveProvider(providerId) {
      const status = document.querySelector(`.pk-status[data-provider="${providerId}"]`);
      const input = document.querySelector(`.pk-key-input[data-provider="${providerId}"]`);
      const baseUrlInput = document.querySelector(`.pk-base-url[data-provider="${providerId}"]`);
      const value = input ? input.value.trim() : "";
      const baseUrl = baseUrlInput ? baseUrlInput.value.trim() : null;
      if (!value) {
        status.textContent = "❌ API key cannot be empty";
        status.style.color = "#dc2626";
        return;
      }
      status.textContent = "Saving…";
      status.style.color = "";
      try {
        const result = await invoke("mc_set_provider_key", {
          providerId,
          apiKey: value,
          baseUrl: baseUrl || null,
        });
        status.textContent = `✅ Saved as ${result.env_var} (${result.summary.value_len} chars).`;
        status.style.color = "#059669";
        // Clear the password input after save.
        if (input) input.value = "";
        await refresh();
      } catch (e) {
        status.textContent = `❌ ${String(e)}`;
        status.style.color = "#dc2626";
      }
    }

    async function clearProvider(providerId) {
      const provider = PROVIDERS.find((p) => p.id === providerId);
      if (!confirm(`Remove your ${provider ? provider.label : providerId} API key?\n\nFuture requests will fall back to MAIC's default routing.`)) return;
      const status = document.querySelector(`.pk-status[data-provider="${providerId}"]`);
      status.textContent = "Removing…";
      status.style.color = "";
      try {
        await invoke("mc_clear_provider_key", { providerId });
        status.textContent = "✅ Removed.";
        status.style.color = "#059669";
        await refresh();
      } catch (e) {
        status.textContent = `❌ ${String(e)}`;
        status.style.color = "#dc2626";
      }
    }

    refresh();
  },

  unmount() {
    // No teardown needed — no timers, no abort controllers.
  },
};
