// MiracleClaw chat toolbar (Lesson 243, rc53.9).
//
// Injected into control-ui/index.html by scripts/patch-openclaw-dist.sh.
// Adds a 🔑 Secrets + 📎 Attach floating toolbar at the top-right of the
// OpenClaw chat page when hosted inside MiracleClaw's Tauri webview.
//
// David asked 2026-08-23: "the right side of that topbar is empty and
// perfect to add the secrets and dragndrop icons" — referring to the
// OpenClaw chat topbar (Openclaw · Main · Chat). The OpenClaw chat
// topbar is built by the vendored React bundle from the OpenClaw
// project, which we don't fork. This patch is the same pattern as the
// mc-back-button.js sibling: a fixed-position div at the top-right
// that LOOKS attached to the topbar without touching the React tree.
//
// Detection (same as mc-back-button.js, Lesson 511 — IPC-INDEPENDENT):
//   `window.location.host === '127.0.0.1:28789'`
// That host is only used by MC's bridge to openclaw_open_window, so
// we're guaranteed to be inside MC's webview.
//
// Click handler (same IPC-INDEPENDENT trick as mc-back-button.js):
//   `window.location.href = 'tauri://localhost/index.html#mcAutoOpen=<key>'`
// WebView2 follows cross-scheme navigation natively. The hash carries
// the auto-open intent across the cross-origin boundary (127.0.0.1:28789
// → tauri://localhost have SEPARATE localStorage, so URL hash is the
// only signal that survives). MC's main.js boot reads the hash and
// routes straight to Terminal with extras; terminal.js consumes the
// hash, opens the matching overlay, and strips the hash from the URL.
//
// Why no Extras hub / mlg-* dropdown here:
// David said "I think the way it is in that small terminal window is
// perfect" — meaning the small terminal toolbar (just 🔑 + 📎) is the
// desired density for an in-chat overlay too. mlg-* lives in the
// Extras hub on the dashboard, accessed by explicit navigation there.
//
// Idempotent: detects existing toolbar by ID and skips re-mount.
(function () {
  try {
    if (!window.location || window.location.host !== '127.0.0.1:28789') return;
    if (document.getElementById('mc-chat-toolbar')) return;
    if (!document.body) {
      document.addEventListener('DOMContentLoaded', arguments.callee, { once: true });
      return;
    }

    var bar = document.createElement('div');
    bar.id = 'mc-chat-toolbar';
    bar.style.cssText = [
      'position: fixed',
      'top: 12px',
      'right: 12px',
      'z-index: 2147483647',
      'display: inline-flex',
      'align-items: center',
      'gap: 6px',
      'padding: 4px',
      'border-radius: 8px',
      'background: rgba(20, 20, 24, 0.55)',
      'box-shadow: 0 4px 14px rgba(0,0,0,0.35)',
      'backdrop-filter: blur(8px)',
      '-webkit-backdrop-filter: blur(8px)',
      'user-select: none',
      '-webkit-user-select: none',
    ].join(';');

    function makeBtn(icon, label, ariaLabel, overlayKey) {
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.setAttribute('aria-label', ariaLabel);
      btn.title = ariaLabel;
      // Style mirrors the dashboard's terminal toolbar icon-link
      // (.icon-link) so the visual language stays consistent across
      // MC pages. Padding is tighter than mc-back-button so the two
      // buttons fit at 1200px chat width without colliding with the
      // page's own topbar elements (which usually sit on the right
      // edge of the OpenClaw topbar — not at the very right of the
      // viewport). If we ever collide, bump the right offset.
      btn.style.cssText = [
        'display: inline-flex',
        'align-items: center',
        'gap: 4px',
        'padding: 4px 10px',
        'border: 1px solid rgba(255,255,255,0.18)',
        'border-radius: 6px',
        'background: rgba(20, 20, 24, 0.85)',
        'color: #f5f5f7',
        'font: 500 12px/1.2 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
        'cursor: pointer',
        'transition: background .15s ease, transform .1s ease, border-color .15s ease',
      ].join(';');

      var ico = document.createElement('span');
      ico.textContent = icon;
      ico.style.cssText = 'font-size: 13px; line-height: 1;';
      var txt = document.createElement('span');
      txt.textContent = label;
      btn.appendChild(ico);
      btn.appendChild(txt);

      btn.addEventListener('mouseenter', function () {
        btn.style.background = 'rgba(40, 40, 48, 0.95)';
        btn.style.borderColor = 'rgba(127, 182, 255, 0.6)';
      });
      btn.addEventListener('mouseleave', function () {
        btn.style.background = 'rgba(20, 20, 24, 0.85)';
        btn.style.borderColor = 'rgba(255,255,255,0.18)';
      });
      btn.addEventListener('mousedown', function () {
        btn.style.transform = 'scale(0.97)';
      });
      btn.addEventListener('mouseup', function () {
        btn.style.transform = 'scale(1)';
      });

      btn.addEventListener('click', function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        // Plain DOM navigation with URL hash payload. The hash is
        // consumed by main.js boot (URL hash → navigate to terminal
        // with extras) and then by terminal.js mount (open overlay
        // + strip hash). WebView2 follows cross-scheme navigation
        // natively — no IPC dependency, no invoke chain.
        try {
          window.location.href =
            'tauri://localhost/index.html#mcAutoOpen=' + encodeURIComponent(overlayKey);
        } catch (e) {
          btn.textContent = 'navigation failed';
          btn.style.background = 'rgba(120, 30, 30, 0.95)';
          btn.style.borderColor = 'rgba(255, 80, 80, 0.6)';
        }
      });

      return btn;
    }

    bar.appendChild(makeBtn('📎', 'Attach', 'Attach files to next chat message', 'attach'));
    bar.appendChild(makeBtn('🔑', 'Secrets', 'Open the secrets vault', 'secrets'));

    document.body.appendChild(bar);
  } catch (e) {
    // Never break the host page if our patch errors.
    if (window.console && window.console.warn) {
      window.console.warn('[mc-chat-toolbar] failed to mount:', e);
    }
  }
})();
