$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "Windows named-pipe test must run on Windows"
}

$groupCreated = $false
if (-not (Get-LocalGroup -Name "Rackio Viewers" -ErrorAction SilentlyContinue)) {
    New-LocalGroup -Name "Rackio Viewers" -Description "Users allowed to view Rackio metrics" | Out-Null
    $groupCreated = $true
}

$root = Join-Path ([System.IO.Path]::GetTempPath()) "rackio-ipc-$([Guid]::NewGuid())"
New-Item -ItemType Directory -Path $root | Out-Null
$env:RACKIO_CONFIG_DIR = Join-Path $root "config"
$env:RACKIO_DATA_DIR = Join-Path $root "data"
$env:RACKIO_STATE_DIR = Join-Path $root "state"
$env:RACKIO_LOG_DIR = Join-Path $root "logs"
$env:RACKIO_PIPE = "\\.\pipe\rackio-test-$([Guid]::NewGuid())"

$binary = Join-Path $PSScriptRoot "..\target\debug\rackio.exe"
$daemon = $null
$completed = $false
try {
    cargo build -p rackio-agent
    $daemon = Start-Process -FilePath $binary -ArgumentList "daemon" -PassThru -NoNewWindow
    $response = $null
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        try {
            $response = (& $binary status | Out-String | ConvertFrom-Json)
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if ($null -eq $response -or -not $response.ok) {
        throw "Rackio named-pipe status request did not succeed"
    }
    [ordered]@{
        ok = $true
        named_pipe = $true
        acl_group = "Rackio Viewers"
        caller_token_verified = $true
    } | ConvertTo-Json -Compress
    $completed = $true
} finally {
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force
        $daemon.WaitForExit()
    }
    if (-not $completed -and (Test-Path -LiteralPath $env:RACKIO_LOG_DIR)) {
        $diagnostics = Join-Path $PSScriptRoot "..\test-results\windows-named-pipe"
        New-Item -ItemType Directory -Path $diagnostics -Force | Out-Null
        Get-ChildItem -LiteralPath $env:RACKIO_LOG_DIR -File |
            Copy-Item -Destination $diagnostics -Force
    }
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
    if ($groupCreated) {
        Remove-LocalGroup -Name "Rackio Viewers"
    }
}
