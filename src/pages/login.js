// pages/login.js — MAIC login form (extracted from main.js v1.0.9-rc34).
//
// Public API: { mount(root, ctx), unmount() }
//
// Mount behavior:
//   - Renders the login card into `root`.
//   - ctx may include `endpoint` (MAIC URL) and `onSuccess` (called after
//     a successful login — the registry or caller decides what to do next).
//
// Unmount behavior:
//   - Clears any pending error state.
//   - (No timers, no abort controllers to clean up in this page yet.)
//
// Future hooks:
//   - If we add "forgot password?" or "magic link", they live here.

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

export const loginPage = {
  label: "Sign in",
  icon: null,
  requiresAuth: false,

  mount(root, ctx = {}) {
    const endpoint = ctx.endpoint || "https://maicserver.com";
    const onSuccess = ctx.onSuccess || (() => {});

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

          <label class="checkbox">
            <input
              type="checkbox"
              name="remember"
              id="login-remember"
            />
            <span>Stay signed in (encrypts your password in this machine's secure store)</span>
          </label>

          <button type="submit" id="login-submit">Sign in</button>

          <button type="button" id="register-btn" class="register-btn">
            New here? Create a MAIC account
          </button>
        </form>

        <div id="login-error" class="error" hidden></div>

        <div class="footer">
          <p class="muted small">
            Signing in connects to <code>${escapeHtml(endpoint)}</code>.
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
      const remember = document.getElementById("login-remember").checked;

      try {
        const result = await invoke("maic_login", { email, password, remember });
        submit.textContent = "Loading dashboard…";
        // Caller decides what to mount next (usually the dashboard).
        onSuccess(result);
      } catch (err) {
        // err is a string per the Tauri command signature
        errorEl.textContent = String(err);
        errorEl.hidden = false;
        submit.disabled = false;
        submit.textContent = "Sign in";
      }
    });

    // v1.0.9-rc33 (David 13:00 MDT): update New User button to point at
    // /signup (the canonical page that matches /v1/users/signup API).
    // /register 404'd because it didn't exist; /signup is the actual HTML
    // page added in the MAIC api/routes/users.py signup_page handler.
    // Opens in OS default browser, not inside the Tauri webview.
    document.getElementById("register-btn").addEventListener("click", async () => {
      try {
        await invoke("open_register_url", {
          url: "https://milagrocloud.com/signup",
        });
      } catch (e) {
        console.error("[login] could not open register URL:", e);
        errorEl.textContent =
          "Could not open your browser automatically. Please visit " +
          "milagrocloud.com/signup to create an account.";
        errorEl.hidden = false;
      }
    });

    document.getElementById("login-email").focus();
  },

  unmount() {
    // Nothing to clean up yet. Reset any visible error so a re-mount
    // doesn't show stale text.
    const errorEl = document.getElementById("login-error");
    if (errorEl) {
      errorEl.hidden = true;
      errorEl.textContent = "";
    }
  },
};
