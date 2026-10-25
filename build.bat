@echo off
rem ===========================================================================
rem  dict-popup - one-click release build for Windows (GNU toolchain)
rem
rem  Produces: dist\dict-popup.exe  with the application icon, a VERSIONINFO
rem  resource and an application manifest embedded in the executable.
rem
rem  Requirements (no MSVC, no .NET SDK, no Windows SDK):
rem    * cargo / rustc, host x86_64-pc-windows-gnu
rem    * MinGW-w64 binutils: windres + gcc  (bundled in tools\mingw64\bin)
rem
rem  Usage:  build.bat              default: manifest uiAccess="false"
rem          build.bat /uiaccess    manifest uiAccess="true" (signed builds only)
rem  Env:    CARGO_TARGET_DIR     overrides cargo's target directory.
rem                              (the copy below follows the same value)
rem          DICT_POPUP_UIACCESS=1  same as /uiaccess.
rem
rem  UIAccess is opt-in because Windows refuses to start an *unsigned* binary
rem  that requests it. The /uiaccess build is still produced unsigned here (the
rem  signing pipeline builds and then signs), so build.bat prints a loud warning
rem  when the result has no valid Authenticode signature.
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

rem --- UIAccess switch (opt-in; default off) ----------------------------------
rem An unsigned exe whose manifest says uiAccess="true" does not start, so the
rem default build must keep uiAccess="false". Both spellings are accepted:
rem   build.bat /uiaccess
rem   set DICT_POPUP_UIACCESS=1 && build.bat
set "UIACCESS="
if /I "%~1"=="/uiaccess" set "UIACCESS=1"
if /I "%DICT_POPUP_UIACCESS%"=="1" set "UIACCESS=1"
if not "%~1"=="" if /I not "%~1"=="/uiaccess" goto :bad_usage
if not "%~2"=="" goto :bad_usage

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

rem --- manifest selection: assets\version.rc picks the .manifest via this -D ----
if defined UIACCESS (
    echo [build] UIAccess manifest: ON - uiAccess="true", unsigned build will NOT start
    set "RC_DEFINE=-D DICT_POPUP_UIACCESS"
) else (
    echo [build] UIAccess manifest: off - uiAccess="false"
    set "RC_DEFINE="
)

rem --- keep GCC's implicit default manifest out of the link ------------------
rem GCC's link spec adds <toolchain>/default-manifest.o, which carries its own
rem RT_MANIFEST id 1 with the same language as ours. ld then reports
rem "multiple non-default manifests" and leaves two id-1 resources in the exe.
rem An empty default-manifest.o in a private -B directory shadows the toolchain
rem one for this link only, so the manifest embedded below is the only one.
set "STUB_DIR=%ROOT%\assets\build"
if not exist "%STUB_DIR%" mkdir "%STUB_DIR%"
gcc -x c -c -o "%STUB_DIR%\default-manifest.o" - < nul
if errorlevel 1 goto :fail
set "STUB_ARG=%STUB_DIR:\=/%"

echo [build] compiling assets\version.rc with windres...
windres %RC_DEFINE% -I "%ROOT%\assets" --codepage=65001 -O coff -o "%ROOT%\assets\dict-popup.res" "%ROOT%\assets\version.rc"
if errorlevel 1 goto :fail

rem resource path handed to the linker; forward slashes, absolute
set "RES=%ROOT:\=/%/assets/dict-popup.res"

if defined CARGO_TARGET_DIR (
    set "TARGET_DIR=%CARGO_TARGET_DIR%"
) else (
    set "TARGET_DIR=%ROOT%\dict-popup\target"
)

rem Extra link args are NOT part of cargo's fingerprint: without this, flipping
rem /uiaccess would reuse the previous exe and silently keep the old manifest.
echo [build] cargo clean -p dict-popup (forces a relink with this manifest)
cargo clean --release --manifest-path "%ROOT%\dict-popup\Cargo.toml" -p dict-popup
if errorlevel 1 goto :fail

echo [build] cargo rustc --release (linking %RES%, -B %STUB_ARG%)
cargo rustc --release --manifest-path "%ROOT%\dict-popup\Cargo.toml" --bin dict-popup -- -C link-arg=%RES% -C link-arg=-B%STUB_ARG%
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

rem --- gate: the requested manifest variant must be in the exe bytes ---------
rem Checked as raw bytes (ISO-8859-1 round-trip) because the manifest is an
rem embedded resource, not something VersionInfo exposes.
if defined UIACCESS (set "MANIFEST_EXPECT=true") else (set "MANIFEST_EXPECT=false")
powershell -NoProfile -Command "$q=[char]34; $needle='uiAccess=' + $q + '%MANIFEST_EXPECT%' + $q; $text=[IO.File]::ReadAllText('%ROOT%\dist\dict-popup.exe', [Text.Encoding]::GetEncoding(28591)); if (-not $text.Contains($needle)) { Write-Host ('[build] ERROR: expected manifest token {0} missing from dist\dict-popup.exe' -f $needle); exit 1 } else { Write-Host ('[build] manifest {0} embedded' -f $needle) }"
if errorlevel 1 goto :fail

rem --- gate: warn loudly when a UIAccess build is still unsigned --------------
rem Expected in the build-then-sign pipeline, fatal for anyone who runs it as-is.
if defined UIACCESS (
    powershell -NoProfile -Command "if ((Get-AuthenticodeSignature '%ROOT%\dist\dict-popup.exe').Status -ne 'Valid') { exit 1 }"
    if errorlevel 1 (
        echo.
        echo ###########################################################################
        echo # [build] WARNING: UIAccess build is NOT Authenticode-signed.
        echo # [build] Windows will refuse to start dist\dict-popup.exe as-is:
        echo # [build]   * a manifest with uiAccess="true" needs a valid signature
        echo # [build]     from a certificate trusted on the target machine, and
        echo # [build]   * the exe must be installed under Program Files.
        echo # [build] Sign the exe before launching, packaging or shipping it.
        echo ###########################################################################
        echo.
    ) else (
        echo [build] UIAccess exe has a valid Authenticode signature.
    )
)

for %%F in ("dist\dict-popup.exe") do echo [build] OK  dist\dict-popup.exe  %%~zF bytes

popd
endlocal
exit /b 0

:no_cargo
echo [build] ERROR: cargo not found. Install Rust or fix CARGO_BIN in build.bat.
exit /b 1
:bad_usage
echo [build] ERROR: unrecognised argument "%~1".
echo [build] usage: build.bat [/uiaccess]   or set DICT_POPUP_UIACCESS=1
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
