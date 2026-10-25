#requires -Version 5.1
<#
.SYNOPSIS
    Installs dict-popup as a UIAccess application under %ProgramFiles%.

.DESCRIPTION
    A UIAccess executable must satisfy four conditions at once:

      * its manifest sets uiAccess="true";
      * it is Authenticode-signed by a certificate trusted by the machine;
      * it lives in a protected location (%ProgramFiles%);
      * UAC and the "only elevate UIAccess applications installed in secure
        locations" policy are enabled.

    This script checks the first three before touching anything. It refuses
    unsigned binaries and normal (uiAccess="false") builds: Windows refuses to
    start an unsigned uiAccess="true" binary, so installing one would only
    produce a shortcut that does nothing.

    The script installs to

        %ProgramFiles%\dict-popup\dict-popup.exe
        %ProgramData%\Microsoft\Windows\Start Menu\Programs\dict-popup.lnk

    and stops a running dict-popup instance first. It is idempotent. It only
    removes files inside -InstallDir plus the one shortcut it creates.

.PARAMETER Exe
    Binary to install. Defaults to dist\dict-popup.exe in this repository.
    It must contain the manifest token uiAccess="true" and have a valid
    Authenticode signature.

.PARAMETER InstallDir
    Destination directory. Defaults to %ProgramFiles%\dict-popup. Install
    refuses a directory outside %ProgramFiles% because UIAccess would not be
    granted there.

.PARAMETER Uninstall
    Remove the shortcut and -InstallDir. User configuration in
    %APPDATA%\dict-popup is kept unless -PurgeConfig is also given.

.PARAMETER PurgeConfig
    Together with -Uninstall, also delete %APPDATA%\dict-popup, which contains
    config.json, config.json.bak and dict-popup.log.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Exe .\dist\dict-popup.exe

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Uninstall

.NOTES
    Run from an elevated PowerShell. See docs\UIAACCESS.md for the full
    procedure, verification steps and troubleshooting.
#>
[CmdletBinding()]
param(
    [string]$Exe = '',
    [string]$InstallDir = "$env:ProgramFiles\dict-popup",
    [switch]$Uninstall,
    [switch]$PurgeConfig
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# $PSScriptRoot is not reliably populated while parameter defaults are being
# evaluated under Windows PowerShell 5.1, so resolve the default here instead.
if ([string]::IsNullOrWhiteSpace($Exe)) {
    if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        throw "cannot determine the script directory; pass -Exe explicitly."
    }
    $Exe = Join-Path (Split-Path -Parent $PSScriptRoot) 'dist\dict-popup.exe'
}

$script:ProgramFilesRoot = [System.Environment]::GetFolderPath('ProgramFiles')
if ([string]::IsNullOrWhiteSpace($script:ProgramFilesRoot)) {
    $script:ProgramFilesRoot = $env:ProgramFiles
}
$script:ShortcutPath = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\dict-popup.lnk'
$script:ConfigDir = Join-Path ([System.Environment]::GetFolderPath('ApplicationData')) 'dict-popup'

function Write-Step {
    param([string]$Message)
    Write-Host "[install] $Message"
}

function Get-AbsolutePath {
    param(
        [string]$Path,
        [string]$Name
    )
    if ([string]::IsNullOrWhiteSpace($Path)) {
        throw "$Name must not be empty."
    }
    if (-not [System.IO.Path]::IsPathRooted($Path)) {
        $Path = Join-Path (Get-Location).Path $Path
    }
    return [System.IO.Path]::GetFullPath($Path)
}

function Assert-Elevated {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object -TypeName System.Security.Principal.WindowsPrincipal -ArgumentList $identity
    if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "administrator rights are required: %ProgramFiles% and the Start Menu are protected. Open PowerShell with 'Run as administrator' and run this script again."
    }
}

function Assert-SafeInstallDir {
    param([string]$Path)
    $trimmed = $Path.TrimEnd('\')
    $root = [System.IO.Path]::GetPathRoot($Path).TrimEnd('\')
    if ($trimmed -ieq $root) {
        throw "refusing to use a drive root as -InstallDir: $Path"
    }

    $protected = @(
        $script:ProgramFilesRoot,
        [System.Environment]::GetFolderPath('Windows'),
        [System.Environment]::GetFolderPath('UserProfile'),
        [System.Environment]::GetFolderPath('ApplicationData')
    )
    foreach ($candidate in $protected) {
        if (-not [string]::IsNullOrWhiteSpace($candidate)) {
            if ($trimmed -ieq $candidate.TrimEnd('\')) {
                throw "refusing to use a protected directory as -InstallDir: $Path"
            }
        }
    }
}

function Assert-UnderProgramFiles {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($script:ProgramFilesRoot)) {
        throw "cannot determine %ProgramFiles%; pass -InstallDir explicitly."
    }
    $programFiles = [System.IO.Path]::GetFullPath($script:ProgramFilesRoot).TrimEnd('\')
    $target = $Path.TrimEnd('\')
    if (-not ($target + '\').StartsWith($programFiles + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "UIAccess requires the executable under %ProgramFiles%. -InstallDir '$Path' is outside '$script:ProgramFilesRoot'; use the default or a subdirectory of Program Files."
    }
}

function Test-UiAccessToken {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    return $text.Contains('uiAccess="true"')
}

function Test-UiAccessFalse {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    return $text.Contains('uiAccess="false"')
}

function Test-CertInMachineRoot {
    param([string]$Thumbprint)
    $store = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Store -ArgumentList @('Root', [System.Security.Cryptography.X509Certificates.StoreLocation]::LocalMachine)
    try {
        $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
        foreach ($certificate in $store.Certificates) {
            if ($certificate.Thumbprint -eq $Thumbprint) {
                return $true
            }
        }
        return $false
    } finally {
        $store.Close()
    }
}

function Stop-RunningApp {
    $processes = @(Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue)
    if ($processes.Count -eq 0) {
        return
    }
    $ids = ($processes | ForEach-Object { $_.Id }) -join ', '
    Write-Step "stopping running dict-popup (PID $ids)"
    foreach ($process in $processes) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        $stillRunning = @(Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue)
        if ($stillRunning.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 100
    }
    throw "dict-popup is still running; close it and try again."
}

try {
    $InstallDir = Get-AbsolutePath -Path $InstallDir -Name 'InstallDir'
    Assert-SafeInstallDir -Path $InstallDir
    $InstallDir = $InstallDir.TrimEnd('\')
    $InstalledExe = Join-Path $InstallDir 'dict-popup.exe'

    if ($PurgeConfig -and -not $Uninstall) {
        throw "-PurgeConfig only makes sense together with -Uninstall."
    }

    if ($Uninstall) {
        Assert-Elevated
        Write-Step "uninstalling dict-popup from $InstallDir"
        Stop-RunningApp

        if (Test-Path -LiteralPath $script:ShortcutPath -PathType Leaf) {
            Write-Step "removing shortcut: $script:ShortcutPath"
            Remove-Item -LiteralPath $script:ShortcutPath -Force
        } else {
            Write-Step "shortcut not present: $script:ShortcutPath"
        }

        if (Test-Path -LiteralPath $InstallDir -PathType Container) {
            $hasExe = Test-Path -LiteralPath $InstalledExe -PathType Leaf
            $entries = @(Get-ChildItem -LiteralPath $InstallDir -Force -ErrorAction Stop)
            if (-not $hasExe -and $entries.Count -gt 0) {
                throw "refusing to remove '$InstallDir': it does not contain dict-popup.exe and is not empty. Remove it manually if you are sure."
            }
            Write-Step "removing install directory: $InstallDir"
            Remove-Item -LiteralPath $InstallDir -Recurse -Force
        } else {
            Write-Step "install directory not present: $InstallDir"
        }

        if ($PurgeConfig) {
            if (Test-Path -LiteralPath $script:ConfigDir -PathType Container) {
                Write-Step "deleting configuration and log directory: $script:ConfigDir (config.json, config.json.bak, dict-popup.log)"
                Remove-Item -LiteralPath $script:ConfigDir -Recurse -Force
            } else {
                Write-Step "configuration directory not present: $script:ConfigDir"
            }
        } else {
            Write-Step "kept configuration and log: $script:ConfigDir (pass -PurgeConfig to delete)"
        }

        Write-Step "uninstall complete."
    } else {
        $Exe = Get-AbsolutePath -Path $Exe -Name 'Exe'
        if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
            throw "executable not found: $Exe (build it with 'build.bat /uiaccess' first)"
        }

        # Validate the source (read-only) before the elevation gate: the
        # "unsigned" diagnosis is the most useful one even when the shell is
        # not elevated. Nothing is written until Assert-Elevated below.
        $hasToken = Test-UiAccessToken -Path $Exe
        $signature = Get-AuthenticodeSignature -FilePath $Exe
        $signatureStatus = [string]$signature.Status

        if ($signatureStatus -eq 'NotSigned') {
            throw "'$Exe' is unsigned (Authenticode status: NotSigned). Windows refuses to start a binary whose manifest sets uiAccess=""true"" unless it is signed and trusted, so installing it would only create a broken shortcut. Sign the build first (see docs\SIGNING.md), then run this script again."
        }
        if ($signatureStatus -ne 'Valid') {
            throw "'$Exe' does not have a valid Authenticode signature (status: $signatureStatus). UIAccess requires a signature trusted by this machine; see docs\SIGNING.md. Status message: $($signature.StatusMessage)"
        }
        if (-not $hasToken) {
            if (Test-UiAccessFalse -Path $Exe) {
                throw "'$Exe' is a normal build: its manifest sets uiAccess=""false"". Rebuild with 'build.bat /uiaccess', sign the result, then install that file."
            }
            throw "'$Exe' does not contain the manifest token uiAccess=""true"". Build the UIAccess variant with 'build.bat /uiaccess' and sign it before installing."
        }

        $signer = $signature.SignerCertificate
        if ($null -eq $signer) {
            throw "'$Exe' reports a valid signature but exposes no signer certificate; treat the signing setup as broken."
        }

        Write-Step "source exe: $Exe"
        Write-Step "signature: Valid"
        Write-Step "signer: $($signer.Subject)"
        Write-Step "signer thumbprint: $($signer.Thumbprint)"

        if ($signer.Subject -eq $signer.Issuer) {
            if (Test-CertInMachineRoot -Thumbprint $signer.Thumbprint) {
                Write-Step 'signer certificate found in LocalMachine\Root'
            } else {
                Write-Warning "[install] the self-signed signer certificate is not in LocalMachine\Root. UIAccess validates machine-wide trust, so a CurrentUser\Root copy is not enough and the app will start without the UIAccess token. Import the certificate into LocalMachine\Root from an elevated prompt, for example: Import-Certificate -FilePath <cert.cer> -CertStoreLocation Cert:\LocalMachine\Root"
            }
        }

        Assert-Elevated
        Assert-UnderProgramFiles -Path $InstallDir

        Stop-RunningApp

        Write-Step "creating install directory: $InstallDir"
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

        if ($Exe -ieq $InstalledExe) {
            Write-Step "source and destination are the same file; skipping copy"
        } else {
            Write-Step "copying exe to $InstalledExe"
            Copy-Item -LiteralPath $Exe -Destination $InstalledExe -Force
        }

        Write-Step "verifying the installed file"
        $installedSignature = Get-AuthenticodeSignature -FilePath $InstalledExe
        $installedStatus = [string]$installedSignature.Status
        if ($installedStatus -ne 'Valid') {
            throw "the installed file failed signature verification (status: $installedStatus). The copy may be corrupt; remove '$InstallDir' and try again."
        }
        if (-not (Test-UiAccessToken -Path $InstalledExe)) {
            throw "the installed file no longer contains uiAccess=""true""; the copy is corrupt."
        }

        Write-Step "creating Start Menu shortcut: $script:ShortcutPath"
        $shortcutParent = Split-Path -Parent $script:ShortcutPath
        if (-not (Test-Path -LiteralPath $shortcutParent -PathType Container)) {
            New-Item -ItemType Directory -Force -Path $shortcutParent | Out-Null
        }

        $shell = New-Object -ComObject WScript.Shell
        try {
            $link = $shell.CreateShortcut($script:ShortcutPath)
            $link.TargetPath = $InstalledExe
            $link.WorkingDirectory = $InstallDir
            $link.IconLocation = "$InstalledExe,0"
            $link.Description = 'dict-popup (UIAccess)'
            $link.Save()

            $check = $shell.CreateShortcut($script:ShortcutPath)
            if ($check.TargetPath -ne $InstalledExe) {
                throw "shortcut verification failed: target is '$($check.TargetPath)', expected '$InstalledExe'."
            }
            if ($check.WorkingDirectory -ne $InstallDir) {
                throw "shortcut verification failed: working directory is '$($check.WorkingDirectory)', expected '$InstallDir'."
            }
        } finally {
            [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
        }

        Write-Step "installed to $InstallDir"
        Write-Step "shortcut target and working directory verified"
        Write-Step "manifest: uiAccess=""true"" (checked in source and installed file)"
        Write-Step "expected token state: uiaccess=true"
        Write-Host ''
        Write-Step "now verify by hand:"
        Write-Host '[install]   1. Launch "dict-popup" from the Start Menu WITHOUT elevation'
        Write-Host '[install]      (do not use "Run as administrator").'
        Write-Host '[install]   2. In %APPDATA%\dict-popup\dict-popup.log, the "privilege:" line'
        Write-Host '[install]      must report uiaccess=true.'
        Write-Host '[install]   The real test: run Notepad as administrator, select text there and press the hotkey.'
        Write-Host ''
        Write-Step "done. Uninstall with: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Uninstall"
    }
} catch {
    Write-Host "[install] ERROR: $($_.Exception.Message)"
    exit 1
}
