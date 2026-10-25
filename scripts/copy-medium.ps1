$src = 'F:\project\translation\dist\dict-popup.exe'
$dst = 'F:\project\translation\dist\dict-popup-m.exe'
Copy-Item -LiteralPath $src -Destination $dst -Force
Write-Output ("copied; whoami says: " + (cmd /c "whoami /groups | findstr Mandatory"))
