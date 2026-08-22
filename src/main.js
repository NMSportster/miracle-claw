// MiracleClaw frontend entry point.
//
// v1.0.9-rc34: page_registry HashMap refactor (David 16:26 MDT — "I have to
// do that before we add any more pages or programs.").
//
// What's here now:
//   - Boot: check MAIC provider status, decide which page to mount.
//   - Registry wiring: login + dashboard pages self-register at import.
//   - Route resolution: needs_login → login page, else dashboard.
//   - Cross-page context: onNeedsLogin callback to bounce back to login.
//
// What's NOT here anymore:
//   - Login form rendering (now in pages/login.js)
//   - Dashboard rendering (now in pages/dashboard.js)
//   - Modals (now scoped to their owning pages)

import "./styles.css";
import { register, mount as mountPage } from "./page_registry.js";
import { loginPage } from "./pages/login.js";
import { dashboardPage } from "./pages/dashboard.js";
import { settingsPage } from "./pages/settings.js";

const { invoke } = window.__TAURI__.core;
const root = document.getElementById("root");

// --- Page registration ----------------------------------------------------
// New pages just call register(...) here. main.js doesn't change again
// until we add a router with deep links.

register("login", loginPage);
register("dashboard", dashboardPage);
register("settings", settingsPage);

// --- Boot -----------------------------------------------------------------

async function boot() {
  let report;
  try {
    report = await invoke("first_run_report");
  } catch (e) {
    showFatal(`Could not check MC status: ${String(e)}\n\nTry restarting MC.`);
    return;
  }

  if (report.needs_maic_login) {
    mountPage("login", root, {
      endpoint: report.maic_provider_endpoint || "https://maicserver.com",
      onSuccess: () => mountPage("dashboard", root, pageCtx()),
    });
  } else {
    mountPage("dashboard", root, pageCtx());
  }
}

function pageCtx() {
  return {
    onNeedsLogin: () =>
      mountPage("login", root, {
        endpoint: "https://maicserver.com",
        onSuccess: () => mountPage("dashboard", root, pageCtx()),
      }),
    onOpenSettings: () => mountPage("settings", root, settingsCtx()),
  };
}

function settingsCtx() {
  return {
    onBackToDashboard: () => mountPage("dashboard", root, pageCtx()),
    onNeedsLogin: () =>
      mountPage("login", root, {
        endpoint: "https://maicserver.com",
        onSuccess: () => mountPage("dashboard", root, pageCtx()),
      }),
  };
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

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

boot();
