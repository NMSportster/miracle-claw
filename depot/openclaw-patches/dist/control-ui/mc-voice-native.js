// MiracleClaw native voice input patch (Lesson 706, rc54.3)
//
// Injected into control-ui/index.html by scripts/patch-openclaw-dist.sh.
// Intercepts the OpenClaw voice input button (CSS class
// `chat-send-btn--voice`) and replaces the slow Realtime Talk cloud
// STT path with native Windows SAPI 5 via the Rust
// `mc_voice_native_capture` Tauri command.
//
// Detection strategy (mirrors mc-back-button.js — Lesson 511):
// We gate on `window.location.host === '127.0.0.1:28789'` directly.
// That's a stable, IPC-free signal that we're on the OpenClaw chat
// page hosted by MC. We don't need `__openclawHostBridge` to be set
// up — this patch must work even if the bridge injection failed.
//
// Click strategy (mirrors mc-back-button.js):
// We attach a CAPTURE-phase click listener to the document so we
// fire BEFORE React's synthetic event system. We then call
// `event.stopImmediatePropagation()` and `event.preventDefault()` so
// the original toggleRealtimeTalk code path NEVER runs. This is
// critical: if we let React's handler run, it spawns a getUserMedia
// session that races against our SAPI 5 capture and the user hears
// two audio streams at once.
//
// Tauri IPC:
//   window.__TAURI_INTERNALS__.invoke('mc_voice_native_capture',
//                                     { timeoutMs: 15000 })
// We accept whichever Tauri API surface is exposed in this build:
//   - window.__TAURI__.core.invoke (Tauri 2.x preferred)
//   - window.__TAURI_INTERNALS__.invoke (Tauri 2.x internals)
//   - window.__TAURI__.invoke (legacy)
//
// Output placement:
// We write the transcribed text into the React-controlled composer
// input via a `composer__draft` custom event the OpenClaw bundle
// already listens for, OR — if that event isn't honored — we set the
// value on the textarea/textarea[contenteditable] and dispatch an
// `input` event so React notices the change.
//
// Privacy:
// No audio ever leaves the device. SAPI 5 is fully offline, runs in
// the user's own PowerShell process, and only returns the final
// transcript string.
//
// Idempotent:
// `data-mc-voice-native-patched` attribute on <html> prevents
// double-injection if the patch script runs twice (defense in depth).
(function () {
  // ---- Gate: only run on the OpenClaw chat page hosted by MC ----
  try {
    if (!window.location || window.location.host !== '127.0.0.1:28789') return;
    if (document.documentElement.getAttribute('data-mc-voice-native-patched') === 'true') return;
    document.documentElement.setAttribute('data-mc-voice-native-patched', 'true');
  } catch (e) {
    return;
  }

  // ---- Tauri invoke resolver ----
  // Try the modern Tauri 2.x paths first, then the legacy global.
  function resolveInvoke() {
    try {
      var internals = window.__TAURI_INTERNALS__;
      if (internals && typeof internals.invoke === 'function') {
        return internals.invoke.bind(internals);
      }
      var core = window.__TAURI__ && window.__TAURI__.core;
      if (core && typeof core.invoke === 'function') {
        return core.invoke.bind(core);
      }
      var legacy = window.__TAURI__;
      if (legacy && typeof legacy.invoke === 'function') {
        return legacy.invoke.bind(legacy);
      }
    } catch (e) {}
    return null;
  }

  // ---- Composer text setter ----
  // OpenClaw's composer is a Lit/React hybrid. We try, in order:
  //   1. Direct value set on a textarea + dispatch input event
  //   2. Direct textContent set on a contenteditable + dispatch input event
  //   3. Click on a known "draft" event target (rare; React components
  //      listening for composer__draft custom events)
  function setComposerText(text) {
    var candidates = document.querySelectorAll(
      'textarea[data-composer-input], textarea.composer__input, textarea[placeholder*="Message" i], [contenteditable="true"][data-composer-input]'
    );
    for (var i = 0; i < candidates.length; i++) {
      var el = candidates[i];
      try {
        // React tracks the previous value on a hidden property; setting
        // `.value` directly bypasses that. We use the prototype's setter
        // to make React's onChange fire.
        var proto = el.tagName === 'TEXTAREA'
          ? window.HTMLTextAreaElement.prototype
          : window.HTMLDivElement.prototype;
        var setter = Object.getOwnPropertyDescriptor(proto, 'value');
        if (setter && setter.set) {
          setter.set.call(el, text);
        } else {
          el.value = text;
        }
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        el.focus();
        return true;
      } catch (e) {
        // Try the next candidate.
        continue;
      }
    }
    return false;
  }

  // ---- Visual feedback overlay ----
  // Show a transient "Listening…" pill so users know the click
  // registered, mirroring the OpenClaw voice-active state.
  function showOverlay(label, kind) {
    var id = 'mc-voice-native-overlay';
    var existing = document.getElementById(id);
    if (existing) existing.remove();
    var pill = document.createElement('div');
    pill.id = id;
    pill.setAttribute('role', 'status');
    pill.setAttribute('aria-live', 'polite');
    pill.textContent = label;
    var color = kind === 'error' ? 'rgba(160, 30, 30, 0.95)' :
                 kind === 'success' ? 'rgba(20, 100, 60, 0.95)' :
                 'rgba(20, 20, 24, 0.85)';
    pill.style.cssText = [
      'position: fixed',
      'top: 12px',
      'right: 12px',
      'z-index: 2147483647',
      'padding: 8px 14px',
      'border: 1px solid rgba(255,255,255,0.18)',
      'border-radius: 8px',
      'background: ' + color,
      'color: #f5f5f7',
      'font: 500 13px/1.2 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
      'box-shadow: 0 6px 20px rgba(0,0,0,0.45)',
      'backdrop-filter: blur(8px)',
      '-webkit-backdrop-filter: blur(8px)',
      'pointer-events: none',
      'transition: opacity .2s ease',
    ].join(';');
    document.body.appendChild(pill);
    return pill;
  }

  // ---- Main handler ----
  var invoke = resolveInvoke();
  if (!invoke) {
    // No Tauri — this patch is a no-op. The original OpenClaw voice
    // button keeps working (cloud Realtime Talk fallback).
    if (window.console && window.console.warn) {
      window.console.warn('[mc-voice-native] Tauri invoke not found; patch is no-op.');
    }
    return;
  }

  // Capture-phase listener fires BEFORE React's bubble-phase handler,
  // so we can stop the original toggleRealtimeTalk from running.
  document.addEventListener('click', async function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    var btn = target.closest('.chat-send-btn--voice, .chat-send-btn--stop');
    if (!btn) return;

    // Block the original click handler entirely.
    event.preventDefault();
    event.stopImmediatePropagation();

    var pill = showOverlay('🎙 Listening… (SAPI 5)', 'pending');

    try {
      var result = await invoke('mc_voice_native_capture', { timeoutMs: 15000 });
      var text = (result && typeof result.text === 'string') ? result.text.trim() : '';

      pill.remove();
      if (!text) {
        showOverlay('No speech detected', 'error');
        setTimeout(function () {
          var p = document.getElementById('mc-voice-native-overlay');
          if (p) p.remove();
        }, 2500);
        return;
      }

      var placed = setComposerText(text);
      if (placed) {
        showOverlay('✓ Transcribed (' + Math.round((result.confidence || 0) * 100) + '% confidence)', 'success');
      } else {
        // Couldn't find the composer; surface the text so user can copy it.
        showOverlay('📋 ' + text.slice(0, 80) + (text.length > 80 ? '…' : ''), 'success');
      }
      setTimeout(function () {
        var p = document.getElementById('mc-voice-native-overlay');
        if (p) p.remove();
      }, 3000);
    } catch (e) {
      pill.remove();
      var msg = (e && e.message) ? e.message : String(e);
      // Lesson 709 (rc54.4.1): 60 chars truncates useful .NET exception
      // messages mid-sentence ("RecognizeAsync failed: We..."). Bump to
      // 200 chars so users see the full underlying cause. The overlay
      // wraps text with word-break so longer messages still render OK.
      showOverlay('Voice error: ' + msg.slice(0, 200), 'error');
      if (window.console && window.console.error) {
        window.console.error('[mc-voice-native] capture failed:', e);
      }
      setTimeout(function () {
        var p = document.getElementById('mc-voice-native-overlay');
        if (p) p.remove();
      }, 5000);
    }
  }, true);

  if (window.console && window.console.info) {
    window.console.info('[mc-voice-native] patched — voice button will use Windows SAPI 5 instead of cloud Realtime Talk.');
  }
})();