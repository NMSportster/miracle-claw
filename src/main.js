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
// Lesson 571 (2026-08-25 00:37 MDT, David): MC Module Framework —
// voice module wires its UI hooks (terminal toolbar, fullscreen
// overlay, dashboard FAB) to the live `mc:module-installed` event
// here. See modules-runtime.js for the activation logic.
import { initModuleRuntime, isModuleInstalled, invokeModule } from "./modules-runtime.js";
import { loginPage } from "./pages/login.js";
import { dashboardPage } from "./pages/dashboard.js";
import { settingsPage } from "./pages/settings.js";
import { terminalPage } from "./pages/terminal.js";
import { filesPage } from "./pages/files.js";
import { notebookPage } from "./pages/notebook.js";
import { secretsPage } from "./pages/secrets.js";
// rc53.8 (feature/extras-hub): hub page listing all mlg-* commands
// with a "Run in Terminal" action per card. Also exposes a palette
// action per command for keyboard-driven access.
import { extrasPage, extrasPaletteActions } from "./pages/extras.js";
// Lesson 564 (2026-08-24 17:30 MDT, David): in-app Plans & Pricing
// page. Lives at its own route; the dashboard shows only a tiny
// "Plans & Pricing ↗" button that navigates here. This page renders
// the same 6 plans as the web /pricing page, fetched live from MAIC,
// and each paid-plan CTA opens Stripe Checkout in the OS browser.
import { pricingPage } from "./pages/pricing.js";
// rc53 (feature/secrets-vault): debug page for v0 verification.
// Not registered in the main page map; mounted via window.__mc_openSecretsDebug.
import { secretsDebugPage } from "./secrets/debug_page.js";
// rc49: centralized navigation so the Cmd-K palette (and future deep
// links / keyboard shortcuts) can jump between pages without
// re-implementing the per-page context dance.
import { installNavigation, navigate } from "./navigation.js";
import { initPalette, enable as enablePalette, disable as disablePalette } from "./cmd_k_palette.js";

const { invoke } = window.__TAURI__.core;
const root = document.getElementById("root");

// --- Page registration ----------------------------------------------------
// New pages just call register(...) here. main.js doesn't change again
// until we add a router with deep links.

register("login", loginPage);
register("dashboard", dashboardPage);
register("settings", settingsPage);
register("terminal", terminalPage);
register("files", filesPage);
register("notebook", notebookPage);
// rc53.5: secrets page registered for palette use; toolbar in
// terminal page opens it as an overlay. Not in main nav (yet).
register("secrets", secretsPage);
// rc53.8 (feature/extras-hub): Extras hub — mlg-* companion CLIs.
// Reachable via dashboard tile or Cmd-K palette.
register("extras", extrasPage);
// Lesson 564: in-app Plans & Pricing page. Reachable via the small
// "Plans & Pricing ↗" button on the dashboard, or via Cmd-K palette.
register("pricing", pricingPage);
// rc53 (feature/secrets-vault): debug page registration. Mounted
// only via window.__mc_openSecretsDebug() (dev escape hatch).
register("secrets-debug", secretsDebugPage);

// rc49: install the central navigation map. Every entry takes the
// same shape: (extras) -> ctx object, where ctx carries the page's
// callbacks. The dashboard ctx is the canonical "home" ctx; other
// pages reuse its needsLogin bounce but expose their own back target.
installNavigation({
  root,
  mount: mountPage,
  builders: new Map([
    [
      "dashboard",
      () => pageCtx(),
    ],
    [
      "settings",
      () => settingsCtx(),
    ],
    [
      "files",
      () => filesCtx(),
    ],
    [
      "notebook",
      () => notebookCtx(),
    ],
    [
      "terminal",
      (extras) => terminalCtx(extras || {}),
    ],
    [
      "extras",
      () => extrasCtx(),
    ],
    [
      "pricing",
      () => pricingCtx(),
    ],
    [
      "login",
      (extras) => ({
        endpoint: (extras && extras.endpoint) || "https://maicserver.com",
        onSuccess: () => navigate("dashboard"),
      }),
    ],
  ]),
});

// rc49: install the global Ctrl+K (and Cmd+K on macOS) palette. It
// attaches its own keyboard listener and renders the palette
// overlay into document.body. Page navigation happens through the
// navigate() helper above.
initPalette({
  open: () => {
    // openPalette() returns a Promise that resolves to the action's
    // return value (or undefined if Esc). The palette handles its own
    // UI; this function is intentionally a no-op stub so initPalette's
    // initial test path stays simple.
  },
  navigate,
  runAction: (fn) => fn(),
});

// --- Boot -----------------------------------------------------------------

async function boot() {
  // Lesson 571: wire the module runtime first so mc:module-installed
  // events arriving during boot() aren't dropped on the floor.
  await initModuleRuntime();

  let report;
  try {
    report = await invoke("first_run_report");
  } catch (e) {
    showFatal(`Could not check MC status: ${String(e)}\n\nTry restarting MC.`);
    return;
  }

  if (report.needs_maic_login) {
    // Not logged in — palette stays disabled (Ctrl+K is a no-op until
    // auth completes). The palette overlay would steal clicks from
    // the login form, so we don't enable it here.
    disablePalette();
    mountLogin();
  } else {
    // Already authenticated (returning user). Palette is on.
    enablePalette();
    // rc53.9 (Lesson 243): if the page loaded with `#mcAutoOpen=<key>`
    // in the URL hash, route straight to the Terminal page with the
    // hash as ctx extras. The hash is consumed and cleared by
    // terminal.js mount(). Used by the OpenClaw chat MC-PATCH
    // toolbar buttons to deep-link into 🔑 Secrets / 📎 Attach
    // overlays on return from the chat.
    const hashMatch = (window.location.hash || "").match(
      /mcAutoOpen=(secrets|attach)/
    );
    if (hashMatch) {
      navigate("terminal", { autoOpenOverlay: hashMatch[1] });
    } else {
      mountPage("dashboard", root, pageCtx());
    }
  }
}

// Single source of truth for bouncing to the login page. Used by the
// initial boot and by every onNeedsLogin in every per-page ctx.
// Disables the palette so the overlay can't steal clicks from the
// login form (rc49 bug).
function mountLogin(extras) {
  disablePalette();
  mountPage("login", root, {
    endpoint: (extras && extras.endpoint) || "https://maicserver.com",
    onSuccess: () => {
      enablePalette();
      mountPage("dashboard", root, pageCtx());
    },
  });
}

// Expose mountLogin on window so the Cmd-K palette's "Sign out"
// command can bounce to login without re-implementing the disable-
// palette logic. Set up after installNavigation so it's available
// before any palette action could possibly fire.
window.__mc_mountLogin = mountLogin;

// rc53 (feature/secrets-vault): dev-only escape hatch to open the
// secrets debug page. Used during v0 verification. Will be removed
// once the dashboard tile + UI flow ship in rc54.
window.__mc_openSecretsDebug = function () {
  mountPage("secrets-debug", root, {
    onBackToDashboard: () => navigate("dashboard"),
  });
};

function pageCtx() {
  return {
    onNeedsLogin: mountLogin,
    // rc49: route every page navigation through navigate() so the
    // palette, dashboard tiles, and back buttons share one source of
    // truth. The page builders in installNavigation above own the
    // per-page ctx shape (e.g. terminalCtx with defaultShell).
    onOpenSettings: () => navigate("settings"),
    onOpenTerminal: () => navigate("terminal"),
    onOpenFiles: () => navigate("files"),
    onOpenNotebook: () => navigate("notebook"),
    onOpenExtras: () => navigate("extras"),
    // Lesson 564: dashboard's "Plans & Pricing ↗" button routes
    // here so the in-app pricing page renders the 6 plans + Stripe
    // CTAs (not a 6-card grid jammed into the dashboard).
    onOpenPricing: () => navigate("pricing"),
    // rc53.8 (feature/extras-hub): hand off to Terminal with a
    // pre-filled command + the right shell for the OS. cmd on
    // Windows, bash on Linux/macOS — both can resolve mlg-* from
    // PATH.
    onOpenTerminalWithCommand: (command, shell) =>
      navigate("terminal", { defaultShell: shell, initialCommand: command }),
    // Open the Terminal page with the OpenClaw TUI (mc-openclaw) pre-selected.
    // Used by the "OpenClaw · Terminal" tile on the dashboard.
    onOpenOpenClawTerminal: () =>
      navigate("terminal", { defaultShell: "mc-openclaw" }),
  };
}

function filesCtx() {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
  };
}

function notebookCtx() {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
  };
}

function settingsCtx() {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
  };
}

function terminalCtx(opts = {}) {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
    // Optional override for the shell the page boots with. Used when the
    // dashboard launches OpenClaw directly into the TUI. Falls back to the
    // user-saved default (localStorage) when not provided.
    defaultShell: opts.defaultShell || undefined,
  };
}

// rc53.8 (feature/extras-hub): ctx for the Extras hub page. Mirrors
// the other top-level page ctxs (back-to-dashboard, onNeedsLogin) and
// also wires the per-card "Run in Terminal" handoff.
//
// Why this was missed in rc53.8: the page was registered
// (register('extras', extrasPage)) and the dashboard button was
// wired to onOpenExtras -> navigate('extras'), but the navigation
// builders map below didn't get an "extras" entry. So clicks landed
// in navigate() which bailed with `no builder for page 'extras'` and
// the dashboard didn't change. Symptom: button does nothing.
function extrasCtx() {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
    // Mirrors pageCtx(). Extras cards call this with the command
    // name + shell so terminal.js can pre-fill the prompt.
    onOpenTerminalWithCommand: (command, shell) =>
      navigate("terminal", { defaultShell: shell, initialCommand: command }),
    onOpenTerminal: () => navigate("terminal"),
  };
}

// Lesson 564 (2026-08-24 17:30 MDT, David): minimal ctx for the
// in-app Pricing page. Just back-to-dashboard + onNeedsLogin. The
// page handles Stripe checkout itself via `mc_open_checkout_url`;
// no ctx extras needed.
function pricingCtx() {
  return {
    onBackToDashboard: () => navigate("dashboard"),
    onNeedsLogin: mountLogin,
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
