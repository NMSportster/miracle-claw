// pages/files.js — MC Files tab (v1.0.0-prep).
//
// Public API: { mount(root, ctx), unmount() }
//
// Surface:
//   - Top breadcrumb row with the current path.
//   - ".." entry (when not at a root) to go up one level.
//   - Dirs-first list of entries. Click a dir → descend. Click a file →
//     load and display its contents in the preview pane.
//   - Preview pane: text content with line numbers; image inline preview
//     (PNG/JPG/GIF/WebP/SVG/BMP); "Open in default app" fallback for
//     binaries (PDF, Office, archives) and over-cap text files.
//
// Backend:
//   - `mc_ui_list_allowed_roots()` → array of paths
//   - `mc_ui_list_dir(path)` → { path, entries: [{name, path, kind, size}] }
//   - `mc_ui_read_file(path, max_bytes?)` → { path, content, bytes, truncated }
//   - `mc_ui_read_image(path)` → { path, mime, bytes, data_base64 }
//   - `mc_ui_open_externally(path)` → hands the file to the OS handler
//
// Path safety: enforcement happens server-side (allowlist under Documents,
// Desktop, Downloads, MC workspace). The UI surfaces the allowed roots
// rather than letting the user type arbitrary paths.

import { invoke } from "@tauri-apps/api/core";

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function dirname(p) {
  if (!p) return "";
  // Cross-platform-ish parent: strip trailing slashes, find last separator.
  const trimmed = p.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  if (idx <= 0) return trimmed; // root or drive root
  return trimmed.slice(0, idx);
}

function normalizeSlashes(p) {
  return (p || "").replace(/\\/g, "/");
}

const IMAGE_EXTS = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico",
]);
const BINARY_EXTS = new Set([
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
  "zip", "rar", "7z", "tar", "gz", "bz2",
  "exe", "dll", "so", "dylib", "bin", "iso", "img",
  "mp3", "mp4", "mov", "avi", "mkv", "wav", "flac",
  "ttf", "otf", "woff", "woff2",
]);

function extOf(p) {
  if (!p) return "";
  const base = normalizeSlashes(p).split("/").pop() || "";
  const dot = base.lastIndexOf(".");
  return dot >= 0 ? base.slice(dot + 1).toLowerCase() : "";
}

function classifyKind(p) {
  const ext = extOf(p);
  if (IMAGE_EXTS.has(ext)) return "image";
  if (BINARY_EXTS.has(ext)) return "binary";
  return "text";
}

// Heuristic for "is this byte buffer probably text?" — used when the
// extension doesn't already classify the file. Looks for high ratio of
// printable ASCII / common UTF-8 sequences. Empty / pure whitespace is
// treated as text. Otherwise binary.
function looksLikeText(bytes) {
  if (!bytes.length) return true;
  let printable = 0;
  for (let i = 0; i < bytes.length; i++) {
    const b = bytes[i];
    if (b === 9 || b === 10 || b === 13) {
      // tab, LF, CR
      printable++;
    } else if (b >= 32 && b < 127) {
      printable++;
    } else if (b >= 0xc0 && b < 0xfe) {
      // lead bytes of 2- and 3-byte UTF-8 sequences
      printable++;
    }
  }
  return printable / bytes.length > 0.85;
}

// Convert a UTF-8 string (with optional text) into an HTML representation
// that adds line numbers in a side gutter. Body is escaped to avoid HTML
// injection from user-supplied files.
function renderTextWithLineNumbers(text) {
  const lines = text.split(/\r?\n/);
  const pad = String(lines.length).length;
  const rows = lines
    .map((line, idx) => {
      const num = String(idx + 1).padStart(pad, " ");
      return `<div class="ln-row"><span class="ln-num">${num}</span><span class="ln-text">${escapeHtml(
        line
      ) || "&nbsp;"}</span></div>`;
    })
    .join("");
  return `<div class="ln">${rows}</div>`;
}

export const filesPage = {
  label: "Files",
  icon: "📁",
  requiresAuth: true,

  mount(root, ctx = {}) {
    const { onBackToDashboard } = ctx;
    let currentPath = null;
    let roots = [];
    let busy = false;

    // Lesson 574 (2026-08-25 08:19 MDT, David): preview-head sends
    // the currently-previewed file to the chat input. Used in all 4
    // preview-head render paths (image, binary, text, binary-detected).
    const sendToChatBtnHtml = `
      <button class="link-button" id="preview-send-to-chat"
              title="Add this file's path to the active chat input">
        Add to Chat
      </button>`;

    root.innerHTML = `
      <div class="files-page">
        <header class="files-header">
          <button class="link-button" id="files-back">← Dashboard</button>
          <div class="files-path" id="files-path" title="Current directory">
            Loading allowed roots…
          </div>
        </header>
        <div class="files-roots" id="files-roots"></div>
        <div class="files-body">
          <div class="files-list" id="files-list">
            <div class="muted small">Pick a folder above to start browsing.</div>
          </div>
          <div class="files-preview" id="files-preview">
            <div class="muted small">Click a file to preview its contents.</div>
          </div>
        </div>
        <div class="files-status muted small" id="files-status"></div>
      </div>
    `;

    const pathEl = root.querySelector("#files-path");
    const rootsEl = root.querySelector("#files-roots");
    const listEl = root.querySelector("#files-list");
    const previewEl = root.querySelector("#files-preview");
    const statusEl = root.querySelector("#files-status");
    const backEl = root.querySelector("#files-back");

    backEl.addEventListener("click", () => {
      if (onBackToDashboard) onBackToDashboard();
    });

    const setStatus = (msg, kind = "info") => {
      statusEl.textContent = msg || "";
      statusEl.dataset.kind = kind;
    };

    const renderRoots = () => {
      if (!roots.length) {
        rootsEl.innerHTML = `<div class="muted small">No allowed roots found.</div>`;
        return;
      }
      rootsEl.innerHTML = roots
        .map(
          (r) =>
            `<button class="root-chip" data-path="${escapeHtml(r)}">${escapeHtml(
              r
            )}</button>`
        )
        .join("");
      rootsEl.querySelectorAll(".root-chip").forEach((b) => {
        b.addEventListener("click", () => loadDir(b.dataset.path));
      });
    };

    const renderEntries = (entries, path) => {
      pathEl.textContent = path;
      const parent = dirname(path);
      const rows = [];
      if (parent && parent !== path) {
        rows.push(
          `<div class="entry entry-up" data-path="${escapeHtml(parent)}">
             <span class="entry-icon">↩</span>
             <span class="entry-name">..</span>
           </div>`
        );
      }
      for (const e of entries) {
        const icon = e.kind === "dir" ? "📁" : e.kind === "file" ? "📄" : "❔";
        const size = e.kind === "file" ? formatBytes(e.size) : "";
        // Lesson 574 (2026-08-25 08:19 MDT, David): Add-to-Chat button so
        // a user browsing files can drop them into the Terminal chat
        // input. Dirs get no button (only files have chat context).
        const addBtn = e.kind === "file"
          ? `<button class="link-button entry-add" title="Add this file's path to the active chat input">Add to Chat</button>`
          : "";
        rows.push(
          `<div class="entry" data-path="${escapeHtml(
            e.path
          )}" data-kind="${escapeHtml(e.kind)}">
             <span class="entry-icon">${icon}</span>
             <span class="entry-name">${escapeHtml(e.name)}</span>
             <span class="entry-size muted small">${escapeHtml(size)}</span>
             ${addBtn}
           </div>`
        );
      }
      if (!rows.length) {
        listEl.innerHTML = `<div class="muted small">(empty directory)</div>`;
        return;
      }
      listEl.innerHTML = rows.join("");
      listEl.querySelectorAll(".entry").forEach((el) => {
        // Lesson 574 (2026-08-25 08:19 MDT, David): per-row "Add to
        // Chat" button emits a global event the chat input picks up.
        // stopPropagation so the click doesn't also preview-load.
        const addBtn = el.querySelector(".entry-add");
        if (addBtn) {
          addBtn.addEventListener("click", (ev) => {
            ev.stopPropagation();
            const filePath = el.dataset.path;
            // Emit on the document so any page currently mounted (terminal
            // chat input, dashboard chat input, OpenClaw overlay chat
            // input) can listen and react. Path-only payload — chat
            // input formats a clean mention like "@file:/path/to/foo.txt".
            document.dispatchEvent(
              new CustomEvent("mc:files:add-to-chat", {
                detail: { path: filePath, source: "files-page" },
              })
            );
            setStatus(`Added ${normalizeSlashes(filePath)} to chat input.`);
          });
        }
        el.addEventListener("click", () => {
          // Mark the new selection visually so the user can see which
          // file is currently being previewed — fixes the "I clicked
          // something toward the bottom and don't know what I picked"
          // confusion (rc49).
          listEl.querySelectorAll(".entry.selected").forEach((sel) => {
            sel.classList.remove("selected");
          });
          el.classList.add("selected");
          const kind = el.dataset.kind;
          const p = el.dataset.path;
          if (kind === "dir" || el.classList.contains("entry-up")) {
            loadDir(p);
          } else {
            loadFile(p);
          }
        });
      });
    };

    const loadDir = async (path) => {
      if (busy) return;
      busy = true;
      setStatus(`Loading ${path}…`);
      // Reset selection — the user is navigating directories, so any
      // prior file selection is no longer the "current" file (rc49).
      listEl
        .querySelectorAll(".entry.selected")
        .forEach((el) => el.classList.remove("selected"));
      // Scroll the preview back to the top so the user sees the new
      // empty-preview placeholder immediately, not a leftover from a
      // previously-loaded file (rc49).
      previewEl.innerHTML = `<div class="muted small">Click a file to preview its contents.</div>`;
      previewEl.scrollTop = 0;
      try {
        const result = await invoke("mc_ui_list_dir", { path });
        currentPath = result.path;
        renderEntries(result.entries, result.path);
        setStatus("");
      } catch (err) {
        setStatus(`Error: ${err}`, "error");
        listEl.innerHTML = `<div class="error small">${escapeHtml(err)}</div>`;
      } finally {
        busy = false;
      }
    };

    const loadFile = async (path) => {
      if (busy) return;
      busy = true;
      const name = normalizeSlashes(path).split("/").pop();
      // Scroll preview to top so the new file's preview-head (path +
      // Open-in-default-app button) is visible immediately. Without
      // this, if the previous preview had been scrolled down, the user
      // would think nothing happened when they clicked a new file (rc49).
      previewEl.innerHTML = `<div class="muted small">Loading ${escapeHtml(
        name
      )}…</div>`;
      previewEl.scrollTop = 0;
      setStatus(`Reading ${path}…`);
      const kind = classifyKind(path);

      try {
        if (kind === "image") {
          const result = await invoke("mc_ui_read_image", { path });
          previewEl.innerHTML = `
            <div class="preview-head">
              <div class="preview-path" title="${escapeHtml(result.path)}">${escapeHtml(
            result.path
          )}</div>
              <div class="preview-meta muted small">
                ${escapeHtml(result.mime)} · ${formatBytes(result.bytes)}
                <button class="link-button" id="preview-open">Open in default app</button>
                ${sendToChatBtnHtml}
              </div>
            </div>
            <div class="preview-image">
              <img src="data:${result.mime};base64,${result.data_base64}" alt="${escapeHtml(
            name
          )}" />
            </div>
          `;
          wireOpenButton(path);
          setStatus("");
        } else if (kind === "binary") {
          // Don't even try to read; just show "open externally" handoff.
          previewEl.innerHTML = `
            <div class="preview-head">
              <div class="preview-path" title="${escapeHtml(path)}">${escapeHtml(
            path
          )}</div>
              <div class="preview-meta muted small">Binary file (.${
                extOf(path) || "?"
              })</div>
            </div>
            <div class="preview-binary">
              <div class="preview-binary-icon">📦</div>
              <div class="preview-binary-msg">
                MC can't preview this file type.<br>
                Open it in your computer's default app to view it.
              </div>
              <button class="primary" id="preview-open">Open in default app</button>
              ${sendToChatBtnHtml}
            </div>
          `;
          wireOpenButton(path);
          setStatus("Binary file — open externally to view.");
        } else {
          // Text-ish path: read, but if we get garbage (high non-printable
          // ratio) fall back to the binary handoff.
          const result = await invoke("mc_ui_read_file", {
            path,
            maxBytes: 524288,
          });
          // Quick sanity probe — read up to the first 4 KB and look at the
          // raw bytes. UTF-8 lossy mode (String.from_utf8_lossy) replaces
          // invalid bytes with U+FFFD which counts as printable in our
          // heuristic but doesn't have the high-byte ratio of real binary.
          // Cheap probe: count replacement chars.
          const replacementCount = (result.content.match(/\uFFFD/g) || [])
          .length;
          if (
            result.content.length > 0 &&
              replacementCount / result.content.length > 0.02
            ) {
            // Looks like a binary file with a non-listed extension.
            previewEl.innerHTML = `
              <div class="preview-head">
                <div class="preview-path" title="${escapeHtml(
                  result.path
                )}">${escapeHtml(result.path)}</div>
                <div class="preview-meta muted small">${formatBytes(
                  result.bytes
                )} — binary content detected</div>
              </div>
              <div class="preview-binary">
                <div class="preview-binary-icon">📦</div>
                <div class="preview-binary-msg">
                  This file looks like binary data.<br>
                  Open it in your computer's default app to view it.
                </div>
                <button class="primary" id="preview-open">Open in default app</button>
                ${sendToChatBtnHtml}
              </div>
            `;
            wireOpenButton(path);
            setStatus("Binary file — open externally to view.");
          } else {
            const truncated = result.truncated
              ? `<span class="badge">truncated at ${formatBytes(524288)}</span>`
              : "";
            previewEl.innerHTML = `
              <div class="preview-head">
                <div class="preview-path" title="${escapeHtml(result.path)}">${escapeHtml(
              result.path
            )}</div>
                <div class="preview-meta muted small">
                  ${formatBytes(result.bytes)} ${truncated}
                  <button class="link-button" id="preview-open">Open in default app</button>
                  ${sendToChatBtnHtml}
                </div>
              </div>
              <div class="preview-body">${renderTextWithLineNumbers(
                result.content
              )}</div>
            `;
            wireOpenButton(path);
            setStatus("");
          }
        }
      } catch (err) {
        previewEl.innerHTML = `<div class="error small">${escapeHtml(
          err
        )}</div>`;
        setStatus(`Error: ${err}`, "error");
      } finally {
        busy = false;
      }
    };

    const wireOpenButton = (path) => {
      const btn = previewEl.querySelector("#preview-open");
      if (!btn) return;
      btn.addEventListener("click", async () => {
        try {
          await invoke("mc_ui_open_externally", { path });
          setStatus(`Opened ${normalizeSlashes(path)} in default app.`);
        } catch (err) {
          setStatus(`Could not open externally: ${err}`, "error");
        }
      });
      // Lesson 574: also wire the Add-to-Chat button if present in this
      // preview-head (sender helper, mirrors the per-row Add to Chat).
      wireSendToChatButton(path);
    };

    // Lesson 574 (2026-08-25 08:19 MDT, David): preview-head "Add to
    // Chat" button. Emits mc:files:add-to-chat; recipient pages (terminal
    // chat input, dashboard chat input) listen and format a clean mention.
    const wireSendToChatButton = (path) => {
      const btn = previewEl.querySelector("#preview-send-to-chat");
      if (!btn) return;
      btn.addEventListener("click", () => {
        document.dispatchEvent(
          new CustomEvent("mc:files:add-to-chat", {
            detail: { path, source: "files-preview-head" },
          })
        );
        setStatus(`Added ${normalizeSlashes(path)} to chat input.`);
      });
    };

    (async () => {
      try {
        roots = await invoke("mc_ui_list_allowed_roots");
        renderRoots();
        if (roots.length) {
          await loadDir(roots[0]);
        } else {
          setStatus("No allowed roots. Check the path allowlist.", "error");
        }
      } catch (err) {
        setStatus(`Error: ${err}`, "error");
      }
    })();
  },

  unmount() {
    // No persistent state to clean up — we don't keep polling timers.
  },
};
