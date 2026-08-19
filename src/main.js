// MiracleClaw frontend entry point.
//
// On boot:
//   1. Call invoke('first_run_report') to check MAIC provider status.
//   2. If needs_maic_login === true → render login form.
//   3. If first_run_report.maic_provider_configured === true → redirect to
//      the openclaw chat UI at http://localhost:28789/.
//
// On login submit:
//   1. invoke('maic_login', { email, password })
//   2. On success → redirect to chat UI (chat will work now that MAIC is wired).
//   3. On error → surface error message, let user retry.

import "./styles.css";

const { invoke } = window.__TAURI__.core;

const root = document.getElementById("root");

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

async function boot() {
  let report;
  try {
    report = await invoke("first_run_report");
  } catch (e) {
    showFatal(`Could not check MC status: ${escapeHtml(e)}\n\nTry restarting MC.`);
    return;
  }

  if (report.needs_maic_login) {
    renderLogin(report.maic_provider_endpoint || "https://maicserver.com");
  } else {
    // Already configured — go straight to chat UI.
    window.location.href = "http://localhost:28789/";
  }
}

function renderLogin(endpoint) {
  root.innerHTML = `
    <div class="login-card">
      <h1 class="logo">MiracleClaw</h1>
      <p class="subtitle">Sign in with your MAIC account</p>

      <form id="login-form" novalidate>
        <label>
          Email
          <input
            type="email"
            name="email"
            id="login-email"
            autocomplete="email"
            required
            placeholder="you@example.com"
          />
        </label>

        <label>
          Password
          <input
            type="password"
            name="password"
            id="login-password"
            autocomplete="current-password"
            required
            placeholder="********"
          />
        </label>

        <button type="submit" id="login-submit">Sign in</button>
      </form>

      <div id="login-error" class="error" hidden></div>

      <div class="footer">
        <p class="muted small">
          Signing in connects to <code>${escapeHtml(endpoint)}</code>.<br />
          Don't have a MAIC account?
          <a href="https://milagrocloud.com/register" target="_blank" rel="noopener">
            Create one — starts free, upgrade anytime.
          </a>
        </p>
      </div>
    </div>
  `;

  const form = document.getElementById("login-form");
  const errorEl = document.getElementById("login-error");
  const submit = document.getElementById("login-submit");

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    errorEl.hidden = true;
    submit.disabled = true;
    submit.textContent = "Signing in…";

    const email = document.getElementById("login-email").value.trim();
    const password = document.getElementById("login-password").value;

    try {
      const result = await invoke("maic_login", { email, password });
      // Login succeeded → MAIC provider is now wired in openclaw.json.
      // Jump to the chat UI. The next /v1/chat/completions call will use
      // the new JWT.
      window.location.href = "http://localhost:28789/";
      // Reference result so V8/etc don't optimistically say "unused".
      console.info(
        "[MiracleClaw] Login OK — user=",
        result.email,
        "tier=",
        result.tier,
        "endpoint=",
        result.endpoint
      );
    } catch (err) {
      // err is a string per the Tauri command signature
      errorEl.textContent = String(err);
      errorEl.hidden = false;
      submit.disabled = false;
      submit.textContent = "Sign in";
    }
  });

  // Auto-focus first input
  document.getElementById("login-email").focus();
}

function showFatal(msg) {
  root.innerHTML = `
    <div class="fatal">
      <h1>MiracleClaw encountered a problem</h1>
      <pre>${escapeHtml(msg)}</pre>
      <p>Please restart the app. If this keeps happening, file an issue at
        <a href="https://github.com/anomalyco/" target="_blank" rel="noopener">github.com/anomalyco/</a>.
      </p>
    </div>
  `;
}

boot();
