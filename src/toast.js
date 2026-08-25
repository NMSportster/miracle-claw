// toast.js — Lesson 572 (2026-08-25 00:48 MDT, David)
// Lightweight inline toast notification system. Replaces alert() calls
// for non-blocking UX (voice transcript, module install, etc).
//
// Usage:
//   import { toast } from "./toast.js";
//   toast("🎙 Voice transcription failed");
//   toast("Voice module installed", { kind: "success", duration: 4000 });
//   toast("Installing whisper model…", { kind: "info", sticky: true });
//
// Public API:
//   toast(message, options?)  — show a toast
//   toast.dismiss(id)         — dismiss a specific toast by id
//   toast.clear()             — dismiss all toasts
//
// Toast kinds: "info" (default, neutral), "success" (green),
// "warn" (amber), "error" (red).

let nextId = 1;
let container = null;

function getContainer() {
  if (container) return container;
  container = document.createElement("div");
  container.id = "mc-toast-container";
  container.setAttribute("role", "region");
  container.setAttribute("aria-label", "Notifications");
  document.body.appendChild(container);
  return container;
}

function makeToast(message, options = {}) {
  const id = nextId++;
  const kind = options.kind || "info";
  const duration = options.duration ?? (kind === "error" ? 7000 : 4000);
  const sticky = options.sticky === true;

  const el = document.createElement("div");
  el.className = `mc-toast mc-toast-${kind}`;
  el.setAttribute("role", kind === "error" ? "alert" : "status");
  el.dataset.toastId = String(id);

  const msg = document.createElement("span");
  msg.className = "mc-toast-message";
  msg.textContent = message;
  el.appendChild(msg);

  // Action button (optional)
  if (typeof options.action === "function" && options.actionLabel) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mc-toast-action";
    btn.textContent = options.actionLabel;
    btn.addEventListener("click", () => {
      try {
        options.action();
      } finally {
        dismiss(id);
      }
    });
    el.appendChild(btn);
  }

  // Dismiss button
  const dismissBtn = document.createElement("button");
  dismissBtn.type = "button";
  dismissBtn.className = "mc-toast-dismiss";
  dismissBtn.setAttribute("aria-label", "Dismiss");
  dismissBtn.textContent = "✕";
  dismissBtn.addEventListener("click", () => dismiss(id));
  el.appendChild(dismissBtn);

  getContainer().appendChild(el);

  // Auto-dismiss
  if (!sticky) {
    setTimeout(() => dismiss(id), duration);
  }

  return id;
}

function dismiss(id) {
  const el = getContainer().querySelector(`[data-toast-id="${id}"]`);
  if (!el) return;
  el.classList.add("mc-toast-leaving");
  setTimeout(() => {
    if (el.parentNode) el.parentNode.removeChild(el);
  }, 220);
}

function clear() {
  if (!container) return;
  container.innerHTML = "";
}

export const toast = makeToast;
toast.dismiss = dismiss;
toast.clear = clear;