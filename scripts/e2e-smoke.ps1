#requires -Version 5.1
<#
.SYNOPSIS
    End-to-end smoke test for dict-popup.

.DESCRIPTION
    Opens a WinForms window holding the word "apple", selects it, fires the global
    hotkey, waits for the card, and captures the whole virtual desktop.

    Must run under Windows PowerShell (STA). Under pwsh (MTA) WinForms windows
    never appear and the test is meaningless:
        powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\e2e-smoke.ps1

    The app must already have a config that points at a real StarDict .ifo.
    The script starts its own instance and stops only the processes it started.
#>
[CmdletBinding()]
param(
    [string]$Exe = 'F:\project\translation\dict-popup\target\release\dict-popup.exe',
    [string]$Screenshot = 'F:\project\translation\.smoke-card.png',
    [string]$Word = 'apple',
    [string]$Hotkey = '^%d'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

function Start-Detached([string]$Path) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Path
    $psi.UseShellExecute = $false
    [System.Diagnostics.Process]::Start($psi)
}

Write-Host "== stopping any running instance"
Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

Write-Host "== starting $Exe"
$app = Start-Detached $Exe
Start-Sleep -Seconds 3
if ($app.HasExited) { throw "app exited early with code $($app.ExitCode)" }

$primary = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
[System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point(($primary.X + 430), ($primary.Y + 360))

$form = New-Object System.Windows.Forms.Form
$form.Text = 'dict-popup e2e host'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(($primary.X + 140), ($primary.Y + 180))
$form.Width = 560
$form.Height = 180
$form.TopMost = $true

$box = New-Object System.Windows.Forms.RichTextBox
$box.Dock = 'Fill'
$box.Font = New-Object System.Drawing.Font('Consolas', 26)
$box.Text = $Word
$form.Controls.Add($box)

$form.Show()
$form.Activate()
$box.Focus()
$box.SelectAll()
[System.Windows.Forms.Application]::DoEvents()
Start-Sleep -Milliseconds 600

Write-Host "== sending hotkey $Hotkey"
[System.Windows.Forms.SendKeys]::SendWait($Hotkey)

$deadline = (Get-Date).AddSeconds(3)
while ((Get-Date) -lt $deadline) {
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 40
}

$virtual = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap($virtual.Width, $virtual.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($virtual.X, $virtual.Y, 0, 0, $bitmap.Size)
$bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

$proc = Get-Process -Id $app.Id -ErrorAction SilentlyContinue
if ($proc) {
    $proc.Refresh()
    Write-Host ("== memory: WS={0:N1} MB  private={1:N1} MB  threads={2}" -f ($proc.WorkingSet64 / 1MB), ($proc.PrivateMemorySize64 / 1MB), $proc.Threads.Count)
}

Write-Host "== windows:"
Get-Process | Where-Object { $_.MainWindowTitle } | Select-Object ProcessName, MainWindowTitle | Format-Table -AutoSize

$form.Close()
Write-Host "== screenshot: $Screenshot"
Write-Host "== app left running (pid $($app.Id)) for inspection"
