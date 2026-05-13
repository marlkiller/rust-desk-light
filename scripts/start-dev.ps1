$ErrorActionPreference = "Stop"

$RootDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$Ip = if ($env:RDL_IP) { $env:RDL_IP } else { "127.0.0.1" }
$Port = if ($env:RDL_PORT) { $env:RDL_PORT } else { "21115" }
$LogDir = Join-Path $RootDir "target\rdl-dev"

function Start-RdlWindow {
    param(
        [string]$Title,
        [string]$Command
    )

    $ShellCommand = "cd `"$RootDir`"; `$Host.UI.RawUI.WindowTitle = `"$Title`"; $Command; Read-Host 'Press Enter to close'"
    Start-Process powershell -ArgumentList @("-NoExit", "-ExecutionPolicy", "Bypass", "-Command", $ShellCommand)
}

Write-Host "Starting rust-desk-light dev stack"
Write-Host "server: ${Ip}:${Port}"
Write-Host "logs: $LogDir"

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
Push-Location $RootDir
cargo build --workspace
Pop-Location

$ServerExe = Join-Path $RootDir "target\debug\rdl-server.exe"
$ClientExe = Join-Path $RootDir "target\debug\rdl-client.exe"
$AdminExe = Join-Path $RootDir "target\debug\rdl-admin.exe"
if (-not (Test-Path $ServerExe)) { $ServerExe = Join-Path $RootDir "target\debug\rdl-server" }
if (-not (Test-Path $ClientExe)) { $ClientExe = Join-Path $RootDir "target\debug\rdl-client" }
if (-not (Test-Path $AdminExe)) { $AdminExe = Join-Path $RootDir "target\debug\rdl-admin" }

Start-RdlWindow "rdl-server" "& `"$ServerExe`" --ip $Ip --port $Port"
Start-Sleep -Seconds 1
Start-Process -FilePath $ClientExe -ArgumentList @("--ip", $Ip, "--port", $Port) -RedirectStandardOutput (Join-Path $LogDir "client.log") -RedirectStandardError (Join-Path $LogDir "client.err.log")
Start-Sleep -Seconds 1
Start-Process -FilePath $AdminExe -ArgumentList @("--ip", $Ip, "--port", $Port) -RedirectStandardOutput (Join-Path $LogDir "admin.log") -RedirectStandardError (Join-Path $LogDir "admin.err.log")

Write-Host "Started server terminal, client GUI, and admin GUI."
