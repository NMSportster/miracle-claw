// MiracleClaw back-to-dashboard button (Lesson 502).
//
// Injected into control-ui/index.html by scripts/patch-openclaw-dist.sh.
// Adds a "← Dashboard" button to the chat UI ONLY when hosted inside
// MiracleClaw's Tauri webview (detected via window.__openclawHostBridge.hosted,
// which the bridge initialization_script sets before this script runs).
//
// Click uses `window.location.href = 'tauri://localhost/index.html'` instead
// of Tauri IPC. This is a deliberate, redundant fallback to the bridge pill:
// - On the dashboard page (tauri://localhost) `window.location.href` does
//   nothing useful — the button is hidden there because we're not on the
//   chat page.
// - On the chat page (http://127.0.0.1:28789) the cross-scheme navigation
//   is followed by WebView2 natively (the same primitive `openclaw_open_window`
//   uses to navigate the main window dashboard→chat). No IPC dependency,
//   no invoke chain, no `__TAURI__` globals needed.
//
// Why two buttons (bridge pill + this one):
// The bridge pill is the primary path: it logs every click to the Rust
// command handler so we have observability. This chat-UI button is a
// belt-and-suspenders fallback for the case where Tauri IPC silently
// fails on cross-origin pages — the page's own DOM never depends on it.
//
// Idempotent: detects existing button by ID and skips.
(function () {
  try {
    // Only render if hosted inside MC. On the standalone web (or any
    // non-MC context) the bridge global is undefined or hosted === false.
    var bridge = window.__openclawHostBridge;
    if (!bridge || !bridge.hosted) return;
    // Only render on the chat page (host is the openclaw gateway).
    if (!bridge.isChatPage || !bridge.isChatPage()) return;
    // Skip if already injected (defense in depth — patch is idempotent but
    // also survives accidental double-injection).
    if (document.getElementById('mc-back-button')) return;
    if (!document.body) {
      // DOM not ready yet — defer until it is. The chat UI's own
      // scripts run after this so the body will exist by next tick.
      document.addEventListener('DOMContentLoaded', arguments.callee, { once: true });
      return;
    }

    var btn = document.createElement('button');
    btn.id = 'mc-back-button';
    btn.type = 'button';
    btn.textContent = '← Dashboard';
    btn.setAttribute('aria-label', 'Return to MiracleClaw dashboard');
    // Match the bridge pill's visual style so the two buttons look like one.
    btn.style.cssText = [
      'position: fixed',
      'top: 12px',
      'left: 12px',
      'z-index: 2147483647',
      'padding: 6px 12px',
      'border: 1px solid rgba(255,255,255,0.18)',
      'border-radius: 6px',
      'background: rgba(20, 20, 24, 0.85)',
      'color: #f5f5f7',
      'font: 500 12px/1.2 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
      'cursor: pointer',
      'box-shadow: 0 4px 14px rgba(0,0,0,0.45)',
      'backdrop-filter: blur(8px)',
      '-webkit-backdrop-filter: blur(8px)',
      'transition: background .15s ease, transform .1s ease',
      'user-select: none',
      '-webkit-user-select: none',
    ].join(';');
    btn.addEventListener('mouseenter', function () {
      btn.style.background = 'rgba(40, 40, 48, 0.95)';
    });
    btn.addEventListener('mouseleave', function () {
      btn.style.background = 'rgba(20, 20, 24, 0.85)';
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
      // Plain DOM navigation — no IPC, no invoke, no Tauri globals needed.
      // WebView2 follows cross-scheme navigation natively.
      try {
        window.location.href = 'tauri://localhost/index.html';
      } catch (e) {
        // Last-resort: try replace() in case href is blocked somehow.
        try {
          window.location.replace('tauri://localhost/index.html');
        } catch (e2) {
          // Visible surface for the user so we know the fallback also failed.
          btn.textContent = 'navigation failed';
          btn.style.background = 'rgba(120, 30, 30, 0.95)';
        }
      }
    });
    document.body.appendChild(btn);
  } catch (e) {
    // Never break the host page if our patch errors.
    // eslint-disable-next-line no-console
    if (window.console && window.console.warn) {
      window.console.warn('[mc-back-button] failed to mount:', e);
    }
  }
})();