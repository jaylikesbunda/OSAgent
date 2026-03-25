@echo off
setlocal

pushd "%~dp0"

set PROFILE=%1
if "%PROFILE%"=="" set PROFILE=release

echo Building OSAgent (%PROFILE%)

echo === Core ===
echo [1/8] Checking core formatting...
cargo fmt -- --check
if errorlevel 1 goto :fail

echo [2/8] Running core clippy...
cargo clippy --all-targets --all-features -- -D warnings
if errorlevel 1 goto :fail

echo [3/8] Running core tests...
cargo test --all-features --verbose
if errorlevel 1 goto :fail

echo [4/8] Building core with Discord...
cargo build --%PROFILE% --features discord
if errorlevel 1 goto :fail

echo === Launcher ===
echo [5/8] Checking launcher formatting...
cargo fmt --manifest-path launcher/Cargo.toml --all -- --check
if errorlevel 1 goto :fail

echo [6/8] Running launcher clippy...
cargo clippy --manifest-path launcher/Cargo.toml --all-targets --all-features -- -D warnings
if errorlevel 1 goto :fail

echo [7/8] Building launcher with embedded core...
cargo build --manifest-path launcher/Cargo.toml --%PROFILE%
if errorlevel 1 goto :fail

echo [8/8] Done!
echo Launcher: launcher\target\%PROFILE%\osagent-launcher.exe

set "EXIT_CODE=0"
goto :end

:fail
set "EXIT_CODE=%ERRORLEVEL%"

:end
popd

exit /b %EXIT_CODE%
