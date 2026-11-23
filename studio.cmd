@echo off
rem Migrate Anything Studio launcher for Windows.
rem Double-click this file, or run:  studio.cmd        (opens Studio)
rem                                  studio.cmd demo   (opens Studio with the demo ready)
rem Keep the window open while you work: approval codes are shown here.
setlocal
cd /d "%~dp0"
set "EXE=target\release\migrate-anything.exe"

where cargo >nul 2>nul
if errorlevel 1 goto :no_cargo

echo Preparing Migrate Anything (the first start takes a few minutes)...
cargo build --release --locked -p migrate-anything
if errorlevel 1 goto :build_failed
goto :run

:no_cargo
if exist "%EXE%" goto :run
echo.
echo Rust is needed to build Migrate Anything the first time.
echo Install it from https://rustup.rs and start this file again.
echo.
pause
exit /b 1

:build_failed
echo.
echo The build failed. The messages above say why.
echo.
pause
exit /b 1

:run
"%EXE%" %*
if errorlevel 1 pause
