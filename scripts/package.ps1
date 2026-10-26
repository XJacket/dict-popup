#requires -Version 5.1
<#
.SYNOPSIS
    Builds dict-popup and assembles the Windows release folder and zip.

.DESCRIPTION
    Runs build.bat (unless -SkipBuild), then stages

        dist\dict-popup-<version>-win64\
            dict-popup.exe
            README.md
            LICENSE
            CHANGELOG.md
            config.example.json

    and zips the folder to dist\dict-popup-<version>-win64.zip.
    The script is idempotent: re-running replaces the staging folder and the
    zip. It only ever deletes inside dist\.

    The build is rejected if the executable has no VERSIONINFO resource, which
    catches a link step that silently dropped assets\dict-popup.res.

.PARAMETER Version
    Release version used in the folder/zip names. Defaults to 0.1.0 and must
    be the major.minor.patch prefix of the exe's FileVersion.

.PARAMETER SkipBuild
    Reuse the already-built executable instead of invoking build.bat.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1

.EXAMPLE
    $env:CARGO_TARGET_DIR = 'F:\project\translation\.release-check'
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
#>
[CmdletBinding()]
param(
    [string]$Version = '0.2.1',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$DistDir = [System.IO.Path]::GetFullPath((Join-Path $Root 'dist'))
$StageName = "dict-popup-$Version-win64"
$StageDir = [System.IO.Path]::GetFullPath((Join-Path $DistDir $StageName))
$ZipPath = [System.IO.Path]::GetFullPath((Join-Path $DistDir "$StageName.zip"))

function Assert-InsideDist {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $prefix = $DistDir.TrimEnd('\') + '\'
    if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to modify '$full' (outside $DistDir)"
    }
}

function Get-ReleaseExe {
    if ($env:CARGO_TARGET_DIR) {
        $target = $env:CARGO_TARGET_DIR
        if (-not [System.IO.Path]::IsPathRooted($target)) {
            $target = Join-Path $Root $target
        }
    } else {
        $target = Join-Path $Root 'dict-popup\target'
    }
    return [System.IO.Path]::GetFullPath((Join-Path $target 'release\dict-popup.exe'))
}

# --- build --------------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host '[package] building via build.bat...'
    Push-Location $Root
    try {
        & cmd.exe /c build.bat
        if ($LASTEXITCODE -ne 0) {
            throw "build.bat failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Host '[package] -SkipBuild: reusing the existing executable.'
}

$exe = Get-ReleaseExe
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "release executable not found: $exe (run build.bat first)"
}

# --- gate: version resource ---------------------------------------------------
$info = (Get-Item -LiteralPath $exe).VersionInfo
if (-not $info.FileVersion) {
    throw "no VERSIONINFO in $exe - the resource link step did not run; rebuild with build.bat"
}
if ($info.FileVersion -notlike "$Version.*") {
    Write-Warning "exe FileVersion $($info.FileVersion) does not match -Version $Version"
}
Write-Host ("[package] exe: {0}  ({1:N0} bytes, FileVersion {2})" -f $exe, (Get-Item -LiteralPath $exe).Length, $info.FileVersion)

$exeMB = (Get-Item -LiteralPath $exe).Length / 1MB
if ($exeMB -gt 15) {
    Write-Warning ("exe is {0:N1} MB, above the 15 MB release threshold" -f $exeMB)
}

# --- stage --------------------------------------------------------------------
$payload = @('README.md', 'LICENSE', 'CHANGELOG.md', 'config.example.json')
foreach ($name in $payload) {
    $src = Join-Path $Root $name
    if (-not (Test-Path -LiteralPath $src -PathType Leaf)) {
        throw "required release file is missing: $src"
    }
}

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
if (Test-Path -LiteralPath $StageDir) {
    Assert-InsideDist $StageDir
    Remove-Item -LiteralPath $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $StageDir 'dict-popup.exe') -Force
foreach ($name in $payload) {
    Copy-Item -LiteralPath (Join-Path $Root $name) -Destination (Join-Path $StageDir $name) -Force
}

# --- zip ----------------------------------------------------------------------
if (Test-Path -LiteralPath $ZipPath) {
    Assert-InsideDist $ZipPath
    Remove-Item -LiteralPath $ZipPath -Force
}
Compress-Archive -Path (Join-Path $StageDir '*') -DestinationPath $ZipPath -CompressionLevel Optimal

# --- report -------------------------------------------------------------------
Write-Host ''
Write-Host "[package] $StageDir"
Get-ChildItem -LiteralPath $StageDir | ForEach-Object {
    Write-Host ("  {0,-24} {1,10:N0} bytes" -f $_.Name, $_.Length)
}
Write-Host ("[package] zip: {0}  ({1:N0} bytes)" -f $ZipPath, (Get-Item -LiteralPath $ZipPath).Length)
Write-Host "[package] OK"
