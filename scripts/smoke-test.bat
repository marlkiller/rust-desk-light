@echo off
setlocal enabledelayedexpansion

set "SCRIPTS_DIR=%~dp0"
pushd "%SCRIPTS_DIR%.."
set "ROOT_DIR=%CD%"
popd

if "%RDL_IP%"=="" (set "IP=127.0.0.1") else (set "IP=%RDL_IP%")
if "%RDL_PORT%"=="" (set "PORT=5169") else (set "PORT=%RDL_PORT%")
set "LOG_DIR=%ROOT_DIR%\target\rdl-smoke"

if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"
del /q "%LOG_DIR%\server.log" "%LOG_DIR%\server.err.log" "%LOG_DIR%\client.log" "%LOG_DIR%\client.err.log" "%LOG_DIR%\admin.log" 2>nul

pushd "%ROOT_DIR%"

echo [1/5] Building workspace
cargo build --workspace
if %ERRORLEVEL% neq 0 (
    echo Cargo build failed.
    popd
    exit /b %ERRORLEVEL%
)

set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server.exe"
set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client.exe"
set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin.exe"

:: Check if files exist, try without .exe if not found (though on Windows .exe is standard)
if not exist "%SERVER_EXE%" set "SERVER_EXE=%ROOT_DIR%\target\debug\rdl-server"
if not exist "%CLIENT_EXE%" set "CLIENT_EXE=%ROOT_DIR%\target\debug\rdl-client"
if not exist "%ADMIN_EXE%" set "ADMIN_EXE=%ROOT_DIR%\target\debug\rdl-admin"

echo [2/5] Starting server on %IP%:%PORT%
start /b "" "%SERVER_EXE%" --ip %IP% --port %PORT% > "%LOG_DIR%\server.log" 2> "%LOG_DIR%\server.err.log"

:: Wait for server log
set "WAIT_COUNT=0"
:wait_server
set /a WAIT_COUNT+=1
if %WAIT_COUNT% gtr 80 (
    echo Timed out waiting for server startup
    if exist "%LOG_DIR%\server.log" type "%LOG_DIR%\server.log"
    goto cleanup_fail
)
findstr /c:"server listening" "%LOG_DIR%\server.log" >nul 2>&1
if %ERRORLEVEL% neq 0 (
    timeout /t 1 /nobreak >nul
    goto wait_server
)

echo [3/5] Starting client
set "RDL_FORCE_TERMINAL=1"
start /b "" "%CLIENT_EXE%" --ip %IP% --port %PORT% > "%LOG_DIR%\client.log" 2> "%LOG_DIR%\client.err.log"

:: Wait for client log
set "WAIT_COUNT=0"
:wait_client
set /a WAIT_COUNT+=1
if %WAIT_COUNT% gtr 80 (
    echo Timed out waiting for client registration
    if exist "%LOG_DIR%\client.log" type "%LOG_DIR%\client.log"
    goto cleanup_fail
)
findstr /c:"client id:" "%LOG_DIR%\client.log" >nul 2>&1
if %ERRORLEVEL% neq 0 (
    timeout /t 1 /nobreak >nul
    goto wait_client
)

:: Get client id
for /f "tokens=3" %%a in ('findstr /c:"client id:" "%LOG_DIR%\client.log"') do (
    set "CLIENT_ID=%%a"
)

if "%CLIENT_ID%"=="" (
    echo Could not detect client id
    if exist "%LOG_DIR%\client.log" type "%LOG_DIR%\client.log"
    goto cleanup_fail
)

echo [4/5] Running admin command flow for client: %CLIENT_ID%
(
    echo list
    echo cmd %CLIENT_ID% computer_info
    echo quit
) | "%ADMIN_EXE%" --ip %IP% --port %PORT% > "%LOG_DIR%\admin.log" 2>&1

echo [5/5] Verifying output
findstr /c:"online clients: 1" "%LOG_DIR%\admin.log" >nul || (echo Admin did not list one online client && goto cleanup_fail)
findstr /c:"command=computer_info" "%LOG_DIR%\admin.log" >nul || (echo Admin did not receive command ack && goto cleanup_fail)
findstr /c:"hostname=" "%LOG_DIR%\admin.log" >nul || (echo Admin did not receive client computer info && goto cleanup_fail)

echo Smoke test passed.
echo Logs: %LOG_DIR%
goto cleanup_success

:cleanup_fail
call :stop_processes
popd
exit /b 1

:cleanup_success
call :stop_processes
popd
exit /b 0

:stop_processes
:: Force kill the processes we started. Using image name as taskkill /pid is hard in pure Batch.
taskkill /f /im rdl-client.exe /t >nul 2>&1
taskkill /f /im rdl-server.exe /t >nul 2>&1
exit /b
