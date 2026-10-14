@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"

echo.
echo ========================================
echo   局域网文件分享 一键打包 (Inno Setup)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0packaging\build.ps1" -OpenDist
if errorlevel 1 (
    echo.
    echo 打包失败。请查看上方错误信息。
    pause
    exit /b 1
)

echo.
pause
