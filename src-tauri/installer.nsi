; Custom NSIS installer template for Miracle Claw.
;
; Lesson 430 — NSIS file lock fix: when upgrading an existing install,
; `node.exe` is held open by the running gateway. Tauri's default NSIS
; installer tries to overwrite it and fails with "Error opening file for
; writing: node.exe" — presenting an "Abort / Retry / Ignore" dialog the
; user can't get past.
;
; Fix: in `customInstall`, taskkill any running `miracle-claw.exe` and
; `node.exe` processes before the file copy begins. Sleep 2 seconds to
; let the OS release the file handles. If the kill fails (e.g. no such
; process), the ExecToLog log will show "ERROR" but the install proceeds
; — this is the same as before, and the user gets a clear upgrade path
; with the running process.
;
; This template is wired in via `tauri.conf.json:
; bundle.windows.nsis.template` (path relative to src-tauri/).

!macro customInstall
    ; Kill any running MC process tree (which owns node.exe via the
    ; launcher sidecar). /F = force, /T = tree (kill child procs too).
    nsExec::ExecToLog 'taskkill /F /IM miracle-claw.exe /T'
    ; Kill any orphaned node.exe from a previous install that may not have
    ; been cleaned up when the user closed MC.
    nsExec::ExecToLog 'taskkill /F /IM node.exe /T'
    ; Give the OS a moment to release file handles.
    Sleep 2000
!macroend
