#requires -Version 5.1
<#
.SYNOPSIS
    Elevated body of the installed-copy update. Run via request-update.ps1.
.DESCRIPTION
    Replaces the exe under %ProgramFiles%\dict-popup with the given build, then
    verifies the result and writes a machine-readable status file the caller can
    poll. Must run elevated.
#>
[CmdletBinding()]
param(
    [string]$Source = 'F:\project\translation\dist\dict-popup-uiaccess.exe',
    [string]$TargetDir = "$env:ProgramFiles\dict-popup",
    [string]$ResultFile = 'F:\project\translation\dist\update-result.txt'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add('started=' + (Get-Date).ToString('o'))

try {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'not elevated'
    }

    if (-not (Test-Path -LiteralPath $Source)) { throw "source not found: $Source" }

    $sig = Get-AuthenticodeSignature -LiteralPath $Source
    $lines.Add('source_signature=' + $sig.Status)
    if ($sig.Status -ne 'Valid') { throw "source signature is $($sig.Status)" }

    $bytes = [System.IO.File]::ReadAllBytes($Source)
    $text = [System.Text.Encoding]::UTF8.GetString($bytes)
    $lines.Add('source_uiaaccess=' + $text.Contains('uiAccess="true"').ToString().ToLower())
    if (-not $text.Contains('uiAccess="true"')) { throw 'source has no uiAccess="true" manifest' }

    $version = (Get-Item -LiteralPath $Source).VersionInfo.FileVersion
    $lines.Add('source_version=' + $version)

    Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | ForEach-Object {
        $lines.Add('stopping_pid=' + $_.Id)
        Stop-Process -Id $_.Id -Force
    }
    Start-Sleep -Milliseconds 800

    $target = Join-Path $TargetDir 'dict-popup.exe'
    Copy-Item -LiteralPath $Source -Destination $target -Force
    $lines.Add('copied_to=' + $target)

    $installed = Get-Item -LiteralPath $target
    $lines.Add('installed_version=' + $installed.VersionInfo.FileVersion)
    $lines.Add('installed_signature=' + (Get-AuthenticodeSignature -LiteralPath $target).Status)

    $installedBytes = [System.IO.File]::ReadAllBytes($target)
    $installedText = [System.Text.Encoding]::UTF8.GetString($installedBytes)
    $lines.Add('installed_uiaaccess=' + $installedText.Contains('uiAccess="true"').ToString().ToLower())

    $lines.Add('result=ok')
}
catch {
    $lines.Add('result=failed')
    $lines.Add('error=' + $_.Exception.Message)
}
finally {
    $lines.Add('finished=' + (Get-Date).ToString('o'))
    try {
        [System.IO.File]::WriteAllLines($ResultFile, $lines)
    } catch {
        # If the status file cannot be written, the caller falls back to
        # inspecting the installed version itself.
    }
}
