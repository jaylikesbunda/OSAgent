@echo off
setlocal

pushd "%~dp0"
cargo build --release
set "EXIT_CODE=%ERRORLEVEL%"
popd

exit /b %EXIT_CODE%
