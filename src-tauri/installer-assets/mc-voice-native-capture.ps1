# Miracle Claw — Native Windows Speech Recognition capture
# (called from Rust `mc_voice_native_capture` Tauri command via std::process::Command)
#
# v1.1.0-rc54.3: replaces the whisper.cpp sidecar path for the OpenClaw
# chat voice button. Uses Windows built-in System.Speech.Recognition
# (SAPI 5) with `SetInputToDefaultAudioDevice()` to capture audio from
# the user's default Windows microphone and transcribe it locally.
#
# Why this beats whisper.cpp:
#   - 10-50x faster (200-500ms vs 2-6s for short utterances)
#   - Zero model download (no 75-141 MB ggml-*.bin)
#   - No cloud round-trip — fully offline, free for all users
#   - Already installed by the rc54.x voice setup (FOD + SAPI 5)
#   - Microsoft Speech Platform handles partial hypotheses natively
#     (we don't expose them — we only return final text)
#
# Parameters:
#   $args[0] = timeout_ms (string, default 15000)
#
# Output: JSON object on stdout
#   {"text": "hello world", "confidence": 0.92, "engine": "System.Speech.Recognition"}
#
# Error handling: errors go to stderr, exit code is non-zero. The Rust
# caller maps stderr to a friendly error message for the JS side.

param([int]$TimeoutMs = 15000)

$ErrorActionPreference = 'Stop'

# ---- Argument parsing ------------------------------------------------------
# PowerShell `param()` only declares named parameters, but we still
# accept positional `$args[0]` so callers can use `script.ps1 15000`.
if ($args.Count -ge 1) {
    try {
        $parsed = [int]$args[0]
        if ($parsed -gt 0 -and $parsed -le 60000) { $TimeoutMs = $parsed }
    } catch {}
}

# ---- SAPI 5 presence check --------------------------------------------------
$sapi = Join-Path $env:windir 'System32\Speech\Common\sapi.dll'
if (-not (Test-Path $sapi)) {
    [Console]::Error.WriteLine('SAPI 5 not installed (sapi.dll missing). Run Windows Optional Features and enable "Speech Recognition".')
    exit 2
}

# ---- Load assembly ----------------------------------------------------------
Add-Type -AssemblyName System.Speech

# ---- Build recognizer -------------------------------------------------------
# We use SpeechRecognitionEngine (in-process) instead of SpeechRecognizer
# (shared, dictation UI pops up) because we want full programmatic control.
# SetInputToDefaultAudioDevice() uses the OS default capture device —
# the same "Windows microphone" the OS Settings panel shows.
$engine = New-Object System.Speech.Recognition.SpeechRecognitionEngine
try {
    $engine.SetInputToDefaultAudioDevice()
} catch {
    [Console]::Error.WriteLine("Could not open default microphone: $($_.Exception.Message)")
    exit 3
}

# Load a free-text dictation grammar. No need for grammar files — the
# DictationGrammar() class handles arbitrary English (and any other
# installed language) text. Users get full STT without configuration.
$grammar = New-Object System.Speech.Recognition.DictationGrammar
$engine.LoadGrammar($grammar)

# ---- Configure wait ---------------------------------------------------------
# RecognizeAsync() returns after the user pauses (~700ms silence) by
# default. We translate the RecognizeCompleted event into a synchronous
# wait via AutoResetEvent. The handler stores its result into script-
# scope variables; we read them after the WaitOne returns.
$done = New-Object System.Threading.AutoResetEvent($false)

# Set up script-scope variables BEFORE registering the handler so the
# closure captures them by reference (PS closures are late-bound).
$script:resultText = ''
$script:resultConfidence = 0.0

$engine.add_RecognizeCompleted({
    param($sender, $e)
    $script:resultText = $e.Result.Text
    $script:resultConfidence = [double]$e.Result.Confidence
    $script:doneEvent.Set() | Out-Null
})
$script:doneEvent = $done

# ---- Start recognition ------------------------------------------------------
try {
    $engine.RecognizeAsync($null) | Out-Null
} catch {
    [Console]::Error.WriteLine("RecognizeAsync failed: $($_.Exception.Message)")
    $engine.Dispose()
    exit 4
}

# Wait for completion or timeout.
$signaled = $done.WaitOne($TimeoutMs)
$engine.RecognizeAsyncStop() | Out-Null

if (-not $signaled) {
    [Console]::Error.WriteLine("Recognition timed out after ${TimeoutMs}ms (no speech detected).")
    $engine.Dispose()
    exit 5
}

# ---- Cleanup ----------------------------------------------------------------
$engine.Dispose()

# ---- Emit JSON --------------------------------------------------------------
# We intentionally use a small hand-rolled JSON encoder instead of
# ConvertTo-Json because the latter can add unicode escapes and
# trailing whitespace that complicate downstream Rust parsing.
# Confidence is rounded to 4 decimals to keep the output stable.
$text = [string]$script:resultText
$confidence = [double]$script:resultConfidence
$safeText = ($text -replace '\\', '\\\\') -replace '"', '\"'
$confidenceStr = ([math]::Round($confidence, 4)).ToString([System.Globalization.CultureInfo]::InvariantCulture)
$out = '{' +
       '"text":"' + $safeText + '",' +
       '"confidence":' + $confidenceStr + ',' +
       '"engine":"System.Speech.Recognition"' +
       '}'
Write-Output $out
exit 0
