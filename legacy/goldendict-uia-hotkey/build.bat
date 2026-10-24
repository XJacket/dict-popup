@echo off
setlocal
set "PATH=F:\project\translation\tools\mingw64\bin;C:\Users\xujia\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin;%PATH%"
cd /d "%~dp0"
cargo build --release
if errorlevel 1 (
  echo Build failed.
  exit /b 1
)
echo Build OK: %~dp0target\release\goldendict-uia-hotkey.exe
