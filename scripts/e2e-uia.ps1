#requires -Version 5.1
<#
.SYNOPSIS
    End-to-end UIA capture test for dict-popup (must run under Windows PowerShell / STA).

.DESCRIPTION
    Opens a WinForms window containing a word, selects it, then posts WM_HOTKEY
    straight to the app's hidden host window - no keystroke injection, so it does
    not disturb whatever else is on the desktop. The app must be running with a
    dictionary configured.

    powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File scripts\e2e-uia.ps1

    Exit code 0 = the card appeared (any content). The screenshot shows what it said.
#>
[CmdletBinding()]
param(
    [string]$Exe = 'F:\project\translation\dict-popup\target\release\dict-popup.exe',
    [string]$Word = 'apple',
    [string]$Screenshot = 'F:\project\translation\.uia-test.png'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class UiaHost {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public struct RECT { public int L, T, R, B; }
    public static IntPtr Host = IntPtr.Zero;
    public static IntPtr Card = IntPtr.Zero;
    public static void Scan(uint pid) {
        Host = IntPtr.Zero; Card = IntPtr.Zero;
        EnumWindows((h, p) => {
            uint wp; GetWindowThreadProcessId(h, out wp);
            if (wp == pid) {
                var cls = new StringBuilder(256); GetClassName(h, cls, 256);
                var txt = new StringBuilder(256); GetWindowText(h, txt, 256);
                if (cls.ToString() == "DictPopupInputHost") Host = h;
                if (cls.ToString() == "Window Class" && txt.ToString() == "dict-popup" && IsWindowVisible(h)) {
                    RECT r; GetWindowRect(h, out r);
                    if ((r.R - r.L) > 100 && (r.B - r.T) > 60) Card = h;
                }
            }
            return true;
        }, IntPtr.Zero);
    }
}
"@

function Start-Detached([string]$Path) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Path
    $psi.UseShellExecute = $false
    [System.Diagnostics.Process]::Start($psi)
}

Write-Host "== stopping any instance"
Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

Write-Host "== starting $Exe"
$app = Start-Detached $Exe
Start-Sleep -Seconds 3
if ($app.HasExited) { throw "app exited early ($($app.ExitCode))" }

[UiaHost]::Scan([uint32]$app.Id)
if ([UiaHost]::Host -eq [IntPtr]::Zero) { throw "host window not found" }
$hostHwnd = [UiaHost]::Host
Write-Host "== host hwnd $hostHwnd"

$primary = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$form = New-Object System.Windows.Forms.Form
$form.Text = 'uia capture target'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(($primary.X + 160), ($primary.Y + 160))
$form.Width = 520
$form.Height = 160
$form.TopMost = $true

$box = New-Object System.Windows.Forms.RichTextBox
$box.Dock = 'Fill'
$box.Font = New-Object System.Drawing.Font('Consolas', 24)
$box.Text = $Word
$form.Controls.Add($box)

$form.Show()
$form.Activate()
[void][UiaHost]::SetForegroundWindow($form.Handle)
$box.Focus()
$box.SelectAll()
[System.Windows.Forms.Application]::DoEvents()
Start-Sleep -Milliseconds 600

Write-Host "== posting WM_HOTKEY"
[void][UiaHost]::PostMessage($hostHwnd, 0x0312, [IntPtr]1, [IntPtr]0)

$deadline = (Get-Date).AddSeconds(3)
while ((Get-Date) -lt $deadline) {
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 50
}

[UiaHost]::Scan([uint32]$app.Id)
$visible = [UiaHost]::Card -ne [IntPtr]::Zero
Write-Host "== card visible: $visible"

$virtual = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap($virtual.Width, $virtual.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($virtual.X, $virtual.Y, 0, 0, $bitmap.Size)
$bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Host "== screenshot: $Screenshot"

$form.Close()
Start-Sleep -Milliseconds 300
Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Write-Host "== done"
if ($visible) { exit 0 } else { exit 1 }
