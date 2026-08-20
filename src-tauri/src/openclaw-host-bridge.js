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
      try {
        // Tauri 2.x with `withGlobalTauri: true` exposes `invoke` at
        // `window.__TAURI__.core.invoke` (NOT `window.__TAURI__.invoke`).
        // Lesson 491 bug: the rc13 bridge originally called
        // `window.__TAURI__.invoke` which is undefined → click silently
        // did nothing. Use the canonical path the dashboard's own
        // `src/main.js` uses.
        const tauri = window.__TAURI__;
        const invoke = tauri && tauri.core && typeof tauri.core.invoke === 'function'
          ? tauri.core.invoke.bind(tauri.core)
          : (typeof tauri?.invoke === 'function' ? tauri.invoke.bind(tauri) : null);
        if (!invoke) {
          console.error('[mc-host-bridge] no Tauri invoke() found on window.__TAURI__', tauri);
          return;
        }
        await invoke('openclaw_back_to_dashboard');
      } catch (e) {
        console.error('[mc-host-bridge] back-to-dashboard invoke failed', e);
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
