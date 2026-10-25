@echo off
rem =====================================================================
rem  诊断用：强制走剪贴板取词（跳过 UI Automation 回退）
rem
rem  用法：先退出正在运行的 dict-popup（托盘图标右键 -> 退出），
rem        再双击本文件。
rem =====================================================================
setlocal
set "DICT_POPUP_CAPTURE_MODE=clipboard"

set "EXE=%~dp0dist\dict-popup.exe"
if not exist "%EXE%" set "EXE=%~dp0dict-popup\target\release\dict-popup.exe"
if not exist "%EXE%" (
    echo 找不到 dict-popup.exe：请先运行 build.bat
    pause
    exit /b 1
)

echo 取词模式：仅剪贴板（DICT_POPUP_CAPTURE_MODE=clipboard）
echo 启动：%EXE%
start "dict-popup" "%EXE%"
exit /b 0
