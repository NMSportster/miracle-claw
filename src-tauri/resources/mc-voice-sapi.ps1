# Miracle Claw — SAPI presence check (called from installer.nsi post-install)
# SAPI 5 is built into Windows 10/11 by default but stripped from N/KN
# editions (Europe/Korea SKUs without media player). If missing, attempt
# to enable via DISM Optional Feature; non-fatal if that also fails
# (Whisper.cpp offline STT still works).
$ErrorActionPreference = 'SilentlyContinue'
$sapi = Join-Path $env:windir 'System32\Speech\Common\sapi.dll'
if (-not (Test-Path $sapi)) {
    Write-Host '[MC-Voice] SAPI missing, attempting install via DISM'
    dism /Online /Enable-Feature /FeatureName:SpeechRec /All /NoRestart | Out-Null
} else {
    Write-Host '[MC-Voice] SAPI present'
}