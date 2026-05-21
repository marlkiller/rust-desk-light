$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = (Resolve-Path (Join-Path $ScriptDir "..")).Path

$BuildMode = "release"
foreach ($Arg in $args) {
    switch -Regex ($Arg) {
        "^(release|--release|-r)$" {
            $BuildMode = "release"
            continue
        }
        "^(debug|--debug)$" {
            $BuildMode = "debug"
            continue
        }
        "^(--help|-h)$" {
            Write-Host "Usage: .\scripts\package-apps.ps1 [debug|release|--debug|--release|-r]"
            Write-Host
            Write-Host "Packages Rust Desk Light Client and Admin as separate local app directories."
            exit 0
        }
        default {
            Write-Error "Unknown argument: $Arg"
        }
    }
}

$CargoProfileArgs = @()
$TargetProfileDir = "debug"
if ($BuildMode -eq "release") {
    $CargoProfileArgs = @("--release")
    $TargetProfileDir = "release"
}

function Get-PlatformName {
    $Arch = $env:PROCESSOR_ARCHITECTURE
    if ($Arch -eq "ARM64") {
        return "windows-arm64"
    }
    return "windows-x64"
}

function Reset-Directory {
    param([string]$Path)

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Copy-ConfigTemplates {
    param([string]$Destination)

    $ConfigSource = Join-Path $RootDir "config"
    if (Test-Path -LiteralPath $ConfigSource) {
        Copy-Item -LiteralPath $ConfigSource -Destination (Join-Path $Destination "config") -Recurse -Force
    }
}

function Ensure-ResourceUpdater {
    if ("NativeResourceUpdater" -as [type]) {
        return
    }

    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class NativeResourceUpdater {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr BeginUpdateResource(string pFileName, bool bDeleteExistingResources);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool UpdateResource(IntPtr hUpdate, IntPtr lpType, IntPtr lpName, ushort wLanguage, byte[] lpData, uint cbData);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool EndUpdateResource(IntPtr hUpdate, bool fDiscard);
}
"@
}

function Read-UInt16LE {
    param(
        [byte[]]$Bytes,
        [int]$Offset
    )

    return [BitConverter]::ToUInt16($Bytes, $Offset)
}

function Read-UInt32LE {
    param(
        [byte[]]$Bytes,
        [int]$Offset
    )

    return [BitConverter]::ToUInt32($Bytes, $Offset)
}

function Write-ExecutableIcon {
    param(
        [string]$ExePath,
        [string]$IconPath
    )

    Ensure-ResourceUpdater

    $IconBytes = [IO.File]::ReadAllBytes($IconPath)
    if ($IconBytes.Length -lt 6) {
        throw "Invalid icon file: $IconPath"
    }

    $Reserved = Read-UInt16LE $IconBytes 0
    $IconType = Read-UInt16LE $IconBytes 2
    $IconCount = Read-UInt16LE $IconBytes 4
    if ($Reserved -ne 0 -or $IconType -ne 1 -or $IconCount -lt 1) {
        throw "Invalid Windows icon file: $IconPath"
    }

    $Entries = @()
    for ($Index = 0; $Index -lt $IconCount; $Index++) {
        $EntryOffset = 6 + (16 * $Index)
        $ImageSize = [int](Read-UInt32LE $IconBytes ($EntryOffset + 8))
        $ImageOffset = [int](Read-UInt32LE $IconBytes ($EntryOffset + 12))
        if ($ImageOffset -lt 0 -or $ImageSize -lt 1 -or ($ImageOffset + $ImageSize) -gt $IconBytes.Length) {
            throw "Invalid icon image entry in: $IconPath"
        }

        $ImageBytes = New-Object byte[] $ImageSize
        [Array]::Copy($IconBytes, $ImageOffset, $ImageBytes, 0, $ImageSize)

        $Entries += [pscustomobject]@{
            Id = [UInt16]($Index + 1)
            Width = [byte]$IconBytes[$EntryOffset]
            Height = [byte]$IconBytes[$EntryOffset + 1]
            ColorCount = [byte]$IconBytes[$EntryOffset + 2]
            Reserved = [byte]$IconBytes[$EntryOffset + 3]
            Planes = [UInt16](Read-UInt16LE $IconBytes ($EntryOffset + 4))
            BitCount = [UInt16](Read-UInt16LE $IconBytes ($EntryOffset + 6))
            ImageSize = [UInt32]$ImageSize
            ImageBytes = $ImageBytes
        }
    }

    $GroupStream = New-Object IO.MemoryStream
    $GroupWriter = New-Object IO.BinaryWriter($GroupStream)
    $GroupWriter.Write([UInt16]0)
    $GroupWriter.Write([UInt16]1)
    $GroupWriter.Write([UInt16]$IconCount)
    foreach ($Entry in $Entries) {
        $GroupWriter.Write([byte]$Entry.Width)
        $GroupWriter.Write([byte]$Entry.Height)
        $GroupWriter.Write([byte]$Entry.ColorCount)
        $GroupWriter.Write([byte]$Entry.Reserved)
        $GroupWriter.Write([UInt16]$Entry.Planes)
        $GroupWriter.Write([UInt16]$Entry.BitCount)
        $GroupWriter.Write([UInt32]$Entry.ImageSize)
        $GroupWriter.Write([UInt16]$Entry.Id)
    }
    $GroupWriter.Flush()
    $GroupBytes = $GroupStream.ToArray()
    $GroupWriter.Dispose()
    $GroupStream.Dispose()

    $Handle = [NativeResourceUpdater]::BeginUpdateResource($ExePath, $false)
    if ($Handle -eq [IntPtr]::Zero) {
        throw "BeginUpdateResource failed for $ExePath. Win32 error: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }

    $Succeeded = $false
    try {
        foreach ($Entry in $Entries) {
            $Ok = [NativeResourceUpdater]::UpdateResource(
                $Handle,
                [IntPtr]3,
                [IntPtr]$Entry.Id,
                [UInt16]0,
                $Entry.ImageBytes,
                [UInt32]$Entry.ImageBytes.Length
            )
            if (-not $Ok) {
                throw "UpdateResource RT_ICON failed for $ExePath. Win32 error: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
            }
        }

        $Ok = [NativeResourceUpdater]::UpdateResource(
            $Handle,
            [IntPtr]14,
            [IntPtr]1,
            [UInt16]0,
            $GroupBytes,
            [UInt32]$GroupBytes.Length
        )
        if (-not $Ok) {
            throw "UpdateResource RT_GROUP_ICON failed for $ExePath. Win32 error: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
        }

        $Succeeded = $true
    }
    finally {
        $Discard = -not $Succeeded
        $Ok = [NativeResourceUpdater]::EndUpdateResource($Handle, $Discard)
        if ($Succeeded -and -not $Ok) {
            throw "EndUpdateResource failed for $ExePath. Win32 error: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
        }
    }
}

function Set-WindowsGuiSubsystem {
    param([string]$ExePath)

    $Bytes = [IO.File]::ReadAllBytes($ExePath)
    if ($Bytes.Length -lt 0x100 -or $Bytes[0] -ne 0x4d -or $Bytes[1] -ne 0x5a) {
        throw "Not a valid PE executable: $ExePath"
    }

    $PeOffset = [BitConverter]::ToInt32($Bytes, 0x3c)
    if ($PeOffset -lt 0 -or ($PeOffset + 0x5c) -ge $Bytes.Length) {
        throw "Invalid PE header offset in: $ExePath"
    }
    if ($Bytes[$PeOffset] -ne 0x50 -or $Bytes[$PeOffset + 1] -ne 0x45 -or $Bytes[$PeOffset + 2] -ne 0 -or $Bytes[$PeOffset + 3] -ne 0) {
        throw "Invalid PE signature in: $ExePath"
    }

    $OptionalHeaderOffset = $PeOffset + 24
    $Magic = Read-UInt16LE $Bytes $OptionalHeaderOffset
    if ($Magic -ne 0x10b -and $Magic -ne 0x20b) {
        throw "Unsupported PE optional header in: $ExePath"
    }

    $SubsystemOffset = $OptionalHeaderOffset + 0x44
    $GuiSubsystem = [BitConverter]::GetBytes([UInt16]2)
    $Stream = [IO.File]::Open($ExePath, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        [void]$Stream.Seek($SubsystemOffset, [IO.SeekOrigin]::Begin)
        $Stream.Write($GuiSubsystem, 0, $GuiSubsystem.Length)
    }
    finally {
        $Stream.Dispose()
    }
}

function New-WindowsAppPackage {
    param(
        [string]$DisplayName,
        [string]$PackageSlug,
        [string]$BinaryName
    )

    $PlatformDir = Join-Path (Join-Path $RootDir "dist\apps") (Get-PlatformName)
    $AppDir = Join-Path $PlatformDir $DisplayName
    $ExeSource = Join-Path (Join-Path $RootDir "target\$TargetProfileDir") "$BinaryName.exe"
    $IconSource = Join-Path $RootDir "assets\icons\rdl-icon.ico"
    $ReadmePath = Join-Path $AppDir "README.txt"
    $ZipPath = Join-Path $PlatformDir "$PackageSlug-$(Get-PlatformName).zip"

    if (-not (Test-Path -LiteralPath $ExeSource)) {
        throw "Missing built executable: $ExeSource"
    }
    if (-not (Test-Path -LiteralPath $IconSource)) {
        throw "Missing icon: $IconSource"
    }

    Reset-Directory $AppDir
    $PackagedExe = Join-Path $AppDir "$BinaryName.exe"
    Copy-Item -LiteralPath $ExeSource -Destination $PackagedExe -Force
    Set-WindowsGuiSubsystem -ExePath $PackagedExe
    Write-ExecutableIcon -ExePath $PackagedExe -IconPath $IconSource
    Copy-Item -LiteralPath $IconSource -Destination (Join-Path $AppDir "rdl-icon.ico") -Force
    Copy-ConfigTemplates $AppDir

    @(
        $DisplayName,
        "",
        "Double-click $BinaryName.exe to start the app.",
        "This packaged copy uses the Windows GUI subsystem and includes the application icon.",
        "Config templates are included in the config directory."
    ) | Set-Content -LiteralPath $ReadmePath -Encoding ASCII

    if (Test-Path -LiteralPath $ZipPath) {
        Remove-Item -LiteralPath $ZipPath -Force
    }
    Compress-Archive -LiteralPath $AppDir -DestinationPath $ZipPath -Force

    Write-Host "Packaged $DisplayName"
    Write-Host "  Directory: $AppDir"
    Write-Host "  Archive:   $ZipPath"
}

Push-Location $RootDir
try {
    Write-Host "Building Rust Desk Light Client ($BuildMode)"
    cargo build -p rust-desk-light-client --bin rdl-client-gui @CargoProfileArgs

    Write-Host "Building Rust Desk Light Admin ($BuildMode)"
    cargo build -p rust-desk-light-admin --bin rdl-admin-gui @CargoProfileArgs

    New-WindowsAppPackage `
        -DisplayName "Rust Desk Light Client" `
        -PackageSlug "Rust-Desk-Light-Client" `
        -BinaryName "rdl-client-gui"

    New-WindowsAppPackage `
        -DisplayName "Rust Desk Light Admin" `
        -PackageSlug "Rust-Desk-Light-Admin" `
        -BinaryName "rdl-admin-gui"
}
finally {
    Pop-Location
}
