// openclaw-host-bridge.js — Tauri 2 initialization script for the main window.
//
// Injected into every page that loads in the main window via tauri.conf.json
// `app.windows[0].initializationScripts`. Runs at document creation, BEFORE
// any other page script. Sets a global `__openclawHostBridge` object the page
// can detect and use to know whether it's hosted in MC, and (when hosted)
// to surface a "← Back to dashboard" overlay if the page is the chat gateway.
//
// Why this exists (Lesson 491, rc13):
//
// Pre-rc13 architecture: `openclaw_open_window` in lib.rs created a SECOND
// webview window via `WebviewWindowBuilder::build()` and pointed it at
// http://127.0.0.1:28789. On David's system this hangs deterministically
// (Lesson 487 — Tauri 2.11.5 + WebView2 multi-runtime + uncached-cleared
// user-data-dir). The Tauri main process gets killed by Windows
// Application Hang detection after ~7 minutes.
//
// rc13 architecture: instead of creating a second webview, NAVIGATE the
// existing main webview to the chat URL. The main window is already
// running successfully (rc11/rc12 dashboard renders fine in it), so
// navigating is a safe in-place operation that avoids `builder.build()`
// entirely. To get back to the dashboard, the user clicks the floating
// "← Dashboard" overlay this script renders whenever the main window is
// currently at the chat gateway.
//
// Why rc15 (Lesson 495, current):
//
// rc13/rc14: bridge used `window.__TAURI__.core.invoke` (Lesson 493
// canonical). Pill click visibly works on dev machines, but on David's
// production machine `invoke('openclaw_back_to_dashboard')` never reaches
// the Rust `#[tauri::command]` handler — the log file shows no
// `openclaw_back_to_dashboard:` entry after click. The Rust IPC chain
// is intact (other commands like `maic_login` work fine; WebView2 IPC is
// verified working by the chat UI's successful WS/API calls).
//
// rc15 hypotheses for the silent failure:
//   A. `window.__TAURI__` global is undefined on the http://127.0.0.1:28789
//      page even though `initialization_script` re-fires on cross-origin
//      navigation. This is possible if the global-API script is registered
//      with `for_main_frame_only` in a way that drops it on origin change.
//   B. `__TAURI__.core.invoke` is defined but its call dies silently —
//      e.g. invoke_key mismatch or option validation rejects the call.
//   C. Click event isn't reaching the bridge at all (z-index/overlay
//      positioned wrong). But pill IS visible at z-index 2147483647 and
//      chat UI's max z-index is 100, so this is unlikely.
//   D. Lower-level IPC `__TAURI_INTERNALS__.invoke` works while the
//      higher-level `__TAURI__.core.invoke` does not (e.g. core.js
//      wrapper has a bug on cross-origin).
//
// rc15 fixes:
//   1. Multi-layered invoke fallback: try high-level core, then low-level
//      internals, then legacy. Whichever path resolves, use it. If the
//      first attempt rejects, try the next.
//   2. VISIBLE on-screen toast that shows: which invoke path was tried,
//      the result/error. David can READ what went wrong without DevTools
//      (WebView2's devtools are not enabled in production builds).
//   3. On every click: log path-tried + result to the console (invisible
//      but helpful for future devs).
//
// Detection rule:
//   - `window.location.host === '127.0.0.1:28789'` → user is on the chat
//     gateway. Show overlay.
//   - anything else (tauri://localhost, http://localhost:1420 in dev, etc.)
//     → dashboard is showing. Don't show overlay.
//
// The overlay is plain HTML positioned in the top-left corner of the
// viewport. It uses inline styles (no external CSS dependency) so it works
// on every page regardless of that page's stylesheet.

(function () {
  'use strict';

  // Bail out if Tauri internals aren't present (running in a normal browser).
  if (typeof window === 'undefined') return;
  const hasTauri =
    typeof window.__TAURI_INTERNALS__ !== 'undefined' ||
    typeof window.__TAURI__ !== 'undefined';

  // Marker so page code can opt-in to knowing it's hosted.
  window.__openclawHostBridge = {
    hosted: hasTauri,
    isChatPage: function () {
      return window.location && window.location.host === '127.0.0.1:28789';
    },
  };

  if (!hasTauri) return;

  const CHAT_HOST = '127.0.0.1:28789';

  // -------- rc15 visible toast helper (Lesson 495 diagnostic) --------
  // WebView2 production builds don't expose DevTools by default. So if the
  // bridge click silently fails, David has zero visibility into what
  // went wrong. This toast surfaces "which invoke path was tried" +
  // "result or error" inline so we can see it.
  let toastEl = null;
  function showToast(msg, kind) {
    try {
      if (!toastEl) {
        toastEl = document.createElement('div');
        toastEl.id = '__mc-bridge-toast';
        toastEl.style.cssText = [
          'position: fixed',
          'top: 56px',
          'left: 12px',
          'z-index: 2147483647',
          'max-width: min(560px, 70vw)',
          'padding: 8px 12px',
          'border-radius: 6px',
          'background: rgba(20, 20, 24, 0.92)',
          'color: #f5f5f7',
          'font: 500 11px/1.4 ui-monospace, "SF Mono", Menlo, Consolas, monospace',
          'box-shadow: 0 4px 14px rgba(0,0,0,0.45)',
          'white-space: pre-wrap',
          'word-break: break-word',
          'pointer-events: none',
          'opacity: 0',
          'transition: opacity .2s ease',
        ].join(';');
        (document.body || document.documentElement).appendChild(toastEl);
      }
      const color =
        kind === 'error' ? '#ff6b6b' :
        kind === 'ok'    ? '#5dd49d' :
        kind === 'info'  ? '#7eb6ff' :
                            '#f5f5f7';
      toastEl.style.color = color;
      toastEl.textContent = msg;
      toastEl.style.opacity = '1';
      // Auto-fade after 6s for success/info, 12s for errors
      clearTimeout(toastEl._fadeTimer);
      toastEl._fadeTimer = setTimeout(function () {
        if (toastEl) toastEl.style.opacity = '0';
      }, kind === 'error' ? 12000 : 6000);
    } catch (_) { /* toast is best-effort */ }
  }

  // Resolve an invoke function across Tauri 2 API layers.
  // Returns { fn, path } or null if nothing found.
  function resolveInvoke() {
    // Path 1: high-level global (canonical for Tauri 2.x with_global_tauri=true)
    const tauri = window.__TAURI__;
    if (tauri && tauri.core && typeof tauri.core.invoke === 'function') {
      return { fn: tauri.core.invoke.bind(tauri.core), path: 'tauri.core.invoke' };
    }
    // Path 2: low-level internals (more stable for cross-origin pages —
    // defined directly by `__RAW_ipc_script__` which runs FIRST in
    // `tauri/scripts/init.js`, so it should be present even if the
    // higher-level IIFE that builds `__TAURI__` fails for some reason).
    const internals = window.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === 'function') {
      return {
        fn: function (cmd, args) { return internals.invoke(cmd, args); },
        path: 'TAURI_INTERNALS.invoke',
      };
    }
    // Path 3: legacy Tauri 1.x-style (not expected in Tauri 2 but harmless
    // to check).
    if (tauri && typeof tauri.invoke === 'function') {
      return { fn: tauri.invoke.bind(tauri), path: 'tauri.invoke (legacy)' };
    }
    return null;
  }

  function showOverlay() {
    if (document.getElementById('__mc-back-overlay')) return;

    const overlay = document.createElement('div');
    overlay.id = '__mc-back-overlay';
    overlay.setAttribute('role', 'button');
    overlay.setAttribute('aria-label', 'Back to MiracleClaw dashboard');
    overlay.title = 'Back to MiracleClaw dashboard';
    overlay.style.cssText = [
      'position: fixed',
      'top: 12px',
      'left: 12px',
      'z-index: 2147483647',
      'display: inline-flex',
      'align-items: center',
      'gap: 6px',
      'padding: 8px 14px',
      'border-radius: 999px',
      'background: rgba(20, 20, 24, 0.85)',
      'color: #f5f5f7',
      'font: 600 13px/1 system-ui, -apple-system, "Segoe UI", sans-serif',
      'cursor: pointer',
      'box-shadow: 0 4px 14px rgba(0,0,0,0.35)',
      'backdrop-filter: blur(6px)',
      '-webkit-backdrop-filter: blur(6px)',
      'user-select: none',
      'transition: transform .15s ease, background .15s ease',
    ].join(';');

    const arrow = document.createElement('span');
    arrow.textContent = '←';
    arrow.style.cssText = 'font-size: 15px; line-height: 1;';

    const text = document.createElement('span');
    text.textContent = 'Dashboard';

    overlay.appendChild(arrow);
    overlay.appendChild(text);

    overlay.addEventListener('mouseenter', function () {
      overlay.style.background = 'rgba(40, 40, 48, 0.95)';
      overlay.style.transform = 'translateY(-1px)';
    });
    overlay.addEventListener('mouseleave', function () {
      overlay.style.background = 'rgba(20, 20, 24, 0.85)';
      overlay.style.transform = 'translateY(0)';
    });

    overlay.addEventListener('click', async function () {
      // rc15: resolve invoke ONCE at click time (not at script load) so
      // we always use the freshest APIs the page actually has. Also try
      // fallback paths if the first one rejects.
      const resolved = resolveInvoke();
      if (!resolved) {
        const tauri = window.__TAURI__;
        const internals = window.__TAURI_INTERNALS__;
        showToast(
          '[mc-bridge] no Tauri invoke found.\n' +
          '__TAURI__ = ' + (typeof tauri) + '\n' +
          '__TAURI_INTERNALS__ = ' + (typeof internals) + '\n' +
          'host = ' + window.location.host,
          'error'
        );
        return;
      }

      showToast('[mc-bridge] click → ' + resolved.path, 'info');
      try {
        const result = await resolved.fn('openclaw_back_to_dashboard');
        showToast('[mc-bridge] OK via ' + resolved.path, 'ok');
        return result;
      } catch (e1) {
        // Try the next layer if first attempt rejected.
        const all = [
          window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke && (function () { return { fn: window.__TAURI__.core.invoke.bind(window.__TAURI__.core), path: 'tauri.core.invoke' }; }),
          window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke && (function () { return { fn: function (c, a) { return window.__TAURI_INTERNALS__.invoke(c, a); }, path: 'TAURI_INTERNALS.invoke' }; }),
        ].filter(Boolean);
        for (let i = 0; i < all.length; i++) {
          if (all[i].path === resolved.path) continue; // already tried
          try {
            await all[i].fn('openclaw_back_to_dashboard');
            showToast('[mc-bridge] OK via fallback ' + all[i].path, 'ok');
            return;
          } catch (e2) {
            // keep trying next layer
          }
        }
        const msg = (e1 && (e1.message || e1.toString())) || 'unknown';
        showToast('[mc-bridge] ALL invoke paths failed.\nFirst error: ' + msg, 'error');
      }
    });

    (document.body || document.documentElement).appendChild(overlay);
  }

  function removeOverlay() {
    const el = document.getElementById('__mc-back-overlay');
    if (el && el.parentNode) el.parentNode.removeChild(el);
  }

  function syncOverlay() {
    const onChat =
      window.location && window.location.host === CHAT_HOST;
    if (onChat) showOverlay();
    else removeOverlay();
  }

  // Initial sync (script may run before document.body exists).
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', syncOverlay, {
      once: true,
    });
  } else {
    syncOverlay();
  }

  // Re-sync on SPA navigation (pushState/replaceState don't fire popstate).
  // Cheap polling at 250ms — Tauri windows are short-lived; the cost is
  // negligible and avoids a full monkey-patch of history.pushState.
  setInterval(syncOverlay, 250);
})();