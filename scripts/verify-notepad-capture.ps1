#requires -Version 5.1
<#
.SYNOPSIS
    Verify clipboard capture against Notepad with a real selection.

.DESCRIPTION
    Opens Notepad with a word, selects it through UI Automation, forces Notepad
    into the foreground (AttachThreadInput), then runs dict-input's capture
    probe, which performs the same Ctrl+C + clipboard read the app does.

    powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File scripts\verify-notepad-capture.ps1
#>
param(
    [string]$Probe = 'F:\project\translation\dict-input\target\release\examples\capture_probe.exe',
    [string]$Word = 'apple'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Fg3 {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetCurrentThreadId();
}
"@

function Start-Detached([string]$File, [string]$Arguments) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $File
    $psi.Arguments = $Arguments
    $psi.UseShellExecute = $false
    [System.Diagnostics.Process]::Start($psi)
}

$tmp = Join-Path $env:TEMP 'dict-popup-verify.txt'
Set-Content -LiteralPath $tmp -Value $Word -Encoding ASCII

Write-Host "== opening notepad"
$before = @(Get-Process -Name Notepad -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$np = Start-Detached 'C:\Windows\System32\notepad.exe' "`"$tmp`""
Start-Sleep -Seconds 2

$win = Get-Process -Name Notepad -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -like '*dict-popup-verify*' } |
    Select-Object -First 1
if (-not $win) {
    $win = Get-Process -Name Notepad -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
}
if (-not $win) { throw "no notepad window" }
Write-Host "== notepad hwnd=$($win.MainWindowHandle) title='$($win.MainWindowTitle)'"

# Select the text through UIA.
$root = [System.Windows.Automation.AutomationElement]::FromHandle($win.MainWindowHandle)
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
function Find-TextElement($el, $depth) {
    if ($depth -gt 6) { return $null }
    $p = $null
    if ($el.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$p)) { return $el }
    $child = $walker.GetFirstChild($el)
    while ($child -ne $null) {
        $found = Find-TextElement $child ($depth + 1)
        if ($found) { return $found }
        $child = $walker.GetNextSibling($child)
    }
    return $null
}
$doc = Find-TextElement $root 0
if (-not $doc) { throw "notepad exposes no text pattern" }
$tp = $doc.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
$tp.DocumentRange.Select()
Write-Host "== selected: '$($tp.GetSelection()[0].GetText(30))'"

# Force Notepad to the foreground so SendInput lands there.
$fg = [Fg3]::GetForegroundWindow()
$pid2 = 0
$fgThread = [Fg3]::GetWindowThreadProcessId($fg, [ref]$pid2)
$myThread = [Fg3]::GetCurrentThreadId()
[void][Fg3]::AttachThreadInput($myThread, $fgThread, $true)
[void][Fg3]::SetForegroundWindow($win.MainWindowHandle)
[void][Fg3]::AttachThreadInput($myThread, $fgThread, $false)
Start-Sleep -Milliseconds 300
Write-Host "== foreground now: 0x$([Fg3]::GetForegroundWindow().ToString('X')) (notepad 0x$($win.MainWindowHandle.ToString('X')))"

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Probe
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)

# Pump the UI thread while the probe runs so Notepad's message queue is served.
$deadline = (Get-Date).AddSeconds(20)
while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 15
}
Write-Host "== probe output"
Write-Host $proc.StandardOutput.ReadToEnd()
$err = $proc.StandardError.ReadToEnd()
if ($err) { Write-Host $err }

$created = @(Get-Process -Name Notepad -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$new = $created | Where-Object { $before -notcontains $_ }
foreach ($id in $new) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
