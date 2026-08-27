# Miracle Claw — Voice setup script (called from installer.nsi post-install)
# Iterates the user's preferred UI languages (top 3), looks up the matching
# Windows Speech Recognition FOD, and installs it via dism if missing.
# Failures are logged to the installer log; nsExec::ExecToLog swallows them.
#
# Lesson TBD (2026-08-27): we bundle this as a separate file rather than
# passing inline via powershell -Command because NSIS single-quoted
# strings can't contain single quotes, which PowerShell scripts need.
$ErrorActionPreference = 'SilentlyContinue'
try {
    $langs = @(Get-WinUserLanguageList).LanguageTag | Select-Object -First 3
    foreach ($l in $langs) {
        $bcp = $l -replace '-', '_'
        $cap = "Language.Speech~~~und-SPEECH~~$bcp"
        $state = (Get-WindowsCapability -Online -Name $cap -ErrorAction SilentlyContinue).State
        if ($state -ne 'Installed') {
            Write-Host "[MC-Voice] Adding speech FOD: $cap"
            $p = Start-Process -FilePath 'dism' -ArgumentList @('/Online','/Add-Capability',"/CapabilityName:$cap",'/NoRestart') -Wait -PassThru -WindowStyle Hidden
            if ($p.ExitCode -ne 0) {
                Write-Host "[MC-Voice] FOD install failed (code $($p.ExitCode)) for $cap"
            }
        } else {
            Write-Host "[MC-Voice] Speech FOD already installed: $cap"
        }
    }
} catch {
    Write-Host "[MC-Voice] FOD enumeration failed: $($_.Exception.Message)"
}