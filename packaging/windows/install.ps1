[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Archive,
    [Parameter(Mandatory = $true)]
    [string]$Checksum
)

$ErrorActionPreference = "Stop"
$serviceName = "RackioAgent"
$viewerGroup = "Rackio Viewers"
$installDir = Join-Path $env:ProgramFiles "Rackio"
$dataRoot = Join-Path $env:ProgramData "Rackio"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Rackio installation requires an elevated PowerShell"
}

$checksumLine = (Get-Content -Raw -LiteralPath $Checksum).Trim()
if ($checksumLine -notmatch "^(?<digest>[0-9a-fA-F]{64})(?:\s+\*?.+)?$") {
    throw "release checksum must contain one SHA-256 digest"
}
$expectedDigest = $Matches.digest
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Archive).Hash
if ($actual -ne $expectedDigest) {
    throw "release archive checksum does not match"
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) "rackio-install-$([Guid]::NewGuid())"
New-Item -ItemType Directory -Path $temporary | Out-Null
$serviceCreated = $false
try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $temporary
    $binary = Get-ChildItem -Path $temporary -Filter "rackio.exe" -Recurse | Select-Object -First 1
    $uninstaller = Get-ChildItem -Path $temporary -Filter "uninstall.ps1" -Recurse | Select-Object -First 1
    if ($null -eq $binary -or $null -eq $uninstaller) {
        throw "release archive must contain rackio.exe and uninstall.ps1"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $binary.FullName
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "rackio.exe must have a valid Authenticode signature"
    }

    if (-not (Get-LocalGroup -Name $viewerGroup -ErrorAction SilentlyContinue)) {
        New-LocalGroup -Name $viewerGroup -Description "Users allowed to view Rackio metrics" | Out-Null
    }
    $currentUser = $identity.Name
    $alreadyMember = Get-LocalGroupMember -Group $viewerGroup -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $currentUser }
    if (-not $alreadyMember) {
        Add-LocalGroupMember -Group $viewerGroup -Member $currentUser
    }

    New-Item -ItemType Directory -Force -Path $installDir, $dataRoot | Out-Null
    if (Get-Service -Name $serviceName -ErrorAction SilentlyContinue) {
        Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
        & sc.exe delete $serviceName | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Copy-Item -Force -LiteralPath $binary.FullName -Destination (Join-Path $installDir "rackio.exe")
    Copy-Item -Force -LiteralPath $uninstaller.FullName -Destination (Join-Path $installDir "uninstall.ps1")

    & icacls.exe $installDir /inheritance:r /grant:r `
        "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" "${viewerGroup}:(OI)(CI)RX" | Out-Null
    & icacls.exe $dataRoot /inheritance:r /grant:r `
        "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null

    $installedBinary = Join-Path $installDir "rackio.exe"
    $binPath = "`"$installedBinary`" daemon"
    & sc.exe create $serviceName "binPath= $binPath" "start= auto" "obj= LocalSystem" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "failed to create the Rackio Windows Service"
    }
    $serviceCreated = $true
    $serviceRegistry = "HKLM:\SYSTEM\CurrentControlSet\Services\$serviceName"
    New-ItemProperty -Path $serviceRegistry -Name "Environment" -PropertyType MultiString -Force -Value @(
        "RACKIO_CONFIG_DIR=$(Join-Path $dataRoot 'config')",
        "RACKIO_DATA_DIR=$(Join-Path $dataRoot 'data')",
        "RACKIO_STATE_DIR=$(Join-Path $dataRoot 'state')",
        "RACKIO_LOG_DIR=$(Join-Path $dataRoot 'logs')",
        "RACKIO_PIPE=\\.\pipe\rackio-agent"
    ) | Out-Null
    & sc.exe description $serviceName "Rackio peer-to-peer monitoring agent" | Out-Null
    & sc.exe failure $serviceName "reset= 86400" "actions= restart/5000/restart/15000/restart/60000" | Out-Null
    Start-Service -Name $serviceName
    $service = Get-Service -Name $serviceName
    $service.WaitForStatus("Running", [TimeSpan]::FromSeconds(20))

    & $installedBinary status | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Rackio service started but the secured named-pipe health check failed"
    }
    Write-Output "Rackio installed. Sign out and back in before using a non-elevated tray."
} catch {
    if ($serviceCreated) {
        Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
        & sc.exe delete $serviceName | Out-Null
    }
    throw
} finally {
    Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
}
