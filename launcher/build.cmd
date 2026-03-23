@echo off
setlocal

pushd "%~dp0"
echo [1/3] Checking formatting...
cargo fmt --all -- --check
if errorlevel 1 goto :fail

echo [2/3] Running clippy...
cargo clippy --all-targets -- -D warnings
if errorlevel 1 goto :fail

echo [3/3] Building launcher release binary...
cargo build --release
if errorlevel 1 goto :fail

set "EXIT_CODE=0"
goto :end

:fail
set "EXIT_CODE=%ERRORLEVEL%"

:end
popd

exit /b %EXIT_CODE%
