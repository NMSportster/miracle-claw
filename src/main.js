// MiracleClaw frontend entry point.
//
// v1.0.7+: post-login = dashboard (was: redirect straight to OpenClaw chat).
//
// On boot:
//   1. Call invoke('first_run_report') to check MAIC provider status.
//   2. If needs_maic_login === true → render login form.
//   3. If logged in → render dashboard with:
//      - Tier badge (Free / Pro / Pro Plus / Team / Enterprise) + token usage
//      - One OpenClaw tile (the only product surface for v1.0.7)
//      - Nudge modal if mc_get_nudge() returns a non-empty text
//
// Dashboard → OpenClaw:
//   Clicking the OpenClaw tile opens the chat workspace in a child window
//   at http://localhost:28789/ via start_gateway_after_login + window.open.

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
    // Already logged in → render the dashboard.
    await renderDashboard();
  }
}

// ---------------------------------------------------------------------------
// Dashboard (v1.0.7)
// ---------------------------------------------------------------------------

async function renderDashboard() {
  // Fetch tier + nudge in parallel. Each is independent and either can
  // fail without blocking the other.
  const [tierResult, nudgeResult] = await Promise.allSettled([
    invoke("mc_get_tier"),
    invoke("mc_get_nudge"),
  ]);

  // If the tier fetch failed with "not logged in", the keychain is empty
  // AND openclaw.json has no api key. Show a sign-in tile instead of a
  // half-rendered dashboard. (Lesson 461 follow-up: previously this
  // rendered with "Unknown" tier and an em-dash usage bar — David had to
  // discover that the tier badge was clickable to escape.)
  if (tierResult.status === "rejected" &&
      String(tierResult.reason).includes("not logged in")) {
    renderLogin("https://maicserver.com");
    return;
  }

  const tier = tierResult.status === "fulfilled" ? tierResult.value : null;
  const nudge = nudgeResult.status === "fulfilled" ? nudgeResult.value : null;

  const tierLabel = tier ? formatTierLabel(tier.tier) : "Unknown";
  const usageText = nudge
    ? `${formatNumber(nudge.used)} / ${formatNumber(nudge.limit)} tokens this period`
    : "—";

  root.innerHTML = `
    <div class="dashboard">
      <header class="dashboard-header">
        <h1 class="logo">MiracleClaw</h1>
        <div class="tier-badge" id="tier-badge" data-tier="${escapeHtml(
          tier?.tier || "free"
        )}" title="Click to refresh tier from MAIC">
          <span class="tier-label">${escapeHtml(tierLabel)}</span>
        </div>
      </header>

      <div class="usage-bar" id="usage-bar" title="Token usage this period">
        <span class="usage-text">${escapeHtml(usageText)}</span>
      </div>

      <div class="tiles">
        <button class="tile tile-primary" id="openclaw-tile" type="button">
          <div class="tile-icon">🦞</div>
          <div class="tile-body">
            <div class="tile-title">OpenClaw</div>
            <div class="tile-description">
              Your MAIC chat workspace. Talk to any MAIC model, run code,
              search the web, work with files. Pro and above unlocks local
              file tools, command execution, and persistent memory.
            </div>
            <div class="tile-cta">Open in new window →</div>
          </div>
        </button>
      </div>

      <div class="dashboard-footer">
        <button class="link-button" id="refresh-tier">Refresh tier</button>
        <button class="link-button" id="signout">Sign out</button>
      </div>
    </div>
  `;

  // Wire interactions
  document.getElementById("openclaw-tile").addEventListener("click", openOpenClaw);
  document.getElementById("tier-badge").addEventListener("click", refreshTier);
  document.getElementById("refresh-tier").addEventListener("click", refreshTier);
  document.getElementById("signout").addEventListener("click", signOut);

  // Tier-changed detection: if the cached last-published tier was higher
  // than the new one, show the downgrade modal.
  if (tier?.tier_changed) {
    showTierChangedModal(tier);
  }

  // Nudge modal: only if there's a non-trivial message.
  if (nudge && nudge.text && nudge.text.length > 0) {
    showNudgeModal(nudge);
  }
}

function formatTierLabel(tier) {
  switch (tier) {
    case "free": return "Free";
    case "pro": return "Pro";
    case "pro_plus": return "Pro Plus";
    case "team": return "Team";
    case "enterprise": return "Enterprise";
    default: return tier || "Unknown";
  }
}

function formatNumber(n) {
  if (n == null) return "—";
  return Number(n).toLocaleString("en-US");
}

async function openOpenClaw() {
  const tile = document.getElementById("openclaw-tile");
  if (tile) {
    tile.disabled = true;
    const label = tile.querySelector(".tile-cta") || tile;
    label.dataset.oldText = label.dataset.oldText ?? label.textContent;
    label.textContent = "Starting…";
  }
  try {
    // Make sure the gateway is up. Cheap no-op if already running.
    const gw = await invoke("start_gateway_after_login");
    console.log("[dashboard] gateway ready:", gw);

    // Spawn (or focus) the dedicated OpenClaw chat webview window.
    // Lesson 461: do NOT use window.open() — Tauri's main webview silently
    // blocks popups and the dashboard would lose its place. The Tauri
    // command creates a sibling webview window that lives independently
    // of the dashboard.
    const result = await invoke("openclaw_open_window");
    console.log("[dashboard] openclaw window:", result);
  } catch (err) {
    console.error("[dashboard] could not open OpenClaw:", err);
    showFatal(`Could not open OpenClaw: ${escapeHtml(err)}`);
  } finally {
    if (tile) {
      tile.disabled = false;
      const label = tile.querySelector(".tile-cta") || tile;
      if (label.dataset.oldText) {
        label.textContent = label.dataset.oldText;
        delete label.dataset.oldText;
      }
    }
  }
}

async function refreshTier() {
  try {
    await invoke("mc_refresh_tier");
    await renderDashboard();
  } catch (err) {
    // Common case: not logged in. The Tauri command returns "not logged in".
    if (String(err).includes("not logged in")) {
      renderLogin("https://maicserver.com");
    } else {
      showFatal(`Could not refresh tier: ${escapeHtml(err)}`);
    }
  }
}

async function signOut() {
  if (!confirm("Sign out of MiracleClaw? Your chat history in OpenClaw will remain, but you'll need to log in again next time.")) {
    return;
  }
  try {
    await invoke("maic_logout");
    renderLogin("https://maicserver.com");
  } catch (err) {
    showFatal(`Could not sign out: ${escapeHtml(err)}`);
  }
}

function showTierChangedModal(tierInfo) {
  // Show a single-shot modal. Free is the only downgradable state for
  // the user's perspective (MC's `has_local_tools()` flips to false).
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal">
      <h2>Your subscription changed</h2>
      <p>Your MAIC tier is now <strong>${escapeHtml(formatTierLabel(tierInfo.tier))}</strong>. Some tools are no longer available.</p>
      <p class="muted small">Local file tools and bash execution require Pro or above.</p>
      <div class="modal-actions">
        <button type="button" id="tier-modal-dismiss">Got it</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);
  document.getElementById("tier-modal-dismiss").addEventListener("click", () => {
    overlay.remove();
  });
}

function showNudgeModal(nudge) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  const cta = nudge.kind === "free_cap" || nudge.kind === "paid_100"
    ? `<a href="https://milagrocloud.com/upgrade" target="_blank" rel="noopener" class="cta">Upgrade to Pro →</a>`
    : "";
  overlay.innerHTML = `
    <div class="modal">
      <h2>${escapeHtml(nudge.kind === "free_cap" || nudge.kind === "paid_100" ? "You've hit your limit" : "Token usage update")}</h2>
      <p>${escapeHtml(nudge.text)}</p>
      ${cta}
      <div class="modal-actions">
        <button type="button" id="nudge-modal-dismiss">${cta ? "Maybe later" : "Got it"}</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);
  document.getElementById("nudge-modal-dismiss").addEventListener("click", () => {
    overlay.remove();
  });
}

// ---------------------------------------------------------------------------
// Login form (unchanged from v1.0.6, just retained here)
// ---------------------------------------------------------------------------

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
    // Lesson 458 / v1.0.6: "Stay signed in" checkbox. Default UNCHECKED
    // (explicit opt-in). When checked, MC encrypts email + password into
    // the OS keychain so the OpenClaw child window can silently mint a
    // new JWT when its session expires. When unchecked, any prior
    // keychain stash is wiped.
    const remember = document.getElementById("login-remember").checked;

    try {
      const result = await invoke("maic_login", { email, password, remember });
      // Login OK + JWT written to openclaw.json + env. Now render the
      // dashboard (no more "redirect to OpenClaw" — post-login is the
      // dashboard per v1.1.0 plan).
      submit.textContent = "Loading dashboard…";
      await renderDashboard();
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

  // v1.0.9 (David 11:20 MDT): New users installing MC need a way to
  // create an account without hunting through footer text. Open the
  // milagrocloud registration page in the OS default browser (NOT inside
  // the Tauri webview — we don't want to leak the dashboard's webview to
  // a third-party site). The user comes back to MC after signup, signs in,
  // done.
  document.getElementById("register-btn").addEventListener("click", async () => {
    try {
      await invoke("open_register_url", {
        url: "https://milagrocloud.com/register",
      });
    } catch (e) {
      console.error("[MiracleClaw] could not open register URL:", e);
      errorEl.textContent =
        "Could not open your browser automatically. Please visit " +
        "milagrocloud.com/register to create an account.";
      errorEl.hidden = false;
    }
  });

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