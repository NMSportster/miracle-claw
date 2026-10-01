// MiracleClaw voice input patch (Lesson 1001, rc57.1)
//
// Replaces the Lesson 706 SAPI 5 path with the cross-platform
// whisper.cpp sidecar (`miracle-claw-voice` module). Works on every
// Windows install regardless of SAPI 5 health, and is the same code
// path as the dashboard [mic] FAB and Terminal voice button (Lesson 581/592).
//
// Flow:
//   1. User clicks `.chat-send-btn--voice` (OpenClaw's native voice button).
//   2. We intercept with a CAPTURE-phase listener and stop the original
//      slow Realtime Talk cloud STT handler from running.
//   3. `mc_voice_check` -> if model_loaded=false, auto-download via
//      `mc_voice_download_model` (~75MB first-time). Show progress pill.
//   4. `mc_voice_transcribe` (via `mc_module_call`) -> cpal captures
//      audio + webrtc-vad auto-stops on silence + whisper.cpp
//      returns transcript.
//   5. Paste transcript into the React composer (textarea prototype setter
//      + dispatched input event so React notices).
//
// Why dispatch through `mc_module_call` and not `mc_voice_transcribe`
// directly? Because the sidecar is a module command -- same pattern as
// `mc_module_call` for mc-toolbar's Modules Run modal. The dispatcher
// resolves `command` to its module's binary + action. If the voice
// module is uninstalled, dispatch returns a friendly error pointing
// at Settings -> Modules.
//
// Detection, click-strategy, and Tauri-invoke resolution mirror
// mc-back-button.js (Lesson 511) and mc-voice-native.js v1 (Lesson 706).
//
// Privacy: no audio leaves the device. whisper.cpp runs entirely
// on-CPU in the sidecar binary.

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

  // ---- Sidecar dispatcher ----
  // mc_module_call routes Tauri-style module commands to their sidecar.
  // Returns the sidecar's `result` field (already unwrapped from {ok,result}).
  function callModule(command, params) {
    return invoke('mc_module_call', { command: command, params: params || {} });
  }

  // ---- Composer text setter ----
  function setComposerText(text) {
    var candidates = document.querySelectorAll(
      'textarea[data-composer-input], textarea.composer__input, textarea[placeholder*="Message" i], [contenteditable="true"][data-composer-input]'
    );
    for (var i = 0; i < candidates.length; i++) {
      var el = candidates[i];
      try {
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
        continue;
      }
    }
    return false;
  }

  // ---- Visual feedback overlay ----
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

  function updateOverlay(pill, label, kind) {
    var id = 'mc-voice-native-overlay';
    var existing = pill || document.getElementById(id);
    if (!existing) return showOverlay(label, kind);
    existing.textContent = label;
    var color = kind === 'error' ? 'rgba(160, 30, 30, 0.95)' :
                 kind === 'success' ? 'rgba(20, 100, 60, 0.95)' :
                 'rgba(20, 20, 24, 0.85)';
    existing.style.background = color;
    return existing;
  }

  function clearOverlay(pill) {
    if (pill && pill.parentNode) pill.remove();
    var fallback = document.getElementById('mc-voice-native-overlay');
    if (fallback) fallback.remove();
  }

  // ---- Auto-download whisper model on first use ----
  // Cached after first successful check so we don't spam the dispatcher.
  // Lesson 1095 (2026-10-01, David): reset cache to null on failure
  // so the next click retries — previously the cache would stick on
  // false until the page reloaded.
  var voiceReadyCache = null; // null = not yet, true = ready, false = not installed/error

  async function ensureVoiceReady(pill) {
    if (voiceReadyCache === true) return true;
    if (voiceReadyCache === false) {
      // Reset and retry — Lesson 1095.
      voiceReadyCache = null;
    }

    // Check current state.
    try {
      var status = await callModule('mc_voice_check', {});
      if (status && status.model_loaded) {
        voiceReadyCache = true;
        return true;
      }
      // Model not loaded -- kick off download.
      updateOverlay(pill, 'Downloading whisper model (~75 MB, first time only)...', 'pending');
      try {
        await callModule('mc_voice_download_model', {});
      } catch (dlErr) {
        // Lesson 1095: surface the actual reason. Classify the error
        // so the user sees something specific (HTTP, TLS, DNS, SHA, fs).
        voiceReadyCache = false;
        var rawDl = (dlErr && dlErr.message) ? dlErr.message : String(dlErr);
        var hint;
        if (/HTTP|status code|404|403|401/i.test(rawDl)) {
          hint = 'Network error (HTTP). Check your connection / firewall. Click the mic again to retry.';
        } else if (/TLS|certificate|handshake/i.test(rawDl)) {
          hint = 'TLS error reaching milagrocloud.com. Your network may intercept HTTPS. Click the mic again to retry.';
        } else if (/DNS|resolve|host/i.test(rawDl)) {
          hint = 'DNS error. Check DNS / VPN. Click the mic again to retry.';
        } else if (/sha256|SHA|mismatch/i.test(rawDl)) {
          hint = 'Downloaded file failed SHA256 verification. Try again — may be a transient corruption.';
        } else {
          hint = 'Download failed: ' + rawDl.slice(0, 140) + '. Click the mic again to retry.';
        }
        var dlErr2 = new Error(hint);
        dlErr2.originalMessage = rawDl;
        throw dlErr2;
      }
      // Re-check.
      var status2 = await callModule('mc_voice_check', {});
      if (status2 && status2.model_loaded) {
        voiceReadyCache = true;
        return true;
      }
      voiceReadyCache = false;
      throw new Error('Download finished but model is not on disk. Check the MiracleClaw log for SHA / filesystem errors.');
    } catch (e) {
      voiceReadyCache = false;
      throw e;
    }
  }

  // ---- Main handler ----
  var invoke = resolveInvoke();
  if (!invoke) {
    if (window.console && window.console.warn) {
      window.console.warn('[mc-voice-native] Tauri invoke not found; patch is no-op.');
    }
    return;
  }

  document.addEventListener('click', async function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    var btn = target.closest('.chat-send-btn--voice, .chat-send-btn--stop');
    if (!btn) return;

    event.preventDefault();
    event.stopImmediatePropagation();

    var pill = showOverlay('[mic] Listening... (whisper.cpp)', 'pending');

    try {
      // Step 1: ensure voice module is ready (auto-download model if needed).
      var ready = await ensureVoiceReady(pill);
      if (!ready) {
        // ensureVoiceReady threw, message already on pill. Return without
        // transcribing.
        return;
      }
      updateOverlay(pill, '[mic] Listening... speak now', 'pending');

      // Step 2: transcribe (15s max capture, auto-stop on VAD silence).
      var result = await callModule('mc_voice_transcribe', {
        seconds: 15,
        vad_enabled: true,
        silence_ms: 1500,
      });
      // Sidecar result shape: {text, duration_ms, transcribe_ms, rms, ...}
      // Dispatcher returns the inner `result.value` directly (already unwrapped).
      var text = (result && typeof result.text === 'string') ? result.text.trim() : '';

      clearOverlay(pill);
      if (!text) {
        // Lesson 1095 (2026-10-01, David): "No speech detected" was
        // the most-confusing voice failure because it could mean
        // mic muted, VAD too strict, whisper rejected audio, or
        // the user spoke too quietly. The sidecar v0.1.8 returns
        // diagnostics.peakAmplitude + diagnostics.rms + vadStats.
        // We disambiguate so the user knows what to fix.
        var diag = result && result.diagnostics;
        var peak = diag && typeof diag.peakAmplitude === 'number' ? diag.peakAmplitude : null;
        var rms = diag && typeof diag.rms === 'number' ? diag.rms : null;
        var vad = diag && diag.vadStats;
        var detail;
        if (peak === 0) {
          detail = 'No audio captured. Check mic is unmuted and Windows has granted microphone permission to MiracleClaw.';
        } else if (peak !== null && peak < 0.01) {
          detail = 'Mic level very low. Try speaking louder or moving closer.';
        } else if (vad && typeof vad.speechFrames === 'number' && vad.speechFrames === 0 && vad.totalFrames > 0) {
          detail = 'Mic picked up audio but voice activity detector rejected it. Try speaking sooner after clicking, or in a quieter room.';
        } else if (rms !== null) {
          detail = 'No speech detected (rms ' + rms.toFixed(3) + ', peak ' + (peak !== null ? peak.toFixed(3) : '?') + ').';
        } else {
          detail = 'No speech detected. Check mic and try again.';
        }
        showOverlay(detail, 'error');
        setTimeout(function () {
          var p = document.getElementById('mc-voice-native-overlay');
          if (p) p.remove();
        }, 4000);
        return;
      }

      var placed = setComposerText(text);
      // Show diagnostics in the success pill: capture duration + transcribe
      // duration + whisper-recognized text preview. Confidence isn't surfaced
      // by whisper.cpp; rms (0..1) is a proxy for "how loud/clear".
      var durMs = (result && typeof result.duration_ms === 'number') ? result.duration_ms : 0;
      var txMs = (result && typeof result.transcribe_ms === 'number') ? result.transcribe_ms : 0;
      var msg = '[OK] Transcribed in ' + (durMs + txMs) + ' ms';
      if (placed) {
        showOverlay(msg + ': ' + text.slice(0, 60) + (text.length > 60 ? '...' : ''), 'success');
      } else {
        // Couldn't find the composer; surface the text so user can copy it.
        showOverlay('[clipboard] ' + text.slice(0, 80) + (text.length > 80 ? '...' : ''), 'success');
      }
      setTimeout(function () {
        var p = document.getElementById('mc-voice-native-overlay');
        if (p) p.remove();
      }, 4000);
    } catch (e) {
      // Voice module not installed, or transcribe/download failed. Reset
      // the cache so we re-check on next click.
      voiceReadyCache = null;
      clearOverlay(pill);
      var msg = (e && e.message) ? e.message : String(e);
      // mc_module_call returns errors like "sidecar not found at ..." or
      // "module 'voice' sidecar not found" -- surface a friendly hint.
      var friendly;
      if (/NotInstalled|sidecar not found|module 'voice'/i.test(msg)) {
        friendly = 'Voice module not installed. Open Settings -> Modules to install the Voice for MiracleClaw module.';
      } else {
        friendly = msg.slice(0, 200);
      }
      showOverlay('Voice error: ' + friendly, 'error');
      if (window.console && window.console.error) {
        window.console.error('[mc-voice-native] capture failed:', e);
      }
      setTimeout(function () {
        var p = document.getElementById('mc-voice-native-overlay');
        if (p) p.remove();
      }, 6000);
    }
  }, true);

  if (window.console && window.console.info) {
    window.console.info('[mc-voice-native] patched (rc57.1) -- voice button will use whisper.cpp sidecar via mc_module_call. Auto-downloads the model (~75MB) on first use.');
  }
})();
