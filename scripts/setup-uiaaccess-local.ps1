#requires -Version 5.1
<#
.SYNOPSIS
    End-to-end local validation of the UIAccess build.

.DESCRIPTION
    Builds the UIAccess variant, signs it with a self-signed code-signing
    certificate, trusts that certificate machine-wide (which UIAccess requires),
    and installs the binary under %ProgramFiles%.

    A self-signed certificate only convinces THIS machine. It is enough to prove
    whether UIAccess solves the input-injection problem; the public release will
    be signed by SignPath Foundation instead (docs/SIGNING.md).

    Must run elevated.

.EXAMPLE
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\setup-uiaaccess-local.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$Exe = Join-Path $Root 'dist\dict-popup.exe'

function Assert-Elevated {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "This script must run elevated (it writes to LocalMachine certificate stores and %ProgramFiles%)."
    }
}

Write-Host '[uia] step 1/5: elevation check'
Assert-Elevated

if (-not $SkipBuild) {
    Write-Host '[uia] step 2/5: building the UIAccess variant'
    Push-Location $Root
    try {
        cmd /c "build.bat /uiaccess < nul"
        if ($LASTEXITCODE -ne 0) { throw "build.bat /uiaccess failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
} else {
    Write-Host '[uia] step 2/5: build skipped'
}

if (-not (Test-Path -LiteralPath $Exe)) { throw "not found: $Exe" }

# The manifest token must be present; an unsigned or non-UIAccess binary will
# either fail to start or silently change nothing.
$bytes = [System.IO.File]::ReadAllBytes($Exe)
$text = [System.Text.Encoding]::UTF8.GetString($bytes)
if (-not $text.Contains('uiAccess="true"')) {
    throw "$Exe does not contain uiAccess=`"true`". Build it with: build.bat /uiaccess"
}

Write-Host '[uia] step 3/5: signing with the local self-signed certificate'
$signScript = Join-Path $PSScriptRoot 'sign-release.ps1'
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $signScript -Exe $Exe
if ($LASTEXITCODE -ne 0) { throw "sign-release.ps1 failed with exit code $LASTEXITCODE" }

$signature = Get-AuthenticodeSignature -FilePath $Exe
if ($signature.Status -ne 'Valid') {
    throw "signature is $($signature.Status), expected Valid. $($signature.StatusMessage)"
}

Write-Host '[uia] step 4/5: trusting the certificate machine-wide'
$cerPath = Join-Path $Root 'assets\signing\dict-popup-codesign.cer'
if (-not (Test-Path -LiteralPath $cerPath)) { throw "certificate export not found: $cerPath" }
$cert = Get-Item -LiteralPath $cerPath | Import-Certificate -CertStoreLocation 'Cert:\LocalMachine\Root'
Write-Host "[uia]  trusted root: $($cert.Subject) [$($cert.Thumbprint)]"

# TrustedPublisher is not required for UIAccess, but it keeps the elevation
# prompt from reading "unknown publisher" during local testing.
$pubStore = New-Object System.Security.Cryptography.X509Certificates.X509Store('TrustedPublisher', 'LocalMachine')
$pubStore.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
try {
    $pubStore.Add($cert)
    Write-Host '[uia]  added to LocalMachine\TrustedPublisher'
} catch {
    Write-Warning "could not add to LocalMachine\TrustedPublisher: $($_.Exception.Message)"
} finally {
    $pubStore.Close()
}

if ($SkipInstall) {
    Write-Host '[uia] step 5/5: install skipped (-SkipInstall)'
} else {
    Write-Host '[uia] step 5/5: installing under %ProgramFiles%'
    $installer = Join-Path $PSScriptRoot 'install-uiaaccess.ps1'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer -Exe $Exe
    if ($LASTEXITCODE -ne 0) { throw "install-uiaaccess.ps1 failed with exit code $LASTEXITCODE" }
}

Write-Host ''
Write-Host '[uia] Done. Now verify WITHOUT elevation:'
Write-Host '  1. Start dict-popup from the Start Menu (do not use "Run as administrator").'
Write-Host '  2. Settings -> About must say: UIAccess: enabled'
Write-Host "  3. %APPDATA%\dict-popup\dict-popup.log must contain: privilege: uiaccess=true"
Write-Host '  4. Select a word in a Notepad started AS ADMINISTRATOR, press the hotkey.'
Write-Host ''
Write-Host '[uia] If step 3 still says uiaccess=false, see docs/UIAACCESS.md (troubleshooting).'
