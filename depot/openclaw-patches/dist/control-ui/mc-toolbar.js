// MiracleClaw chat-toolbar (Lesson 978, rc56.8)
//
// Adds a top-right floating toolbar with four icon-link buttons to the
// OpenClaw chat UI when hosted inside MiracleClaw's Tauri webview
// (http://127.0.0.1:28789/):
//
//   📎 Attach    — drop files, copy paths to clipboard
//   🔑 Secrets   — list local tool names + secret names (no values)
//   🎙 Voice     — invoke mc_voice_native_capture (SAPI 5)
//   🧩 Modules   — dropdown of installed modules, "Run" calls
//                  mc_run_module and sends the result into the chat input
//
// This file is *deliberately* self-contained — the openclaw dist has its
// own JS world and can't import from MC's source tree (`src/`). We
// duplicate the minimum we need: a Tauri-invoke helper, an event bus,
// a toast helper, and the modules menu. State that we *do* share with
// MC's `modules-runtime.js` (the `installed` Map) is not shared
// directly — both sides read from the same Tauri command
// `mc_list_installed_modules` so they agree.
//
// Detection (Lesson 511 — IPC-independent):
//   We gate on `window.location.host === '127.0.0.1:28789'`. No Tauri
//   globals required for the gate; the gate just tells us "we're on the
//   OpenClaw chat page hosted by MC". Actual Tauri calls inside the
//   button handlers will fail loudly if MC's webview isn't wired —
//   which is the right behavior, since the user wouldn't be seeing
//   this UI in that case anyway.
//
// Click strategy:
//   We attach CAPTURE-phase click listeners on our own buttons. We do
//   NOT touch the openclaw app's own button handlers — we live in our
//   own floating panel and never touch the React tree.
//
// Idempotency:
//   We check for `#mc-oc-toolbar` before mounting. The patcher is also
//   idempotent (it's a WRITE patch, content-compared).
//
//   All UI elements carry the `.mc-oc-` prefix so we can't accidentally
//   select something inside the openclaw host page.

(function () {
  'use strict';

  // ---------------------------------------------------------------------------
  // Gate
  // ---------------------------------------------------------------------------

  function shouldMount() {
    if (!window.location || window.location.host !== '127.0.0.1:28789') {
      return false;
    }
    if (document.getElementById('mc-oc-toolbar')) {
      return false; // already mounted
    }
    return true;
  }

  // ---------------------------------------------------------------------------
  // Tauri invoke — supports the three surfaces Tauri 2.x exposes
  // (mirrors mc-voice-native.js + mc-back-button.js).
  // ---------------------------------------------------------------------------

  function tauriInvoke(cmd, args) {
    args = args || {};
    var w = window;
    var internals = w.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === 'function') {
      return internals.invoke(cmd, args);
    }
    var t2 = w.__TAURI__ && w.__TAURI__.core;
    if (t2 && typeof t2.invoke === 'function') {
      return t2.invoke(cmd, args);
    }
    var t1 = w.__TAURI__;
    if (t1 && typeof t1.invoke === 'function') {
      return t1.invoke(cmd, args);
    }
    return Promise.reject(new Error(
      'Tauri IPC not available — mc-toolbar must run inside MC Tauri webview'
    ));
  }

  // ---------------------------------------------------------------------------
  // Toast
  // ---------------------------------------------------------------------------

  var toastTimer = null;
  function showToast(msg, kind) {
    kind = kind || 'info';
    var t = document.getElementById('mc-oc-toast');
    if (!t) {
      t = document.createElement('div');
      t.id = 'mc-oc-toast';
      t.className = 'mc-oc-toast';
      t.setAttribute('role', 'status');
      t.setAttribute('aria-live', 'polite');
      t.setAttribute('hidden', '');
      document.body.appendChild(t);
    }
    t.className = 'mc-oc-toast' + (kind === 'error' ? ' is-error' : kind === 'success' ? ' is-success' : '');
    t.textContent = String(msg);
    t.removeAttribute('hidden');
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      t.setAttribute('hidden', '');
    }, 3500);
  }

  // ---------------------------------------------------------------------------
  // Dropdown open/close (Lesson 968 — sibling of trigger, viewport-aware
  // positioning from Lesson 976)
  // ---------------------------------------------------------------------------

  var openDropdown = null;

  function closeDropdown() {
    if (openDropdown) {
      openDropdown.setAttribute('hidden', '');
      var triggerId = openDropdown.getAttribute('data-trigger');
      if (triggerId) {
        var trig = document.getElementById(triggerId);
        if (trig) trig.setAttribute('aria-expanded', 'false');
      }
      openDropdown = null;
    }
  }

  function openDropdownFor(trigger, dropdown) {
    if (openDropdown && openDropdown !== dropdown) closeDropdown();
    dropdown.removeAttribute('hidden');
    trigger.setAttribute('aria-expanded', 'true');

    // Position AFTER attach so we can measure width.
    var rect = trigger.getBoundingClientRect();
    var menuRect = dropdown.getBoundingClientRect();
    var menuWidth = menuRect.width || 280;
    var menuHeight = menuRect.height || 240;
    var margin = 8;
    var top, left;
    // Default: below the trigger, right-aligned to its right edge
    left = rect.right - menuWidth;
    if (left < margin) left = margin;
    if (left + menuWidth > window.innerWidth - margin) {
      left = window.innerWidth - menuWidth - margin;
    }
    top = rect.bottom + 6;
    // Flip above if not enough room below (200px minimum visible).
    if (top + menuHeight > window.innerHeight - margin && top - menuHeight - rect.height - 6 > margin) {
      top = top - menuHeight - rect.height - 6;
    }
    dropdown.style.left = Math.max(margin, left) + 'px';
    dropdown.style.top = Math.max(margin, top) + 'px';
    openDropdown = dropdown;
  }

  function makeButton(id, glyph, title, ariaLabel, extraClass) {
    var btn = document.createElement('button');
    btn.type = 'button';
    btn.id = id;
    btn.className = 'mc-oc-icon-link' + (extraClass ? ' ' + extraClass : '');
    btn.setAttribute('title', title);
    btn.setAttribute('aria-label', ariaLabel || title);
    btn.setAttribute('aria-haspopup', 'true');
    btn.setAttribute('aria-expanded', 'false');
    var span = document.createElement('span');
    span.className = 'mc-oc-icon-link-glyph';
    span.textContent = glyph;
    btn.appendChild(span);
    return btn;
  }

  // ---------------------------------------------------------------------------
  // Modules: list (Lesson 975 — query mc_list_installed_modules) + run
  // ---------------------------------------------------------------------------

  var _modulesCountInflight = false;

  async function fetchInstalledModulesMerged() {
    // Lesson 975: read remote (MAIC) install list. The Tauri command
    // is the same one the rest of MC uses. If it fails, return [] and
    // the dropdown will show "No modules installed" — same graceful
    // fallback the terminal toolbar uses.
    try {
      var resp = await tauriInvoke('mc_list_installed_modules');
      var list = Array.isArray(resp) ? resp
        : Array.isArray(resp && resp.modules) ? resp.modules
        : Array.isArray(resp && resp.installed) ? resp.installed
        : [];
      // Normalize: MAIC returns each module with module_id,
      // client_manifest, etc. Map to { id, name, version, hooks, icon }.
      return list.map(normalizeRemoteModule);
    } catch (e) {
      console.warn('[mc-toolbar] mc_list_installed_modules failed:', e);
      return [];
    }
  }

  function normalizeRemoteModule(m) {
    var cm = m && m.client_manifest ? m.client_manifest : {};
    return {
      id: m.module_id || m.id || cm.id || '(unknown)',
      name: cm.name || m.name || m.module_id || m.id || '(unknown)',
      version: cm.version || m.version || '',
      hooks: cm.ui_hooks || m.hooks || {},
      icon: m.icon || cm.icon || '🧩',
      tools: cm.tools || m.tools || [],
      _raw: m,
    };
  }

  async function refreshModulesCount() {
    if (_modulesCountInflight) return;
    _modulesCountInflight = true;
    try {
      var list = await fetchInstalledModulesMerged();
      var badge = document.getElementById('mc-oc-modules-count');
      if (!badge) return;
      if (list.length === 0) {
        badge.setAttribute('hidden', '');
      } else {
        badge.textContent = String(list.length);
        badge.removeAttribute('hidden');
      }
    } catch (e) {
      var badge2 = document.getElementById('mc-oc-modules-count');
      if (badge2) badge2.setAttribute('hidden', '');
    } finally {
      _modulesCountInflight = false;
    }
  }

  async function openModulesMenu() {
    var dropdown = document.getElementById('mc-oc-modules-menu');
    if (!dropdown) return;
    if (!dropdown.hasAttribute('hidden')) {
      closeDropdown();
      return;
    }
    var body = dropdown.querySelector('.mc-oc-dropdown-body');
    body.innerHTML = '<div class="mc-oc-dropdown-empty">Loading…</div>';
    openDropdownFor(document.getElementById('mc-oc-modules-btn'), dropdown);

    var list = await fetchInstalledModulesMerged();
    if (list.length === 0) {
      body.innerHTML = '<div class="mc-oc-dropdown-empty">No modules installed.<br>Install from the launchpad marketplace.</div>';
      return;
    }
    body.innerHTML = '';
    list.forEach(function (m) {
      var item = document.createElement('button');
      item.type = 'button';
      item.className = 'mc-oc-dropdown-item';
      item.setAttribute('data-module-id', m.id);
      var iconEl = document.createElement('span');
      iconEl.className = 'mc-oc-dropdown-item-icon';
      iconEl.textContent = m.icon;
      var bodyEl = document.createElement('span');
      bodyEl.className = 'mc-oc-dropdown-item-body';
      var nameEl = document.createElement('div');
      nameEl.className = 'mc-oc-dropdown-item-name';
      nameEl.textContent = m.name;
      var metaEl = document.createElement('div');
      metaEl.className = 'mc-oc-dropdown-item-meta';
      var toolCount = m.tools.length;
      var toolLabel = toolCount === 0
        ? 'no tools — open in launchpad'
        : toolCount === 1 ? '1 tool'
        : toolCount + ' tools';
      metaEl.textContent = 'v' + (m.version || '?') + ' · ' + toolLabel;
      bodyEl.appendChild(nameEl);
      bodyEl.appendChild(metaEl);
      var runBtn = document.createElement('span');
      runBtn.className = 'mc-oc-dropdown-item-run';
      runBtn.textContent = toolCount > 0 ? 'Run ▸' : 'Info ▸';
      runBtn.setAttribute('role', 'button');
      item.appendChild(iconEl);
      item.appendChild(bodyEl);
      item.appendChild(runBtn);
      item.addEventListener('click', function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        closeDropdown();
        if (m.tools.length === 0) {
          openModuleInfoModal(m);
        } else {
          openModuleRunModal(m);
        }
      });
      body.appendChild(item);
    });
  }

  function openModuleInfoModal(m) {
    showModuleModal(
      m.name + ' (v' + (m.version || '?') + ')',
      'This module does not expose client-side tools. Open it in the launchpad to run.',
      [],
      null
    );
  }

  async function openModuleRunModal(m) {
    // If the module has exactly one tool, skip the picker and go
    // straight to the run form. Otherwise show a tool picker.
    var tool = m.tools[0];
    if (m.tools.length === 1) {
      showRunForm(m, tool);
    } else {
      showModuleModal(
        m.name + ' — pick a tool',
        'Choose a tool to run.',
        m.tools.map(function (t) { return { label: t.name || t.id, value: t.id }; }),
        function (picked) { showRunForm(m, picked); }
      );
    }
  }

  function showModuleModal(title, subtitle, rows, onPick) {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = title;
    panel.appendChild(h);
    if (subtitle) {
      var sub = document.createElement('p');
      sub.className = 'mc-oc-modal-subtitle';
      sub.textContent = subtitle;
      panel.appendChild(sub);
    }
    if (rows && rows.length > 0) {
      rows.forEach(function (r) {
        var row = document.createElement('button');
        row.type = 'button';
        row.className = 'mc-oc-modal-row';
        row.style.cursor = 'pointer';
        row.style.width = '100%';
        row.style.textAlign = 'left';
        var name = document.createElement('span');
        name.className = 'mc-oc-modal-row-name';
        name.textContent = r.label;
        var val = document.createElement('span');
        val.className = 'mc-oc-modal-row-value';
        val.textContent = r.value || '';
        row.appendChild(name);
        row.appendChild(val);
        row.addEventListener('click', function () {
          document.body.removeChild(backdrop);
          if (onPick) onPick(r);
        });
        panel.appendChild(row);
      });
    }
    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () {
      document.body.removeChild(backdrop);
    });
    actions.appendChild(close);
    panel.appendChild(actions);
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);
  }

  function showRunForm(m, tool) {
    // Show a modal with a form for the tool's inputs. For now we keep
    // it simple: one textarea for free-form JSON inputs, and a Run
    // button. Power users can paste inputs; everyone else gets the
    // launchpad form for richer UX.
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = 'Run ' + m.name + ' · ' + (tool.name || tool.id);
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    var desc = (tool.description || '').slice(0, 200);
    sub.textContent = desc || 'Provide inputs as JSON. The result is pasted into the chat input.';
    panel.appendChild(sub);

    var label = document.createElement('div');
    label.style.fontSize = '11px';
    label.style.color = 'rgba(255,255,255,0.55)';
    label.style.textTransform = 'uppercase';
    label.style.letterSpacing = '0.08em';
    label.style.margin = '12px 0 4px';
    label.textContent = 'Inputs (JSON)';
    panel.appendChild(label);
    var ta = document.createElement('textarea');
    ta.style.width = '100%';
    ta.style.minHeight = '100px';
    ta.style.padding = '8px';
    ta.style.background = 'rgba(0,0,0,0.4)';
    ta.style.color = '#f5f5f7';
    ta.style.border = '1px solid rgba(255,255,255,0.20)';
    ta.style.borderRadius = '4px';
    ta.style.font = '12px/1.4 ui-monospace, "SF Mono", Menlo, Consolas, monospace';
    ta.style.resize = 'vertical';
    ta.placeholder = '{}';
    // Pre-fill with the tool's input schema if we have it.
    if (tool.inputs && typeof tool.inputs === 'object') {
      try {
        var example = {};
        Object.keys(tool.inputs).forEach(function (k) {
          var spec = tool.inputs[k];
          if (spec && spec.examples && spec.examples[0] != null) {
            example[k] = spec.examples[0];
          } else if (spec && spec.default != null) {
            example[k] = spec.default;
          } else if (spec && spec.type === 'string') {
            example[k] = '';
          }
        });
        ta.value = JSON.stringify(example, null, 2);
      } catch (e) { /* leave blank */ }
    }
    panel.appendChild(ta);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'mc-oc-modal-button';
    cancel.textContent = 'Cancel';
    cancel.addEventListener('click', function () { document.body.removeChild(backdrop); });
    var run = document.createElement('button');
    run.type = 'button';
    run.className = 'mc-oc-modal-button is-primary';
    run.textContent = 'Run';
    run.addEventListener('click', async function () {
      run.disabled = true;
      run.textContent = 'Running…';
      var inputs;
      try {
        inputs = ta.value.trim() ? JSON.parse(ta.value) : {};
      } catch (e) {
        showToast('Invalid JSON: ' + e.message, 'error');
        run.disabled = false;
        run.textContent = 'Run';
        return;
      }
      try {
        var resp = await tauriInvoke('mc_run_module', {
          moduleId: m.id,
          tool: tool.id,
          inputs: inputs,
          surface: 'openclaw_chat',
        });
        document.body.removeChild(backdrop);
        await pasteIntoChatInput(resp);
        showToast('Module ran — result pasted into chat input', 'success');
      } catch (e) {
        showToast('Run failed: ' + (e && e.message ? e.message : e), 'error');
        run.disabled = false;
        run.textContent = 'Run';
      }
    });
    actions.appendChild(cancel);
    actions.appendChild(run);
    panel.appendChild(actions);
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);
  }

  // Try to find the openclaw chat input and paste the module result into it.
  // Falls back to clipboard if we can't find the input.
  async function pasteIntoChatInput(payload) {
    var text;
    if (typeof payload === 'string') {
      text = payload;
    } else {
      // Try to surface a human-readable summary.
      if (payload.output != null) text = typeof payload.output === 'string' ? payload.output : JSON.stringify(payload.output, null, 2);
      else if (payload.result != null) text = typeof payload.result === 'string' ? payload.result : JSON.stringify(payload.result, null, 2);
      else text = JSON.stringify(payload, null, 2);
    }
    // Try common openclaw chat input selectors.
    var selectors = [
      'textarea[placeholder*="essage" i]',
      'textarea[placeholder*="chat" i]',
      'textarea[placeholder*="type" i]',
      'textarea',
      '[contenteditable="true"]',
    ];
    var el = null;
    for (var i = 0; i < selectors.length; i++) {
      var candidates = document.querySelectorAll(selectors[i]);
      for (var j = 0; j < candidates.length; j++) {
        var c = candidates[j];
        // Prefer visible, non-empty elements
        if (c.offsetParent !== null) { el = c; break; }
      }
      if (el) break;
    }
    if (el && 'value' in el) {
      var existing = el.value || '';
      el.value = existing + (existing ? '\n\n' : '') + text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.focus();
      return;
    }
    if (el && el.isContentEditable) {
      el.innerText = (el.innerText || '') + '\n\n' + text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.focus();
      return;
    }
    // Fallback: clipboard
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(text);
        showToast('Copied to clipboard — paste into chat (Ctrl+V)', 'info');
      } else {
        showToast('Could not locate chat input; result in console', 'error');
        console.log('[mc-toolbar] module result:', text);
      }
    } catch (e) {
      showToast('Could not locate chat input; result in console', 'error');
      console.log('[mc-toolbar] module result:', text);
    }
  }

  // ---------------------------------------------------------------------------
  // Voice: invoke mc_voice_native_capture (proven path from mc-voice-native.js)
  // ---------------------------------------------------------------------------

  async function onVoiceClick(btn) {
    if (btn.getAttribute('data-state') === 'listening') return;
    setVoiceState(btn, 'listening');
    try {
      var text = await tauriInvoke('mc_voice_native_capture', { timeoutMs: 15000 });
      setVoiceState(btn, 'idle');
      if (text && String(text).trim()) {
        // Paste into chat input if we can find it.
        await pasteIntoChatInput(String(text));
        showToast('Voice captured — pasted into chat input', 'success');
      } else {
        showToast('No speech detected', 'info');
      }
    } catch (e) {
      setVoiceState(btn, 'idle');
      showToast('Voice capture failed: ' + (e && e.message ? e.message : e), 'error');
    }
  }

  function setVoiceState(btn, state) {
    btn.setAttribute('data-state', state);
  }

  // ---------------------------------------------------------------------------
  // Attach: drop files, copy paths to clipboard (same UX as terminal toolbar)
  // ---------------------------------------------------------------------------

  var _attached = [];

  function openAttachModal() {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = '📎 Attach files';
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    sub.textContent = 'Drop files below. We will copy the file paths to your clipboard so you can paste them into the chat (Ctrl+V).';
    panel.appendChild(sub);

    var dz = document.createElement('div');
    dz.className = 'mc-oc-attach-dropzone';
    dz.textContent = 'Drop files here, or click to choose';
    dz.setAttribute('role', 'button');
    dz.setAttribute('tabindex', '0');
    var fileInput = document.createElement('input');
    fileInput.type = 'file';
    fileInput.multiple = true;
    fileInput.style.display = 'none';
    dz.addEventListener('click', function () { fileInput.click(); });
    dz.addEventListener('keydown', function (ev) {
      if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); fileInput.click(); }
    });
    fileInput.addEventListener('change', function () {
      addAttached(Array.from(fileInput.files || []));
      fileInput.value = '';
    });
    ['dragenter', 'dragover'].forEach(function (evName) {
      dz.addEventListener(evName, function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        dz.classList.add('is-dragover');
      });
    });
    ['dragleave', 'drop'].forEach(function (evName) {
      dz.addEventListener(evName, function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        if (evName === 'dragleave' && ev.target !== dz) return;
        dz.classList.remove('is-dragover');
      });
    });
    dz.addEventListener('drop', function (ev) {
      var dt = ev.dataTransfer;
      if (!dt) return;
      addAttached(Array.from(dt.files || []));
    });
    panel.appendChild(dz);
    panel.appendChild(fileInput);

    var list = document.createElement('div');
    list.className = 'mc-oc-attach-list';
    panel.appendChild(list);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () {
      document.body.removeChild(backdrop);
      _attached = [];
    });
    var copy = document.createElement('button');
    copy.type = 'button';
    copy.className = 'mc-oc-modal-button is-primary';
    copy.textContent = 'Copy paths to clipboard';
    copy.disabled = true;
    copy.addEventListener('click', async function () {
      var paths = _attached.map(function (f) { return f.path || f.name; });
      var joined = paths.join('\n');
      try {
        await navigator.clipboard.writeText(joined);
        showToast('Copied ' + paths.length + ' path(s) — paste into chat (Ctrl+V)', 'success');
        document.body.removeChild(backdrop);
        _attached = [];
      } catch (e) {
        showToast('Clipboard write failed: ' + e.message, 'error');
      }
    });
    actions.appendChild(close);
    actions.appendChild(copy);
    panel.appendChild(actions);

    function addAttached(files) {
      files.forEach(function (f) {
        // In a Tauri webview, File.path is set (Tauri patches the File
        // ctor to include the absolute path). If it's not, we just
        // record the name.
        var entry = { name: f.name, path: f.path || f.name, size: f.size };
        _attached.push(entry);
        var item = document.createElement('div');
        item.className = 'mc-oc-attach-list-item';
        var nameEl = document.createElement('span');
        nameEl.className = 'mc-oc-attach-list-item-name';
        nameEl.textContent = entry.path;
        var sz = document.createElement('span');
        sz.style.color = 'rgba(255,255,255,0.55)';
        sz.style.fontSize = '11px';
        sz.textContent = humanSize(entry.size);
        item.appendChild(nameEl);
        item.appendChild(sz);
        list.appendChild(item);
      });
      copy.disabled = _attached.length === 0;
    }
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) {
        document.body.removeChild(backdrop);
        _attached = [];
      }
    });
    document.body.appendChild(backdrop);
  }

  function humanSize(n) {
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
    if (n < 1024 * 1024 * 1024) return Math.round(n / 1024 / 1024) + ' MB';
    return (n / 1024 / 1024 / 1024).toFixed(1) + ' GB';
  }

  // ---------------------------------------------------------------------------
  // Secrets: list tool names (no values). Uses mc_list_tools.
  // ---------------------------------------------------------------------------

  async function openSecretsModal() {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = '🔑 Secrets & local tools';
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    sub.textContent = 'Names of the local tools and secrets available to MAIC. Values are never shown in the chat UI.';
    panel.appendChild(sub);

    var section = document.createElement('div');
    section.className = 'mc-oc-modal-section';
    var sectionH = document.createElement('h3');
    sectionH.textContent = 'Local tools';
    section.appendChild(sectionH);
    var loading = document.createElement('div');
    loading.style.color = 'rgba(255,255,255,0.55)';
    loading.style.fontStyle = 'italic';
    loading.textContent = 'Loading…';
    section.appendChild(loading);
    panel.appendChild(section);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () { document.body.removeChild(backdrop); });
    actions.appendChild(close);
    panel.appendChild(actions);

    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);

    try {
      var resp = await tauriInvoke('mc_list_tools');
      var tools = Array.isArray(resp) ? resp : [];
      section.removeChild(loading);
      if (tools.length === 0) {
        var empty = document.createElement('div');
        empty.style.color = 'rgba(255,255,255,0.55)';
        empty.style.fontStyle = 'italic';
        empty.textContent = 'No local tools enabled.';
        section.appendChild(empty);
        return;
      }
      tools.forEach(function (t) {
        var row = document.createElement('div');
        row.className = 'mc-oc-modal-row';
        var name = document.createElement('span');
        name.className = 'mc-oc-modal-row-name';
        name.textContent = String(t);
        row.appendChild(name);
        section.appendChild(row);
      });
    } catch (e) {
      section.removeChild(loading);
      var err = document.createElement('div');
      err.style.color = '#ff8080';
      err.textContent = 'Failed to list tools: ' + (e && e.message ? e.message : e);
      section.appendChild(err);
    }
  }

  // ---------------------------------------------------------------------------
  // Mount
  // ---------------------------------------------------------------------------

  function loadCSS() {
    if (document.getElementById('mc-oc-toolbar-css')) return;
    var link = document.createElement('link');
    link.id = 'mc-oc-toolbar-css';
    link.rel = 'stylesheet';
    link.href = './mc-toolbar.css';
    document.head.appendChild(link);
  }

  function mount() {
    if (!shouldMount()) return;
    if (!document.body) {
      document.addEventListener('DOMContentLoaded', mount, { once: true });
      return;
    }
    loadCSS();

    var bar = document.createElement('div');
    bar.id = 'mc-oc-toolbar';
    bar.className = 'mc-oc-toolbar';
    bar.setAttribute('role', 'toolbar');
    bar.setAttribute('aria-label', 'MiracleClaw chat tools');

    var attachBtn = makeButton('mc-oc-attach-btn', '📎', 'Attach files to next chat message', 'Attach files');
    attachBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      openAttachModal();
    });
    bar.appendChild(attachBtn);

    var secretsBtn = makeButton('mc-oc-secrets-btn', '🔑', 'Add usernames, passwords, API keys to the MC secrets vault', 'Secrets');
    secretsBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      // Lesson 980: open the actual MC secrets card (same overlay the
      // Terminal page uses) instead of a tools-list modal. The MC main
      // window navigates to #mcAutoOpen=secrets which the terminal
      // page picks up and auto-opens the secrets overlay on. When the
      // user closes the overlay, mc_close_overlay navigates them back
      // to the chat URL (stashed by mc_open_overlay).
      tauriInvoke('mc_open_overlay', { overlay_key: 'secrets' })
        .then(function () { /* main window will navigate */ })
        .catch(function (e) {
          console.error('[mc-toolbar] mc_open_overlay(secrets) failed:', e);
          alert('Failed to open Secrets: ' + (e && e.message ? e.message : e));
        });
    });
    bar.appendChild(secretsBtn);

    var voiceBtn = makeButton('mc-oc-voice-btn', '🎙', 'Voice input (Windows SAPI)', 'Voice input');
    voiceBtn.classList.add('mc-oc-voice-btn');
    voiceBtn.setAttribute('data-state', 'idle');
    voiceBtn.removeAttribute('aria-haspopup');
    voiceBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      onVoiceClick(voiceBtn);
    });
    bar.appendChild(voiceBtn);

    var modulesBtn = makeButton('mc-oc-modules-btn', '🧩', 'Installed modules', 'Open modules menu');
    modulesBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      openModulesMenu();
    });
    var modulesCount = document.createElement('span');
    modulesCount.id = 'mc-oc-modules-count';
    modulesCount.className = 'mc-oc-icon-link-count';
    modulesCount.setAttribute('hidden', '');
    modulesCount.textContent = '0';
    modulesBtn.appendChild(modulesCount);
    bar.appendChild(modulesBtn);

    document.body.appendChild(bar);

    // Sibling dropdown (Lesson 968) for the modules menu.
    var modulesMenu = document.createElement('div');
    modulesMenu.id = 'mc-oc-modules-menu';
    modulesMenu.className = 'mc-oc-dropdown';
    modulesMenu.setAttribute('hidden', '');
    modulesMenu.setAttribute('data-trigger', 'mc-oc-modules-btn');
    modulesMenu.setAttribute('role', 'menu');
    var modulesHeader = document.createElement('div');
    modulesHeader.className = 'mc-oc-dropdown-header';
    modulesHeader.textContent = 'Installed modules';
    modulesMenu.appendChild(modulesHeader);
    var modulesBody = document.createElement('div');
    modulesBody.className = 'mc-oc-dropdown-body';
    modulesMenu.appendChild(modulesBody);
    document.body.appendChild(modulesMenu);

    // Close on outside click + scroll/resize (Lesson 976).
    document.addEventListener('click', function (ev) {
      if (!openDropdown) return;
      if (openDropdown.contains(ev.target)) return;
      var triggerId = openDropdown.getAttribute('data-trigger');
      if (triggerId && document.getElementById(triggerId) && document.getElementById(triggerId).contains(ev.target)) return;
      closeDropdown();
    });
    window.addEventListener('scroll', closeDropdown, true);
    window.addEventListener('resize', closeDropdown);
    document.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape' && openDropdown) closeDropdown();
    });

    // Initial count + retry after a delay (in case the openclaw app
    // finishes mounting after us).
    refreshModulesCount();
    setTimeout(refreshModulesCount, 2000);
    setTimeout(refreshModulesCount, 6000);
  }

  // Defer to next tick so we run after openclaw's own scripts.
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', mount, { once: true });
  } else {
    setTimeout(mount, 0);
  }
})();
