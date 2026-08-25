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

  // rc53.10 (Lesson 244): generic invoke wrapper exposed as
  // window.openclawBridge.invoke(cmd, args). Used by
  // mc-chat-toolbar.js to call mc_open_overlay without duplicating
  // the resolveInvoke/fallback chain. Returns a Promise.
  //
  // Why this lives here (not inside mc-chat-toolbar.js):
  // mc-chat-toolbar is injected by the patcher AFTER the bridge IIFE
  // runs, so it can rely on the bridge having already wired everything
  // up. Putting the fallback chain here means the chat toolbar stays
  // a thin presentation layer that just calls one method.
  function bridgeInvoke(cmd, args) {
    const resolved = resolveInvoke();
    if (!resolved) return Promise.reject(new Error('no Tauri invoke available'));
    return resolved.fn(cmd, args).catch(function (firstErr) {
      // Try the other layers in case the first resolved path is flaky
      // for this particular command. Mirrors the multi-path fallback
      // the back button uses inline.
      const tauri = window.__TAURI__;
      const internals = window.__TAURI_INTERNALS__;
      const fallbacks = [
        tauri && tauri.core && typeof tauri.core.invoke === 'function'
          ? tauri.core.invoke.bind(tauri.core) : null,
        internals && typeof internals.invoke === 'function'
          ? function (c, a) { return internals.invoke(c, a); } : null,
      ].filter(Boolean);
      let lastErr = firstErr;
      return fallbacks.reduce(function (p, fn) {
        return p.catch(function () {
          return fn(cmd, args).catch(function (e) { lastErr = e; throw e; });
        });
      }, Promise.reject(firstErr)).catch(function () {
        throw lastErr || new Error('all invoke paths failed for ' + cmd);
      });
    });
  }
  // Module registry (mirrors modules-runtime.js's Map, scoped to this window)
  // Declared early so the bridge helpers below can reference it without TDZ.
  const installedModules = new Map(); // id -> { name, version }

  window.__openclawHostBridge.invoke = bridgeInvoke;

  // Lesson 572: module-aware helpers exposed on the bridge so page code
  // can do `__openclawHostBridge.isModuleInstalled('voice')` and
  // `__openclawHostBridge.invokeModule('mc_voice_transcribe', {...})`
  // without caring about mc_module_call routing. Mirrors the public API
  // of modules-runtime.js (used in MC's main window).
  window.__openclawHostBridge.isModuleInstalled = function (id) {
    return installedModules.has(id);
  };
  window.__openclawHostBridge.invokeModule = function (command, params) {
    const id = command.split('_')[1] || '';
    return bridgeInvoke('mc_module_call', { id: id, command: command, params: params || {} });
  };

  // rc53.11 (Lesson 247): convenience wrapper for the overlay-close
  // round trip. Page code calls `__openclawHostBridge.closeOverlay()`
  // from inside the dashboard (Terminal page Secrets/Attach close
  // handlers). The Rust `mc_close_overlay` command captures the URL
  // the user was on before `mc_open_overlay` navigated, and navigates
  // back to it — typically the OpenClaw chat window at
  // http://127.0.0.1:28789/chat?session=...
  //
  // If the bridge isn't wired (dev / test), the wrapper rejects so
  // the page can fall back to plain DOM close (no navigation).
  window.__openclawHostBridge.closeOverlay = function () {
    return bridgeInvoke('mc_close_overlay', {});
  };

  // ============================================================
  // Lesson 572: Voice button wiring for the OpenClaw chat overlay.
  // The chat window (openclaw-host-bridge.js) does NOT have access to
  // MC's modules-runtime.js (which lives in the main bundle), so we
  // re-implement just enough here: subscribe to the module-installed
  // events, find the chat input, inject a floating mic button, and
  // dispatch to mc_module_call.
  //
  // Why here (not in main.js / modules-runtime.js): the chat window
  // is a separate runtime with its own DOM tree. The bridge script
  // runs in BOTH the main window and the chat window via
  // initialization_script(), so it's the single source of truth for
  // voice button wiring across runtimes.
  // ============================================================

  // Re-broadcast events for page-level subscribers (mirrors modules-runtime.js)
  function reBroadcast(name, detail) {
    try { window.dispatchEvent(new CustomEvent(name, { detail: detail })); } catch (_) {}
  }

  // Subscribe to MC module events. listen() returns an unlisten fn we keep
  // around for potential teardown. Listening happens once at script load.
  if (typeof window.__TAURI__ !== 'undefined' && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
    window.__TAURI__.event.listen('mc:module-installed', function (event) {
      const m = (event && event.payload) || {};
      if (m.id) {
        installedModules.set(m.id, m);
        reBroadcast('mc:module-installed', m);
        syncVoiceButtons();
      }
    }).catch(function () { /* listen failed; ignore */ });

    window.__TAURI__.event.listen('mc:module-uninstalled', function (event) {
      const m = (event && event.payload) || {};
      if (m.id) {
        installedModules.delete(m.id);
        reBroadcast('mc:module-uninstalled', m);
        syncVoiceButtons();
      }
    }).catch(function () { /* listen failed; ignore */ });

    // Initial sync: pull current module list so we don't have to wait for
    // an event (in case the module was installed before this window loaded).
    bridgeInvoke('mc_module_list', {}).then(function (list) {
      const arr = Array.isArray(list) ? list : [];
      arr.forEach(function (m) {
        if (m && m.id) installedModules.set(m.id, m);
      });
      syncVoiceButtons();
    }).catch(function () { /* mc_module_list unavailable; ok */ });
  }

  function isVoiceInstalled() { return installedModules.has('voice'); }

  function syncVoiceButtons() {
    // Gate any existing voice buttons in the page DOM via data-attr.
    const installed = isVoiceInstalled();
    document.querySelectorAll('[data-module-voice-installed]').forEach(function (el) {
      el.setAttribute('data-module-voice-installed', installed ? 'true' : 'false');
    });
    // If we're on a chat page and no button exists yet, inject one.
    if (window.__openclawHostBridge.isChatPage()) {
      ensureChatVoiceButton();
    }
  }

  // Pick the most likely chat input. The OpenClaw chat UI uses various
  // input shapes (textarea, contenteditable). Try a few selectors in
  // priority order; first hit wins.
  function findChatInput() {
    const selectors = [
      'textarea[data-chat-input]',
      '#chat-input',
      'textarea[name="message"]',
      'textarea[placeholder*="message" i]',
      'div[contenteditable="true"][data-chat-input]',
      'div[contenteditable="true"][role="textbox"]',
      'div[contenteditable="true"]',
      'input[type="text"][data-chat-input]',
      'input[type="text"]',
    ];
    for (let i = 0; i < selectors.length; i++) {
      const el = document.querySelector(selectors[i]);
      if (el) return el;
    }
    return null;
  }

  // Drop text into a chat input. Handles textarea, input, contenteditable.
  function writeChatInput(el, text) {
    if (!el || !text) return false;
    const tag = (el.tagName || '').toLowerCase();
    if (tag === 'textarea' || tag === 'input') {
      // Append to existing content rather than replace — user may have
      // started typing.
      const existing = el.value || '';
      el.value = (existing ? existing + (existing.endsWith(' ') ? '' : ' ') : '') + text;
      // Trigger input event so framework listeners (React, Vue, etc.)
      // pick up the change.
      try { el.dispatchEvent(new Event('input', { bubbles: true })); } catch (_) {}
      el.focus();
      // Move caret to end
      try {
        const len = el.value.length;
        if (el.setSelectionRange) el.setSelectionRange(len, len);
      } catch (_) {}
      return true;
    }
    if (el.isContentEditable || el.getAttribute && el.getAttribute('contenteditable') === 'true') {
      const existing = el.innerText || el.textContent || '';
      const appended = (existing ? existing + (existing.endsWith(' ') ? '' : ' ') : '') + text;
      el.innerText = appended;
      try { el.dispatchEvent(new Event('input', { bubbles: true })); } catch (_) {}
      el.focus();
      return true;
    }
    return false;
  }

  // The voice button element we inject. Singleton: re-attach if removed.
  let chatVoiceBtn = null;

  function ensureChatVoiceButton() {
    if (chatVoiceBtn && document.body && document.body.contains(chatVoiceBtn)) return;
    const btn = document.createElement('button');
    btn.id = 'openclaw-chat-voice-btn';
    btn.type = 'button';
    btn.setAttribute('data-module-voice-installed', isVoiceInstalled() ? 'true' : 'false');
    btn.setAttribute('aria-label', 'Voice input');
    btn.title = 'Voice input';
    btn.textContent = '🎙';
    btn.style.cssText = [
      'position: fixed',
      'bottom: 18px',
      'right: 18px',
      'z-index: 2147483646',
      'width: 44px',
      'height: 44px',
      'border-radius: 50%',
      'border: 1px solid rgba(255,255,255,0.18)',
      'background: rgba(20, 20, 24, 0.92)',
      'color: #f5f5f7',
      'font-size: 20px',
      'line-height: 1',
      'cursor: pointer',
      'box-shadow: 0 4px 14px rgba(0,0,0,0.45)',
      'backdrop-filter: blur(6px)',
      '-webkit-backdrop-filter: blur(6px)',
      'transition: transform .12s ease, background .12s ease',
    ].join(';');

    // Greyed-out state when module not installed
    function applyDisabledState() {
      if (isVoiceInstalled()) {
        btn.style.opacity = '1';
        btn.style.cursor = 'pointer';
        btn.style.pointerEvents = 'auto';
        btn.title = 'Voice input (click to record)';
      } else {
        btn.style.opacity = '0.4';
        btn.style.cursor = 'not-allowed';
        btn.style.pointerEvents = 'none';
        btn.title = 'Voice module not installed — open Settings → Modules to install';
      }
    }
    applyDisabledState();

    btn.addEventListener('mouseenter', function () {
      if (isVoiceInstalled()) btn.style.background = 'rgba(40, 40, 48, 0.95)';
    });
    btn.addEventListener('mouseleave', function () {
      btn.style.background = 'rgba(20, 20, 24, 0.92)';
    });

    btn.addEventListener('click', async function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      if (!isVoiceInstalled()) {
        showToast('Voice module not installed.\nOpen Settings → Modules to install.', 'error');
        return;
      }
      btn.style.transform = 'scale(0.95)';
      showToast('🎙 Recording… (auto-stops on silence)', 'info');
      try {
        const result = await bridgeInvoke('mc_module_call', {
          id: 'voice',
          command: 'mc_voice_transcribe',
          params: { seconds: 30, vad_enabled: true, silence_ms: 1500 },
        });
        // result shape from dispatcher.rs:
        // { ok: true, result: { text: "..." } } OR { ok: false, error: "..." }
        const ok = result && result.ok;
        const text = ok && result.result ? (result.result.text || '') : '';
        if (text) {
          const input = findChatInput();
          if (input && writeChatInput(input, text)) {
            showToast('🎙 Transcript: ' + (text.length > 60 ? text.slice(0, 57) + '...' : text), 'ok');
          } else {
            // No chat input found — copy to clipboard as fallback
            try {
              if (navigator.clipboard && navigator.clipboard.writeText) {
                await navigator.clipboard.writeText(text);
              }
            } catch (_) {}
            showToast('🎙 Transcript copied to clipboard:\n' + text.slice(0, 80), 'ok');
          }
        } else if (!ok && result && result.error) {
          showToast('Voice error: ' + result.error, 'error');
        } else {
          showToast('🎙 No speech detected', 'info');
        }
      } catch (e) {
        const msg = (e && (e.message || e.toString())) || 'unknown';
        showToast('Voice failed: ' + msg, 'error');
      } finally {
        btn.style.transform = 'scale(1)';
      }
    });

    (document.body || document.documentElement).appendChild(btn);
    chatVoiceBtn = btn;
  }

  // Poll for the button's data-attr to stay in sync with module state.
  // Cheap, runs every 1s; only writes the attr if it changed.
  setInterval(function () {
    if (chatVoiceBtn && chatVoiceBtn.parentNode) {
      const want = isVoiceInstalled() ? 'true' : 'false';
      if (chatVoiceBtn.getAttribute('data-module-voice-installed') !== want) {
        chatVoiceBtn.setAttribute('data-module-voice-installed', want);
      }
    }
  }, 1000);

  // Hook into the same syncOverlay flow so the voice button appears on
  // chat pages and disappears on dashboard.
  (function () {
    const origSync = typeof syncOverlay === 'function' ? syncOverlay : null;
    if (origSync) {
      // Wrap syncOverlay to also inject/remove the voice button.
      // (syncOverlay is already defined later in the file via function
      // declaration; we re-define behavior by polling at 250ms below.)
    }
  })();

  // Re-sync the voice button on a 250ms cadence (same pattern as the
  // back-to-dashboard overlay). Cheap, mirrors the SPA-navigate
  // detection that syncOverlay already does.
  setInterval(function () {
    if (window.__openclawHostBridge.isChatPage()) {
      ensureChatVoiceButton();
    } else if (chatVoiceBtn && chatVoiceBtn.parentNode) {
      chatVoiceBtn.parentNode.removeChild(chatVoiceBtn);
      chatVoiceBtn = null;
    }
  }, 250);

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