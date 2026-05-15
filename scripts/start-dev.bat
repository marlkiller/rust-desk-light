@echo off
setlocal enabledelayedexpansion

set "SCRIPTS_DIR=%~dp0"
pushd "%SCRIPTS_DIR%.."
set "ROOT_DIR=%CD%"
popd

if "%RDL_IP%"=="" (set "IP=127.0.0.1") else (set "IP=%RDL_IP%")
if "%RDL_PORT%"=="" (set "PORT=5169") else (set "PORT=%RDL_PORT%")
set "LOG_DIR=%ROOT_DIR%\target\rdl-dev"

echo Starting rust-desk-light dev stack
echo server: %IP%:%PORT%
echo logs: %LOG_DIR%

if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"

pushd "%ROOT_DIR%"
cargo build --workspace
if %ERRORLEVEL% neq 0 (
    echo Cargo build failed.
    popd
    exit /b %ERRORLEVEL%
)

set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server.exe"
set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client.exe"
set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin.exe"

if not exist "%SERVER_EXE%" set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server"
if not exist "%CLIENT_EXE%" set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client"
if not exist "%ADMIN_EXE%" set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin"

:: Start server in a new window, keeps open on exit
start "rdl-server" cmd /k "cd /d "%ROOT_DIR%" && "%SERVER_EXE%" --ip %IP% --port %PORT%"

timeout /t 1 /nobreak >nul

:: Start client and admin with redirection
start /b "" "%CLIENT_EXE%" --ip %IP% --port %PORT% > "%LOG_DIR%\client.log" 2> "%LOG_DIR%\client.err.log"
timeout /t 1 /nobreak >nul
start /b "" "%ADMIN_EXE%" --ip %IP% --port %PORT% > "%LOG_DIR%\admin.log" 2> "%LOG_DIR%\admin.err.log"

echo Started server terminal, client GUI, and admin GUI.
popd
