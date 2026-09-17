// pages/paired_devices.js — Mobile pairing section content (Phase 2.4).
//
// Spec: docs/specs/mobile-desktop-pairing.md.
//
// Renders inside the Settings page as the "Mobile pairing" section.
// Self-contained: mounts/unmounts as part of settings.js lifecycle but
// owns its own DOM under the #mobile-pairing-slot container.
//
// Public API: { mountPairingSection(ctx), refresh(), revokeDevice(id, btn) }
//
// Backed by these Tauri commands (Phase 2.4):
//   - mc_get_pairing_status     — quick status snapshot
//   - mc_register_desktop_self  — register (generates keypair server-side)
//   - mc_unregister_desktop     — graceful shutdown (revokes all phones)
//   - mc_paired_devices         — list phones
//   - mc_revoke_device          — remove a phone
//   - mc_set_drop_folder        — set/clear the phone's filesystem sandbox root
//
// QR code (advanced): rendered via the `qrcode` package (added to
// package.json in Phase 2.4). Only shown when the user clicks "Show QR"
// because the primary flow is automatic via MAIC presence relay.
//
// Detection of new pairings: polls mc_get_pairing_status every 5s while
// the section is visible. Stops when unmount() is called.

import { invoke } from "@tauri-apps/api/core";
import { toast } from "../toast.js";
import QRCode from "qrcode";

let pollTimer = null;
let currentCtx = null;

// ============================================================================
// Public API
// ============================================================================

/**
 * Render the mobile pairing section into the given slot. Idempotent —
 * safe to call multiple times; will replace existing content.
 *
 * @param {object} ctx  shared context (passed through to Tauri invocations)
 */
export async function mountPairingSection(ctx = {}) {
  currentCtx = ctx;
  const slot = document.getElementById("mobile-pairing-slot");
  if (!slot) return;
  slot.innerHTML = renderSkeleton();

  // Static event listeners
  wireStaticListeners();

  // Initial load
  await refresh();

  // Poll every 5s while mounted so new pairings show up live.
  if (pollTimer) clearInterval(pollTimer);
  pollTimer = setInterval(() => {
    if (!document.getElementById("mobile-pairing-slot")) {
      // Section unmounted; stop polling.
      if (pollTimer) clearInterval(pollTimer);
      pollTimer = null;
      return;
    }
    refresh().catch((err) => {
      // Silent on poll errors — visible load errors go through toast in refresh().
      console.warn("[pairing] poll failed:", err);
    });
  }, 5000);
}

/** Refresh status + devices. Called on mount and every 5s. */
export async function refresh() {
  const slot = document.getElementById("mobile-pairing-slot");
  if (!slot) return;
  try {
    const status = await invoke("mc_get_pairing_status");
    const devices = status.registered
      ? await invoke("mc_paired_devices").catch(() => [])
      : [];
    slot.innerHTML = renderPanel(status, devices);
    wirePanelListeners(status, devices);
  } catch (err) {
    slot.innerHTML = renderError(err?.toString?.() ?? String(err));
  }
}

/** Revoke a paired device, then refresh. */
export async function revokeDevice(deviceId, btn) {
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Revoking…";
  }
  try {
    await invoke("mc_revoke_device", { deviceId: Number(deviceId) });
    toast("Phone revoked", "success");
    await refresh();
  } catch (err) {
    toast("Could not revoke phone: " + (err?.toString?.() ?? err), "error");
    if (btn) {
      btn.disabled = false;
      btn.textContent = "Revoke";
    }
  }
}

// ============================================================================
// Rendering
// ============================================================================

function renderSkeleton() {
  return `<div class="loading-slot">Loading mobile pairing…</div>`;
}

function renderError(msg) {
  return `
    <div class="settings-error">
      <strong>Could not load mobile pairing.</strong>
      <div class="muted small">${escapeHtml(msg)}</div>
      <button type="button" class="settings-button" id="pairing-retry">Retry</button>
    </div>
  `;
}

/**
 * @param {object} status  PairingStatus from Rust
 * @param {Array}  devices PairedDeviceInfo[] from Rust
 */
function renderPanel(status, devices) {
  if (!status.registered) {
    return renderUnregistered(status);
  }
  return renderRegistered(status, devices);
}

function renderUnregistered(status) {
  return `
    <div class="pairing-card">
      <div class="pairing-card-header">
        <div class="pairing-card-title">Not yet registered</div>
        <div class="muted small">
          Registering gives your phone a target to pair with. The X25519
          keypair is generated locally and never leaves this machine.
        </div>
      </div>
      <div class="pairing-card-actions">
        <button type="button" class="settings-button primary" id="pairing-register">
          Register this desktop
        </button>
      </div>
    </div>
  `;
}

function renderRegistered(status, devices) {
  return `
    <div class="pairing-card">
      <div class="pairing-card-header">
        <div class="pairing-card-title">Mobile pairing</div>
        <div class="pairing-meta">
          <div class="pairing-meta-row">
            <span class="muted">Instance</span>
            <code>${escapeHtml(status.instance_id || "—")}</code>
          </div>
          <div class="pairing-meta-row">
            <span class="muted">Active sessions</span>
            <span>${status.active_session_count ?? 0}</span>
          </div>
          <div class="pairing-meta-row">
            <span class="muted">Paired phones</span>
            <span>${status.paired_device_count ?? devices.length}</span>
          </div>
        </div>
      </div>

      <div class="pairing-card-section">
        <div class="pairing-section-label">Paired phones</div>
        ${devices.length === 0
          ? `<div class="muted small">No phones paired yet. Open the Miracle Claw mobile app and sign in with the same account — pairing is automatic.</div>`
          : renderDeviceList(devices)
        }
      </div>

      <div class="pairing-card-section">
        <div class="pairing-section-label">Drop folder</div>
        <div class="muted small">
          Phone is sandboxed to this folder. Leave empty to disable phone FS access.
        </div>
        <div class="pairing-folder-row">
          <input type="text" id="pairing-folder-input"
                 class="settings-input"
                 value="${escapeHtml(status.drop_folder || "")}"
                 placeholder="/home/you/MobileDrop" />
          <button type="button" class="settings-button" id="pairing-folder-save">Save</button>
        </div>
      </div>

      <div class="pairing-card-section">
        <details class="pairing-advanced">
          <summary>Advanced</summary>
          <div class="pairing-advanced-body">
            <button type="button" class="settings-button" id="pairing-show-qr">
              Show pairing QR code
            </button>
            <div id="pairing-qr-host" class="pairing-qr-host"></div>
            <div class="pairing-advanced-actions">
              <button type="button" class="settings-button danger" id="pairing-unregister">
                Unregister this desktop
              </button>
            </div>
            <div class="muted small pairing-advanced-note">
              Unregistering revokes every paired phone. You can re-register
              at any time (generates a fresh identity).
            </div>
          </div>
        </details>
      </div>
    </div>
  `;
}

function renderDeviceList(devices) {
  return `
    <div class="pairing-device-list">
      ${devices.map(renderDeviceCard).join("")}
    </div>
  `;
}

function renderDeviceCard(d) {
  const paired = formatTimestamp(d.paired_at_unix);
  const lastSeen = formatTimestamp(d.last_seen_at_unix);
  const fp = fingerprintFromPubkeyB64(d.phone_pubkey_b64);
  return `
    <div class="pairing-device-card" data-device-id="${escapeHtml(String(d.device_id))}">
      <div class="pairing-device-main">
        <div class="pairing-device-name">${escapeHtml(d.device_name || "(unnamed)")}</div>
        <div class="muted small">fingerprint <code>${escapeHtml(fp)}</code></div>
        <div class="muted small">paired ${escapeHtml(paired)} · last seen ${escapeHtml(lastSeen)}</div>
      </div>
      <div class="pairing-device-actions">
        <button type="button" class="settings-button danger small"
                data-action="revoke"
                data-device-id="${escapeHtml(String(d.device_id))}">
          Revoke
        </button>
      </div>
    </div>
  `;
}

// ============================================================================
// Event wiring
// ============================================================================

function wireStaticListeners() {
  // Retry on load error
  document.getElementById("pairing-retry")?.addEventListener("click", () => refresh());
}

function wirePanelListeners(status, devices) {
  // Register button (unregistered state)
  document.getElementById("pairing-register")?.addEventListener("click", () => onRegister());

  // Save drop folder
  document.getElementById("pairing-folder-save")?.addEventListener("click", (e) => onSaveFolder(e.currentTarget));

  // Show QR
  document.getElementById("pairing-show-qr")?.addEventListener("click", (e) => onShowQr(e.currentTarget));

  // Unregister
  document.getElementById("pairing-unregister")?.addEventListener("click", () => onUnregister());

  // Per-device revoke (event delegation on the list)
  document.querySelectorAll('[data-action="revoke"]').forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.getAttribute("data-device-id");
      revokeDevice(id, btn);
    });
  });
}

// ============================================================================
// Action handlers
// ============================================================================

async function onRegister() {
  const btn = document.getElementById("pairing-register");
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Registering…";
  }
  try {
    await invoke("mc_register_desktop_self", { frontendFingerprint: "" });
    toast("Registered. Open the mobile app on the same account to pair.", "success");
    await refresh();
  } catch (err) {
    toast("Could not register: " + (err?.toString?.() ?? err), "error");
    if (btn) {
      btn.disabled = false;
      btn.textContent = "Register this desktop";
    }
  }
}

async function onSaveFolder(btn) {
  const input = document.getElementById("pairing-folder-input");
  const folder = (input?.value ?? "").trim();
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Saving…";
  }
  try {
    const saved = await invoke("mc_set_drop_folder", { folder });
    toast("Drop folder updated", "success");
    if (input) input.value = saved ?? folder;
    await refresh();
  } catch (err) {
    toast("Could not save drop folder: " + (err?.toString?.() ?? err), "error");
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = "Save";
    }
  }
}

async function onShowQr(btn) {
  const host = document.getElementById("pairing-qr-host");
  if (!host) return;
  if (host.dataset.shown === "1") {
    // Already showing — toggle off
    host.innerHTML = "";
    host.dataset.shown = "0";
    if (btn) btn.textContent = "Show pairing QR code";
    return;
  }
  host.innerHTML = `<div class="muted small">Building QR…</div>`;
  try {
    const status = await invoke("mc_get_pairing_status");
    if (!status.instance_id) throw new Error("not registered");
    // Encode the instance_id as the QR payload. The phone's bootstrap
    // flow scans this and looks up the desktop via MAIC presence.
    // (Cross-network relay kicks in automatically if LAN is unreachable.)
    const payload = JSON.stringify({
      v: 1,
      kind: "mc-desktop",
      instance_id: status.instance_id,
    });
    const dataUrl = await QRCode.toDataURL(payload, {
      margin: 1,
      width: 240,
      errorCorrectionLevel: "M",
    });
    host.innerHTML = `
      <div class="pairing-qr-wrap">
        <img src="${dataUrl}" alt="Pairing QR code" width="240" height="240" />
        <div class="muted small">Scan with the Miracle Claw mobile app.</div>
      </div>
    `;
    host.dataset.shown = "1";
    if (btn) btn.textContent = "Hide QR";
  } catch (err) {
    host.innerHTML = `<div class="settings-error">QR build failed: ${escapeHtml(err?.toString?.() ?? err)}</div>`;
  }
}

async function onUnregister() {
  if (!window.confirm("Unregister this desktop? All paired phones will be revoked.")) return;
  try {
    await invoke("mc_unregister_desktop");
    toast("Unregistered", "success");
    await refresh();
  } catch (err) {
    toast("Could not unregister: " + (err?.toString?.() ?? err), "error");
  }
}

// ============================================================================
// Helpers
// ============================================================================

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatTimestamp(unixSec) {
  if (!unixSec || !Number.isFinite(unixSec)) return "—";
  const d = new Date(unixSec * 1000);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString("en-US", {
    year: "numeric", month: "short", day: "numeric",
    hour: "2-digit", minute: "2-digit",
  });
}

/**
 * Derive a short fingerprint from a base64-encoded X25519 public key.
 * Mirror of pairing_identity::fingerprint_from_pubkey (SHA-256, 6 bytes,
 * XXXX-XXXX-XXXX). Stays in sync because both sides use the same digest
 * truncation; if pairing_identity changes its format, update this too.
 */
function fingerprintFromPubkeyB64(b64) {
  try {
    const bin = atob(b64 || "");
    // Inline SHA-256 via SubtleCrypto (browser-only).
    // Synchronous fallback: hash via a small pure-JS would be heavier;
    // we accept async via a deferred render if crypto.subtle is missing.
    if (!window.crypto?.subtle) return "(no-webcrypto)";
    // We can't await inside a sync function, so we kick off an async
    // fill-in and return a placeholder. The next refresh() will pick
    // it up. For v1 this is acceptable since the placeholder is honest.
    fillFingerprintAsync(b64, bin);
    return "…";
  } catch (_) {
    return "(invalid)";
  }
}

const _fpCache = new Map();
async function fillFingerprintAsync(b64, bin) {
  if (_fpCache.has(b64)) return _fpCache.get(b64);
  try {
    const buf = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    const digest = await window.crypto.subtle.digest("SHA-256", buf);
    const hex = Array.from(new Uint8Array(digest).slice(0, 6))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    const fp = `${hex.slice(0, 4)}-${hex.slice(4, 8)}-${hex.slice(8, 12)}`;
    _fpCache.set(b64, fp);
    // Re-render to swap the placeholder. Cheap because we're already
    // showing the panel; just re-render with same data.
    refresh();
    return fp;
  } catch (_) {
    return "(hash-failed)";
  }
}
