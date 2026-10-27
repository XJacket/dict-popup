#requires -Version 5.1
<#
.SYNOPSIS
    Update the copy installed under %ProgramFiles% by relaunching elevated.

.DESCRIPTION
    Windows shows a UAC consent dialog; the update only happens if it is
    approved. The elevated body is scripts\apply-update.ps1, which writes its
    outcome to dist\update-result.txt.

.EXAMPLE
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\request-update.ps1
#>
[CmdletBinding()]
param(
    [string]$Source,
    [string]$ResultFile
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
if (-not $Source) { $Source = Join-Path $Root 'dist\dict-popup-uiaccess.exe' }
if (-not $ResultFile) { $ResultFile = Join-Path $Root 'dist\update-result.txt' }

if (-not (Test-Path -LiteralPath $Source)) {
    throw "not found: $Source (build the UIAccess variant first: build.bat /uiaccess, then sign it)"
}
if (Test-Path -LiteralPath $ResultFile) { Remove-Item -LiteralPath $ResultFile -Force }

$body = Join-Path $PSScriptRoot 'apply-update.ps1'
$arguments = @(
    '-NoProfile'
    '-ExecutionPolicy', 'Bypass'
    '-File', ('"{0}"' -f $body)
    '-Source', ('"{0}"' -f $Source)
    '-ResultFile', ('"{0}"' -f $ResultFile)
)

Write-Host '[update] requesting elevation (approve the UAC prompt)'
Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs -Wait
Write-Host "[update] elevated step finished; result file: $ResultFile"
if (Test-Path -LiteralPath $ResultFile) {
    Get-Content -LiteralPath $ResultFile | ForEach-Object { Write-Host "  $_" }
} else {
    Write-Warning 'no result file was written; the elevated step may have been cancelled'
}
