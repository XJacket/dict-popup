@echo off
rem ===========================================================================
rem  dict-popup - one-click release build for Windows (GNU toolchain)
rem
rem  Produces: dist\dict-popup.exe  with the application icon and a
rem  VERSIONINFO resource embedded in the executable.
rem
rem  Requirements (no MSVC, no .NET SDK, no Windows SDK):
rem    * cargo / rustc, host x86_64-pc-windows-gnu
rem    * MinGW-w64 binutils: windres + gcc  (bundled in tools\mingw64\bin)
rem
rem  Usage:  build.bat
rem  Env:    CARGO_TARGET_DIR   overrides cargo's target directory.
rem                            (the copy below follows the same value)
rem
rem  Notes:
rem    * Resources are injected with `cargo rustc ... -- -C link-arg=<res>`
rem      so the dependency graph is NOT rebuilt (plain `cargo build` cannot
rem      carry extra link args and would produce an exe without resources).
rem    * Keep the checkout path free of spaces: the link arg is passed
rem      verbatim to gcc.
rem ===========================================================================
setlocal EnableExtensions

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

rem --- toolchain --------------------------------------------------------------
rem cargo/rustc are normally found through the rustup proxy in %USERPROFILE%\.cargo\bin.
rem No MSVC / Windows SDK is needed: linking uses the MinGW toolchain below or on PATH.
set "CARGO_BIN=%USERPROFILE%\.cargo\bin"
set "MINGW_BIN=%ROOT%\tools\mingw64\bin"

set "PATH=%PATH%;%CARGO_BIN%"
if exist "%MINGW_BIN%\windres.exe" set "PATH=%PATH%;%MINGW_BIN%"

where cargo    >nul 2>nul || goto :no_cargo
where windres  >nul 2>nul || goto :no_windres
where gcc      >nul 2>nul || goto :no_gcc

pushd "%ROOT%"

if not exist "assets\dict-popup.ico" (
    echo [build] assets\dict-popup.ico is missing - generating it...
    powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\make-icon.ps1"
    if errorlevel 1 goto :fail
)

echo [build] compiling assets\version.rc with windres...
windres -I "%ROOT%\assets" --codepage=65001 -O coff -o "%ROOT%\assets\dict-popup.res" "%ROOT%\assets\version.rc"
if errorlevel 1 goto :fail

rem resource path handed to the linker; forward slashes, absolute
set "RES=%ROOT:\=/%/assets/dict-popup.res"

if defined CARGO_TARGET_DIR (
    set "TARGET_DIR=%CARGO_TARGET_DIR%"
) else (
    set "TARGET_DIR=%ROOT%\dict-popup\target"
)

echo [build] cargo rustc --release (linking %RES%)
cargo rustc --release --manifest-path "%ROOT%\dict-popup\Cargo.toml" --bin dict-popup -- -C link-arg=%RES%
if errorlevel 1 goto :fail

if not exist "%TARGET_DIR%\release\dict-popup.exe" (
    echo [build] ERROR: expected "%TARGET_DIR%\release\dict-popup.exe" was not produced.
    goto :fail
)

if not exist "dist" mkdir "dist"
copy /Y "%TARGET_DIR%\release\dict-popup.exe" "dist\dict-popup.exe" >nul
if errorlevel 1 goto :fail

rem --- gate: fail loudly if the resource did not make it into the binary -----
powershell -NoProfile -Command "$v = (Get-Item '%ROOT%\dist\dict-popup.exe').VersionInfo; if (-not $v.FileVersion) { Write-Host '[build] ERROR: no VERSIONINFO in dist\dict-popup.exe'; exit 1 } else { Write-Host ('[build] version {0} / {1}' -f $v.FileVersion, $v.FileDescription) }"
if errorlevel 1 goto :fail

for %%F in ("dist\dict-popup.exe") do echo [build] OK  dist\dict-popup.exe  %%~zF bytes

popd
endlocal
exit /b 0

:no_cargo
echo [build] ERROR: cargo not found. Install Rust or fix CARGO_BIN in build.bat.
exit /b 1
:no_windres
echo [build] ERROR: windres not found. Expected "%MINGW_BIN%\windres.exe".
exit /b 1
:no_gcc
echo [build] ERROR: gcc not found. Expected "%MINGW_BIN%\gcc.exe".
exit /b 1
:fail
echo.
echo [build] FAILED.
popd
endlocal
exit /b 1
