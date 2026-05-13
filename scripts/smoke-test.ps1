$ErrorActionPreference = "Stop"

$RootDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$Ip = if ($env:RDL_IP) { $env:RDL_IP } else { "127.0.0.1" }
$Port = if ($env:RDL_PORT) { $env:RDL_PORT } else { "21116" }
$LogDir = Join-Path $RootDir "target\rdl-smoke"
$ServerProcess = $null
$ClientProcess = $null

function Stop-StartedProcess {
    param($Process)
    if ($null -ne $Process -and -not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    }
}

function Wait-ForLog {
    param(
        [string]$Path,
        [string]$Pattern,
        [string]$Label
    )

    for ($i = 0; $i -lt 80; $i++) {
        if ((Test-Path $Path) -and ((Get-Content $Path -Raw) -match $Pattern)) {
            return
        }
        Start-Sleep -Milliseconds 100
    }

    Write-Host "Timed out waiting for $Label"
    if (Test-Path $Path) {
        Get-Content $Path
    }
    exit 1
}

try {
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    Remove-Item -Force -ErrorAction SilentlyContinue `
        (Join-Path $LogDir "server.log"), `
        (Join-Path $LogDir "client.log"), `
        (Join-Path $LogDir "admin.log")

    Push-Location $RootDir

    Write-Host "[1/5] Building workspace"
    cargo build --workspace

    $ServerExe = Join-Path $RootDir "target\debug\rdl-server.exe"
    $ClientExe = Join-Path $RootDir "target\debug\rdl-client.exe"
    $AdminExe = Join-Path $RootDir "target\debug\rdl-admin.exe"
    if (-not (Test-Path $ServerExe)) { $ServerExe = Join-Path $RootDir "target\debug\rdl-server" }
    if (-not (Test-Path $ClientExe)) { $ClientExe = Join-Path $RootDir "target\debug\rdl-client" }
    if (-not (Test-Path $AdminExe)) { $AdminExe = Join-Path $RootDir "target\debug\rdl-admin" }

    Write-Host "[2/5] Starting server on ${Ip}:${Port}"
    $ServerProcess = Start-Process -FilePath $ServerExe -ArgumentList @("--ip", $Ip, "--port", $Port) -RedirectStandardOutput (Join-Path $LogDir "server.log") -RedirectStandardError (Join-Path $LogDir "server.err.log") -PassThru -WindowStyle Hidden
    Wait-ForLog (Join-Path $LogDir "server.log") "server listening" "server startup"

    Write-Host "[3/5] Starting client"
    $env:RDL_FORCE_TERMINAL = "1"
    $ClientProcess = Start-Process -FilePath $ClientExe -ArgumentList @("--ip", $Ip, "--port", $Port) -RedirectStandardOutput (Join-Path $LogDir "client.log") -RedirectStandardError (Join-Path $LogDir "client.err.log") -PassThru -WindowStyle Hidden
    Wait-ForLog (Join-Path $LogDir "client.log") "client id:" "client registration"

    $ClientId = (Get-Content (Join-Path $LogDir "client.log") | Select-String "^client id: " | Select-Object -Last 1).Line -replace "^client id: ", ""
    if (-not $ClientId) {
        Write-Host "Could not detect client id"
        Get-Content (Join-Path $LogDir "client.log")
        exit 1
    }

    Write-Host "[4/5] Running admin command flow for client: $ClientId"
    $AdminInput = "list`ncmd $ClientId computer_info`nquit`n"
    $AdminInput | & $AdminExe --ip $Ip --port $Port *> (Join-Path $LogDir "admin.log")

    Write-Host "[5/5] Verifying output"
    $AdminLog = Get-Content (Join-Path $LogDir "admin.log") -Raw
    if ($AdminLog -notmatch "online clients: 1") { throw "Admin did not list one online client" }
    if ($AdminLog -notmatch "command=computer_info") { throw "Admin did not receive command ack" }
    if ($AdminLog -notmatch "hostname=") { throw "Admin did not receive client computer info" }

    Write-Host "Smoke test passed."
    Write-Host "Logs: $LogDir"
}
finally {
    Stop-StartedProcess $ClientProcess
    Stop-StartedProcess $ServerProcess
    Pop-Location
}

