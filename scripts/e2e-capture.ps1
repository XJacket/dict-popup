#requires -Version 5.1
<#
.SYNOPSIS
    Deterministic UIA capture test for dict-popup (Windows PowerShell / STA).

.DESCRIPTION
    Creates its own Notepad window with a known word, selects that word through
    UI Automation, then starts dict-popup with DICT_POPUP_CAPTURE_HWND pointing
    at that window and posts WM_HOTKEY. Nothing depends on being able to steal
    foreground focus, which an automated session cannot do.

    powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File scripts\e2e-capture.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = 'F:\project\translation\dict-popup\target\release\dict-popup.exe',
    [string]$Word = 'apple',
    [string]$Screenshot = 'F:\project\translation\.capture-test.png'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Cap {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public struct RECT { public int L, T, R, B; }
    public static IntPtr Host, Card;
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

function Start-Detached([string]$File, [string]$Arguments) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $File
    $psi.Arguments = $Arguments
    $psi.UseShellExecute = $false
    [System.Diagnostics.Process]::Start($psi)
}

$tmp = Join-Path $env:TEMP 'dict-popup-capture-test.txt'
Set-Content -LiteralPath $tmp -Value $Word -Encoding ASCII

Write-Host "== opening notepad with '$Word'"
$before = @(Get-Process -Name Notepad -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$np = Start-Detached 'C:\Windows\System32\notepad.exe' "`"$tmp`""
Start-Sleep -Seconds 2

# The packaged Notepad may hand the file to an existing process; find the window.
$npWin = Get-Process -Name Notepad -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -like '*capture-test*' } |
    Select-Object -First 1
if (-not $npWin) { throw "notepad window not found" }
Write-Host "== notepad hwnd=$($npWin.MainWindowHandle)"

Write-Host "== selecting the text through UI Automation"
$doc = [System.Windows.Automation.AutomationElement]::FromHandle($npWin.MainWindowHandle)
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
$textEl = Find-TextElement $doc 0
if (-not $textEl) { throw "no text element in notepad" }
$tp = $textEl.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
$tp.DocumentRange.Select()
Write-Host "== selected text: '$($tp.GetSelection()[0].GetText(40))'"

Write-Host "== starting app with capture target override"
Get-Process -Name 'dict-popup' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400
$env:DICT_POPUP_CAPTURE_HWND = "$($npWin.MainWindowHandle.ToInt64())"
$app = Start-Detached $Exe ''
Start-Sleep -Seconds 3

[Cap]::Scan([uint32]$app.Id)
if ([Cap]::Host -eq [IntPtr]::Zero) { throw "host window not found" }
Write-Host "== posting WM_HOTKEY"
[void][Cap]::PostMessage([Cap]::Host, 0x0312, [IntPtr]1, [IntPtr]0)
Start-Sleep -Seconds 2

[Cap]::Scan([uint32]$app.Id)
Write-Host "== card visible: $([Cap]::Card -ne [IntPtr]::Zero)"

$virtual = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap($virtual.Width, $virtual.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($virtual.X, $virtual.Y, 0, 0, $bitmap.Size)
$bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose(); $bitmap.Dispose()
Write-Host "== screenshot: $Screenshot"

$env:DICT_POPUP_CAPTURE_HWND = $null
Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Id $np.Id -Force -ErrorAction SilentlyContinue
# close the tab if it was handed to an existing notepad process
$created = @(Get-Process -Name Notepad -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$new = $created | Where-Object { $before -notcontains $_ }
foreach ($id in $new) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
Write-Host "== done"
