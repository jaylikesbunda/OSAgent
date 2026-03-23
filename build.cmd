@echo off
setlocal

pushd "%~dp0"
echo [1/4] Checking formatting...
cargo fmt -- --check
if errorlevel 1 goto :fail

echo [2/4] Running clippy...
cargo clippy --all-targets --all-features -- -D warnings
if errorlevel 1 goto :fail

echo [3/4] Running tests...
cargo test --all-features --verbose
if errorlevel 1 goto :fail

echo [4/4] Building release binary...
cargo build --release
if errorlevel 1 goto :fail

set "EXIT_CODE=0"
goto :end

:fail
set "EXIT_CODE=%ERRORLEVEL%"

:end
popd

exit /b %EXIT_CODE%
