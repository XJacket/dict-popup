#requires -Version 5.1
<#
.SYNOPSIS
    Samples dict-popup's Working Set and checks it against the 100 MB budget.

.DESCRIPTION
    Samples Process.WorkingSet64 every 250 ms for N seconds, prints
    min / avg / max in MB and a PASS or FAIL line against the threshold,
    then stops the process - but only when this script started it (-Launch).

    Nothing is launched unless -Launch is passed. Without it the script
    attaches to an already running dict-popup.exe and leaves it alone.

    A short warm-up window is skipped so that one-off start-up allocations do
    not count towards the steady-state budget.

    Exit code: 0 = PASS, 1 = FAIL (or an error).

.PARAMETER Seconds
    Sampling duration after warm-up. Default 20.

.PARAMETER IntervalMs
    Sampling interval in milliseconds. Default 250.

.PARAMETER ThresholdMB
    PASS/FAIL threshold for the maximum sample. Default 100.

.PARAMETER WarmupSeconds
    Seconds to wait after start before sampling begins. Default 2.

.PARAMETER ExePath
    Executable to launch with -Launch. Defaults to dist\dict-popup.exe.

.PARAMETER Launch
    Opt-in: start the GUI executable, sample it, and stop it afterwards.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure-memory.ps1 -Launch

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure-memory.ps1 -Seconds 60
#>
[CmdletBinding()]
param(
    [int]$Seconds = 20,
    [int]$IntervalMs = 250,
    [double]$ThresholdMB = 100,
    [double]$WarmupSeconds = 2,
    [string]$ExePath = '',
    [switch]$Launch
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# NB: $PSScriptRoot is empty inside parameter defaults on Windows PowerShell
# 5.1, so resolve the default path here instead.
if (-not $ExePath) {
    $ExePath = Join-Path (Split-Path -Parent $PSScriptRoot) 'dist\dict-popup.exe'
}

if ($Seconds -lt 1) { throw '-Seconds must be at least 1' }
if ($IntervalMs -lt 50) { throw '-IntervalMs must be at least 50' }
if ($WarmupSeconds -lt 0) { throw '-WarmupSeconds must not be negative' }

$proc = $null
$startedHere = $false
$expectedName = 'dict-popup'

try {
    if ($Launch) {
        $already = Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -ne $already) {
            throw "dict-popup is already running (PID $($already.Id)). Quit it first, or omit -Launch to sample the running instance."
        }
        $exe = [System.IO.Path]::GetFullPath($ExePath)
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
            throw "executable not found: $exe (run build.bat first, or pass -ExePath)"
        }
        Write-Host "[mem] launching $exe"
        # Use Process.Start instead of Start-Process: Start-Process goes through
        # ShellExecuteEx, which can block indefinitely for a GUI process when
        # PowerShell runs without an interactive shell (as it does in build
        # agents). UseShellExecute=false is a plain CreateProcess and returns
        # immediately, which is what we want.
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exe
        $psi.WorkingDirectory = Split-Path -Parent $exe
        $psi.UseShellExecute = $false
        $proc = [System.Diagnostics.Process]::Start($psi)
        $startedHere = $true
        $expectedName = [System.IO.Path]::GetFileNameWithoutExtension($exe)
    } else {
        $proc = Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -eq $proc) {
            throw 'dict-popup.exe is not running. Pass -Launch to start it (that opens the GUI).'
        }
        Write-Host "[mem] attaching to running PID $($proc.Id)"
    }

    if ($WarmupSeconds -gt 0) {
        Write-Host ("[mem] warm-up: {0:N1} s" -f $WarmupSeconds)
        $warmupEnd = [DateTime]::UtcNow.AddSeconds($WarmupSeconds)
        while ([DateTime]::UtcNow -lt $warmupEnd) {
            if ($proc.HasExited) { throw "dict-popup exited during warm-up (code $($proc.ExitCode))" }
            Start-Sleep -Milliseconds 100
        }
    }
    if ($proc.HasExited) { throw "dict-popup exited before sampling (code $($proc.ExitCode))" }

    $samples = New-Object System.Collections.Generic.List[double]
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.Elapsed.TotalSeconds -lt $Seconds) {
        $proc.Refresh()
        if ($proc.HasExited) { throw "dict-popup exited during sampling (code $($proc.ExitCode))" }
        $samples.Add($proc.WorkingSet64 / 1MB)
        Start-Sleep -Milliseconds $IntervalMs
    }
    $stopwatch.Stop()

    $stats = $samples | Measure-Object -Minimum -Maximum -Average
    $min = [double]$stats.Minimum
    $avg = [double]$stats.Average
    $max = [double]$stats.Maximum
    $pass = $max -le $ThresholdMB

    Write-Host ''
    Write-Host ("[mem] samples     : {0} x {1} ms over {2:N1} s" -f $samples.Count, $IntervalMs, $Seconds)
    Write-Host ("[mem] working set : min {0:N1} MB / avg {1:N1} MB / max {2:N1} MB" -f $min, $avg, $max)
    Write-Host ("[mem] threshold   : {0:N1} MB" -f $ThresholdMB)
    if ($pass) {
        Write-Host ("[mem] PASS (headroom {0:N1} MB)" -f ($ThresholdMB - $max))
    } else {
        Write-Host ("[mem] FAIL (max exceeds budget by {0:N1} MB)" -f ($max - $ThresholdMB))
    }
} finally {
    if ($startedHere -and $null -ne $proc) {
        # Stop only the process this script started, and only if it is still
        # alive and still the same image (guards against PID reuse).
        try {
            $proc.Refresh()
            if (-not $proc.HasExited -and $proc.ProcessName -eq $expectedName) {
                Stop-Process -Id $proc.Id -Force
                [void]$proc.WaitForExit(2000)
                Write-Host "[mem] stopped PID $($proc.Id)"
            }
        } catch {
            Write-Host "[mem] could not stop PID $($proc.Id): $($_.Exception.Message)"
        }
    }
}

if (-not $pass) { exit 1 }
