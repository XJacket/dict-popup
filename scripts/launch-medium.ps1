# Launch dict-popup from a normal (medium-integrity) shell.
# Used to verify that the exe runs at Medium integrity, which is required for
# sending Ctrl+C to other applications (UIPI blocks Low -> Medium input).
param(
    [string]$Exe = 'F:\project\translation\dist\dict-popup.exe'
)

$process = [System.Diagnostics.Process]::Start($Exe)
Write-Output $process.Id
