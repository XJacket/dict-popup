@echo off
rem =====================================================================
rem  dict-popup - one-time dependency fetch
rem
rem  WHY: the DSH agent session cannot reach crates.io (its token is
rem  blocked from TLS), but YOUR normal PowerShell can. Run this once;
rem  afterwards the agent builds fully offline.
rem
rem  It only writes inside F:\project\translation\.cargo-home and
rem  F:\project\translation\dict-popup\target - nothing global.
rem =====================================================================
setlocal
echo.
echo === dict-popup dependency fetch ===
echo.

set "CARGO_HOME=F:\project\translation\.cargo-home"
set "PATH=C:\Users\xujia\.cargo\bin;F:\project\translation\tools\mingw64\bin;%PATH%"

cd /d "F:\project\translation\dict-popup"
if errorlevel 1 goto fail

echo CARGO_HOME=%CARGO_HOME%
echo.
echo [1/2] downloading crates...
cargo fetch
if errorlevel 1 goto fail

echo.
echo [2/2] test build (release, takes a few minutes the first time)...
cargo build --release
if errorlevel 1 goto fail

echo.
echo =====================================================================
echo  DONE - dependencies are local, the agent can build offline now.
echo  Tell the agent "bat 跑完了" and close this window.
echo =====================================================================
pause
exit /b 0

:fail
echo.
echo =====================================================================
echo  FAILED - nothing else was changed.
echo  Copy the error text above (or this window) back to the agent.
echo =====================================================================
pause
exit /b 1
