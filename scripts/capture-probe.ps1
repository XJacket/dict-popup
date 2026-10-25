#requires -Version 5.1
<#
.SYNOPSIS
    Run dict-input's capture probe against a window we can force into the foreground.

.DESCRIPTION
    Automated agents cannot normally take the foreground, so this harness uses
    AttachThreadInput to give the test form focus, selects a word, then starts
    capture_probe.exe as a hidden child and prints what the pipeline decided.
#>
param(
    [string]$Probe = 'F:\project\translation\dict-input\target\release\examples\capture_probe.exe',
    [string]$Word = 'apple'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Fg2 {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
}
"@

$form = New-Object System.Windows.Forms.Form
$form.Text = 'capture probe target'
$form.StartPosition = 'CenterScreen'
$form.Width = 480
$form.Height = 160
$form.TopMost = $true

$box = New-Object System.Windows.Forms.RichTextBox
$box.Dock = 'Fill'
$box.Font = New-Object System.Drawing.Font('Consolas', 24)
$box.Text = $Word
$form.Controls.Add($box)
$form.Show()
$box.Focus()
$box.SelectAll()
[System.Windows.Forms.Application]::DoEvents()

# Force this window into the foreground (automation sessions cannot otherwise).
$fg = [Fg2]::GetForegroundWindow()
$pid2 = 0
$fgThread = [Fg2]::GetWindowThreadProcessId($fg, [ref]$pid2)
$myThread = [Fg2]::GetCurrentThreadId()
[void][Fg2]::AttachThreadInput($myThread, $fgThread, $true)
[void][Fg2]::SetForegroundWindow($form.Handle)
[void][Fg2]::AttachThreadInput($myThread, $fgThread, $false)
[System.Windows.Forms.Application]::DoEvents()
Start-Sleep -Milliseconds 200

# The foreground dance can move keyboard focus back to the form itself; put it
# into the text box and re-select, or Ctrl+C has nothing to copy.
$box.Focus()
$box.SelectAll()
[System.Windows.Forms.Application]::DoEvents()
Start-Sleep -Milliseconds 200

Write-Host "== form hwnd: $($form.Handle), foreground now: $([Fg2]::GetForegroundWindow())"

# Control experiment: does *any* synthesized Ctrl+C reach the text box? This
# separates "the system blocks synthetic input" from "our SendInput is wrong".
Set-Clipboard -Value 'PROBE-SENTINEL'
[System.Windows.Forms.Application]::DoEvents()
[System.Windows.Forms.SendKeys]::SendWait('^c')
Start-Sleep -Milliseconds 250
[System.Windows.Forms.Application]::DoEvents()
Write-Host "== control (SendKeys ^c) clipboard: '$([string](Get-Clipboard))'"

# Second control: the exact INPUT sequence the Rust code builds, injected from
# this (foreground-owning) process. Separates "our event records are wrong"
# from "a background process may not inject input".
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Inj {
    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)]
    public struct INPUTUNION { [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT { public uint type; public INPUTUNION u; }
    [DllImport("user32.dll", SetLastError=true)] public static extern uint SendInput(uint n, INPUT[] p, int cb);
    public static uint CtrlC() {
        INPUT[] seq = new INPUT[4];
        INPUT[] keys = new INPUT[4];
        for (int i = 0; i < 4; i++) { keys[i].type = 1; }
        keys[0].u.ki.wVk = 0x11;                       // Ctrl down
        keys[1].u.ki.wVk = 0x43;                       // C down
        keys[2].u.ki.wVk = 0x43; keys[2].u.ki.dwFlags = 2;  // C up
        keys[3].u.ki.wVk = 0x11; keys[3].u.ki.dwFlags = 2;  // Ctrl up
        uint total = 0;
        foreach (var k in keys) {
            INPUT[] one = new INPUT[1]; one[0] = k;
            total += SendInput(1, one, Marshal.SizeOf(typeof(INPUT)));
            System.Threading.Thread.Sleep(25);
        }
        return total;
    }
}
"@
Set-Clipboard -Value 'PROBE-SENTINEL-2'
[System.Windows.Forms.Application]::DoEvents()
$sent = [Inj]::CtrlC()
Start-Sleep -Milliseconds 300
[System.Windows.Forms.Application]::DoEvents()
Write-Host "== control (P/Invoke SendInput) sent=$sent clipboard: '$([string](Get-Clipboard))'"

# Restore the word into the box for the real probe run below.
$box.Text = $Word
$box.Focus()
$box.SelectAll()
[System.Windows.Forms.Application]::DoEvents()

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Probe
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$psi.EnvironmentVariables['DICT_PROBE_TYPE'] = 'ZZ'

# Clear the box so a typed character is unambiguous.
$box.Text = ''
$box.Focus()
[System.Windows.Forms.Application]::DoEvents()
$proc = [System.Diagnostics.Process]::Start($psi)

# Pump the WinForms message loop while the probe runs. Calling ReadToEnd()
# first would block this STA thread, so the text box would never process the
# WM_KEYDOWN/WM_COPY that SendInput generates - and the probe would always
# report "nothing copied".
$deadline = (Get-Date).AddSeconds(20)
while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 15
}

$stdout = $proc.StandardOutput.ReadToEnd()
$stderr = $proc.StandardError.ReadToEnd()
Write-Host "== text box now contains: '$($box.Text)'"

Write-Host "== probe exit: $($proc.ExitCode)"
Write-Host "== stdout"
Write-Host $stdout
if ($stderr) { Write-Host "== stderr"; Write-Host $stderr }

$form.Close()
