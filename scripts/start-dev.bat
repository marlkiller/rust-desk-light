@echo off
setlocal enabledelayedexpansion

set "SCRIPTS_DIR=%~dp0"
pushd "%SCRIPTS_DIR%.."
set "ROOT_DIR=%CD%"
popd

set "DEFAULT_IP=127.0.0.1"
set "DEFAULT_PORT=5169"
set "DEFAULT_AUTH_TOKEN=123456"

if "%RDL_IP%"=="" (set "IP=%DEFAULT_IP%") else (set "IP=%RDL_IP%")
if "%RDL_PORT%"=="" (set "PORT=%DEFAULT_PORT%") else (set "PORT=%RDL_PORT%")
if "%RDL_AUTH_TOKEN%"=="" (
    set "AUTH_TOKEN=%DEFAULT_AUTH_TOKEN%"
) else (
    set "AUTH_TOKEN=%RDL_AUTH_TOKEN%"
)
set "LOG_DIR=%ROOT_DIR%\target\rdl-dev"

echo Starting rust-desk-light dev stack
echo server: %IP%:%PORT%
echo auth token: %AUTH_TOKEN%
echo logs: %LOG_DIR%

if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"

pushd "%ROOT_DIR%"
cargo build --workspace
if %ERRORLEVEL% neq 0 (
    echo Cargo build failed.
    popd
    exit /b %ERRORLEVEL%
)

set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server-cli.exe"
set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client-gui.exe"
set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin-gui.exe"

if not exist "%SERVER_EXE%" set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server-cli"
if not exist "%CLIENT_EXE%" set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client-gui"
if not exist "%ADMIN_EXE%" set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin-gui"

:: Start server in a new window, keeps open on exit
start "rdl-server-cli" /D "%ROOT_DIR%" cmd /k ""%SERVER_EXE%" --ip "%IP%" --port "%PORT%" --auth-token "%AUTH_TOKEN%""

timeout /t 1 /nobreak >nul

:: Start client and admin with redirection
start /b "" "%CLIENT_EXE%" --ip "%IP%" --port "%PORT%" --auth-token "%AUTH_TOKEN%" > "%LOG_DIR%\client.log" 2> "%LOG_DIR%\client.err.log"
timeout /t 1 /nobreak >nul
start /b "" "%ADMIN_EXE%" --ip "%IP%" --port "%PORT%" --auth-token "%AUTH_TOKEN%" > "%LOG_DIR%\admin.log" 2> "%LOG_DIR%\admin.err.log"

echo Started server terminal, client GUI, and admin GUI.
popd
