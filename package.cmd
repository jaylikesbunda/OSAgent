@echo off
setlocal enabledelayedexpansion

echo === OSAgent Package Builder ===
echo.

set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%"

:: Parse arguments
set "PROFILE=release"
set "SKIP_BUILD=0"
:parse_args
if "%~1"=="" goto :done_args
if /i "%~1"=="--debug" set "PROFILE=debug"
if /i "%~1"=="--skip-build" set "SKIP_BUILD=1"
shift
goto :parse_args
:done_args

echo Profile: %PROFILE%
echo.

:: Build osagent core if not skipped
if "%SKIP_BUILD%"=="0" (
    echo [1/3] Building osagent core (%PROFILE%)...
    cargo build --profile %PROFILE%
    if errorlevel 1 (
        echo Failed to build osagent core
        exit /b 1
    )
)

:: Build launcher if not skipped
if "%SKIP_BUILD%"=="0" (
    echo [2/3] Building launcher (%PROFILE%)...
    cd launcher
    cargo tauri build
    if errorlevel 1 (
        echo Failed to build launcher
        exit /b 1
    )
    cd ..
)

:: Create package directory
echo [3/3] Creating package...
set "DIST_DIR=dist\osagent-windows-x86_64"
if exist "%DIST_DIR%" rmdir /s /q "%DIST_DIR%"
mkdir "%DIST_DIR%"

:: Copy core binary
set "CORE_EXT=.exe"
copy "target\%PROFILE%\osagent%CORE_EXT%" "%DIST_DIR%\osagent%CORE_EXT%" >nul

:: Copy launcher (from tauri build output)
set "LAUNCHER_SRC=launcher\src-tauri\target\release\osagent-launcher.exe"
if exist "%LAUNCHER_SRC%" (
    copy "%LAUNCHER_SRC%" "%DIST_DIR%\osagent-launcher.exe" >nul
) else (
    :: Fallback to debug if release not found
    set "LAUNCHER_SRC=launcher\src-tauri\target\debug\osagent-launcher.exe"
    if exist "%LAUNCHER_SRC%" (
        copy "%LAUNCHER_SRC%" "%DIST_DIR%\osagent-launcher.exe" >nul
    )
)

:: Copy any required DLLs from target
if exist "target\%PROFILE%\*.dll" (
    copy "target\%PROFILE%\*.dll" "%DIST_DIR%\" >nul 2>&1
)

:: Create README for the package
echo OSAgent - Your Open Source Agent > "%DIST_DIR%\README.txt"
echo. >> "%DIST_DIR%\README.txt"
echo To start: >> "%DIST_DIR%\README.txt"
echo   1. Run osagent-launcher.exe >> "%DIST_DIR%\README.txt"
echo   2. Follow the setup wizard >> "%DIST_DIR%\README.txt"
echo   3. OSA will start at http://localhost:8765 >> "%DIST_DIR%\README.txt"

echo.
echo ✓ Package created at: %DIST_DIR%
echo.
echo Contents:
dir /b "%DIST_DIR%"
echo.
echo Done!
