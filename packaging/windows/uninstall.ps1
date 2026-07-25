[CmdletBinding()]
param([switch]$Purge)

$ErrorActionPreference = "Stop"
$serviceName = "RackioAgent"
$installDir = Join-Path $env:ProgramFiles "Rackio"
$dataRoot = Join-Path $env:ProgramData "Rackio"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Rackio uninstall requires an elevated PowerShell"
}

if (Get-Service -Name $serviceName -ErrorAction SilentlyContinue) {
    Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
    & sc.exe delete $serviceName | Out-Null
}
Remove-Item -Recurse -Force $installDir -ErrorAction SilentlyContinue
if ($Purge) {
    Remove-Item -Recurse -Force $dataRoot -ErrorAction SilentlyContinue
    if (Get-LocalGroup -Name "Rackio Viewers" -ErrorAction SilentlyContinue) {
        Remove-LocalGroup -Name "Rackio Viewers"
    }
    Write-Output "Rackio service, binaries, identity, configuration and history removed."
} else {
    Write-Output "Rackio service and binaries removed; identity, configuration and history preserved."
}
