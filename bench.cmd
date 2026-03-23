@echo off
REM OSAgent Performance Benchmark Suite for Windows
REM Run with: bench.cmd

setlocal enabledelayedexpansion

set RESULTS_DIR=benchmark_results
set TIMESTAMP=%date:~-4%%date:~3,2%%date:~0,2%_%time:~0,2%%time:~3,2%%time:~6,2%
set TIMESTAMP=%TIMESTAMP: =0%
set REPORT_FILE=%RESULTS_DIR%\report_%TIMESTAMP%.md

if not exist "%RESULTS_DIR%" mkdir "%RESULTS_DIR%"

echo.
echo ============================================================
echo          OSAgent Performance Benchmark Suite
echo ============================================================
echo.

REM Build release binary
echo [1/5] Building release binary...
cargo build --release 2>nul
if exist "target\release\osagent.exe" (
    for %%A in ("target\release\osagent.exe") do set BINARY_SIZE=%%~zA
    set /a BINARY_MB=!BINARY_SIZE! / 1048576
    echo    Binary size: !BINARY_MB!MB
) else (
    echo    ERROR: Build failed
    exit /b 1
)

REM Startup time measurement
echo [2/5] Measuring startup time...
set SUM_MS=0
for /L %%i in (1,1,10) do (
    powershell -Command "$s = Get-Date; ./target/release/osagent.exe --version | Out-Null; $e = Get-Date; Write-Output (($e - $s).TotalMilliseconds)" > temp_time.txt
    set /p MS=<temp_time.txt
    set /a SUM_MS+=!MS!
)
set /a AVG_STARTUP=SUM_MS / 10
echo    Average startup: !AVG_STARTUP!ms
del temp_time.txt 2>nul

REM Memory measurement
echo [3/5] Measuring memory...
start /B target\release\osagent.exe --version >nul 2>&1
timeout /t 1 >nul
for /f "tokens=2" %%a in ('tasklist /FI "IMAGENAME eq osagent.exe" ^| find "osagent.exe"') do set MEM_KB=%%a
set MEM_KB=%MEM_KB:,=%
set /a MEM_MB=%MEM_KB% / 1024
echo    Idle memory: !MEM_MB!MB
taskkill /F /IM osagent.exe >nul 2>&1

REM Run criterion benchmarks
echo [4/5] Running micro-benchmarks...
cargo bench --bench performance -- --save-baseline osagent_%TIMESTAMP% 2>&1 | tee "%RESULTS_DIR%\criterion_%TIMESTAMP%.txt"

REM Check for OpenCode
echo [5/5] Checking competitors...
where opencode >nul 2>&1
if %ERRORLEVEL% EQU 0 (
    echo    Found OpenCode, comparing...
    set OPCODE_SUM=0
    for /L %%i in (1,1,5) do (
        powershell -Command "$s = Get-Date; opencode --version | Out-Null; $e = Get-Date; Write-Output (($e - $s).TotalMilliseconds)" > temp_op.txt
        set /p OPCODE_MS=<temp_op.txt
        set /a OPCODE_SUM+=!OPCODE_MS!
    )
    set /a OPCODE_AVG=OPCODE_SUM / 5
    echo    OpenCode startup: !OPCODE_AVG!ms
    del temp_op.txt 2>nul
) else (
    echo    OpenCode not found
    set OPCODE_AVG=N/A
)

REM Generate report
echo Generating report...

(
echo # OSAgent Performance Report
echo.
echo **Generated:** %date% %time%
echo **Platform:** Windows %PROCESSOR_ARCHITECTURE%
echo.
echo ---
echo.
echo ## Summary
echo.
echo ^| Metric ^| OSAgent ^| OpenCode ^|
echo ^|--------^|---------^|----------^|
echo ^| Startup Time ^| !AVG_STARTUP!ms ^| !OPCODE_AVG!ms ^|
echo ^| Idle Memory ^| !MEM_MB!MB ^| N/A ^|
echo ^| Binary Size ^| !BINARY_MB!MB ^| N/A ^|
echo.
echo ---
echo.
echo ## Performance Targets
echo.
echo ^| Metric ^| Target ^| Status ^|
echo ^|--------^|--------^|--------^|
echo ^| Startup ^< 50ms ^| !AVG_STARTUP!ms ^| !AVG_STARTUP! LSS 50 (PASS^) ELSE (FAIL^) ^|
echo ^| Memory ^< 30MB ^| !MEM_MB!MB ^| !MEM_MB! LSS 30 (PASS^) ELSE (FAIL^) ^|
) > "%REPORT_FILE%"

echo.
echo ============================================================
echo                    BENCHMARK COMPLETE
echo ============================================================
echo.
echo Report saved to: %REPORT_FILE%
echo.
echo Quick Summary:
echo   Startup:   !AVG_STARTUP!ms
echo   Memory:    !MEM_MB!MB
echo   Binary:    !BINARY_MB!MB
echo.

if "!OPCODE_AVG!" NEQ "N/A" (
    set /a SPEEDUP=!OPCODE_AVG! / !AVG_STARTUP!
    echo vs OpenCode:
    echo   Startup:  !SPEEDUP!x faster
)

echo.
echo View full report: type "%REPORT_FILE%"
