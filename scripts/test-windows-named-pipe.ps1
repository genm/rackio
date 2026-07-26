$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "Windows named-pipe test must run on Windows"
}

$testUser = "rackio-ci-$([Guid]::NewGuid().ToString("N").Substring(0, 8))"
$testPassword = "Rk!9$([Guid]::NewGuid().ToString("N"))"
$securePassword = ConvertTo-SecureString $testPassword -AsPlainText -Force
$credential = [PSCredential]::new("$env:COMPUTERNAME\$testUser", $securePassword)
$groupCreated = $false
$root = $null
$binary = $null
$daemon = $null
$completed = $false
$userCreated = $false
$localPipe = $null
$environmentNames = @(
    "RACKIO_CONFIG_DIR",
    "RACKIO_DATA_DIR",
    "RACKIO_STATE_DIR",
    "RACKIO_LOG_DIR",
    "RACKIO_PIPE"
)
$previousEnvironment = @{}
foreach ($environmentName in $environmentNames) {
    $previousEnvironment[$environmentName] =
        [Environment]::GetEnvironmentVariable($environmentName, "Process")
}

function Invoke-StatusAsTestUser {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $stdout = Join-Path $root "$Name.stdout"
    $stderr = Join-Path $root "$Name.stderr"
    $process = Start-Process `
        -FilePath $binary `
        -ArgumentList "status" `
        -Credential $credential `
        -LoadUserProfile `
        -PassThru `
        -RedirectStandardOutput $stdout `
        -RedirectStandardError $stderr
    if (-not $process.WaitForExit(10_000)) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
        throw "$Name status client did not exit within ten seconds"
    }
    [ordered]@{
        exit_code = $process.ExitCode
        stdout = if (Test-Path -LiteralPath $stdout) {
            Get-Content -Raw -LiteralPath $stdout
        } else {
            ""
        }
        stderr = if (Test-Path -LiteralPath $stderr) {
            Get-Content -Raw -LiteralPath $stderr
        } else {
            ""
        }
    }
}

try {
    if (-not (Get-LocalGroup -Name "Rackio Viewers" -ErrorAction SilentlyContinue)) {
        New-LocalGroup -Name "Rackio Viewers" -Description "Users allowed to view Rackio metrics" | Out-Null
        $groupCreated = $true
    }

    $root = Join-Path ([System.IO.Path]::GetTempPath()) "rackio-ipc-$([Guid]::NewGuid())"
    New-Item -ItemType Directory -Path $root | Out-Null
    # The temporary non-administrator account needs only this test directory for
    # its Rackio state and redirected output. Use the language-neutral Users SID.
    & icacls.exe $root /grant "*S-1-5-32-545:(OI)(CI)M" /Q | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to grant temporary test-directory access"
    }
    $env:RACKIO_CONFIG_DIR = Join-Path $root "config"
    $env:RACKIO_DATA_DIR = Join-Path $root "data"
    $env:RACKIO_STATE_DIR = Join-Path $root "state"
    $env:RACKIO_LOG_DIR = Join-Path $root "logs"
    $env:RACKIO_PIPE = "\\.\pipe\rackio-test-$([Guid]::NewGuid())"
    $localPipe = $env:RACKIO_PIPE

    cargo build -p rackio-agent
    $targetDirectory = (cargo metadata --format-version=1 --no-deps | ConvertFrom-Json).target_directory
    $binary = Join-Path $targetDirectory "debug\rackio.exe"
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

    New-LocalUser `
        -Name $testUser `
        -Password $securePassword `
        -AccountNeverExpires `
        -PasswordNeverExpires `
        -UserMayNotChangePassword `
        -Description "Temporary Rackio named-pipe CI account" | Out-Null
    $userCreated = $true
    Add-LocalGroupMember -Group "Rackio Viewers" -Member $testUser

    $allowed = Invoke-StatusAsTestUser -Name "allowed"
    if ($allowed.exit_code -ne 0) {
        throw "Rackio Viewers member was rejected: $($allowed.stderr)"
    }
    $allowedResponse = $allowed.stdout | ConvertFrom-Json
    if (-not $allowedResponse.ok) {
        throw "Rackio Viewers member received an unsuccessful response"
    }

    Remove-LocalGroupMember -Group "Rackio Viewers" -Member $testUser
    $denied = Invoke-StatusAsTestUser -Name "denied"
    if ($denied.exit_code -eq 0) {
        throw "Non-viewer account unexpectedly connected to the Rackio pipe"
    }

    $env:RACKIO_PIPE = "\\localhost\pipe\rackio-test-$([Guid]::NewGuid())"
    $savedErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $remoteOutput = & $binary status 2>&1 | Out-String
        $remoteExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedErrorActionPreference
        $env:RACKIO_PIPE = $localPipe
    }
    if ($remoteExitCode -eq 0 -or $remoteOutput -notmatch "must start with") {
        throw "Remote named-pipe configuration was not rejected locally"
    }

    [ordered]@{
        ok = $true
        named_pipe = $true
        acl_group = "Rackio Viewers"
        authorized_viewer = $true
        unauthorized_user_rejected = $true
        remote_pipe_name_rejected = $true
    } | ConvertTo-Json -Compress
    $completed = $true
} finally {
    if ($null -ne $localPipe) {
        $env:RACKIO_PIPE = $localPipe
    }
    try {
        if ($null -ne $daemon -and -not $daemon.HasExited) {
            Stop-Process -Id $daemon.Id -Force
            $daemon.WaitForExit()
        }
        $logDirectory = $env:RACKIO_LOG_DIR
        if (
            -not $completed -and
            $null -ne $root -and
            -not [string]::IsNullOrWhiteSpace($logDirectory) -and
            (Test-Path -LiteralPath $logDirectory)
        ) {
            $diagnostics = Join-Path $PSScriptRoot "..\test-results\windows-named-pipe"
            New-Item -ItemType Directory -Path $diagnostics -Force | Out-Null
            Get-ChildItem -LiteralPath $logDirectory -File |
                Copy-Item -Destination $diagnostics -Force
        }
    } finally {
        try {
            if ($userCreated) {
                Remove-LocalUser -Name $testUser
            }
        } finally {
            try {
                if ($null -ne $root) {
                    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
                }
            } finally {
                try {
                    if ($groupCreated) {
                        Remove-LocalGroup -Name "Rackio Viewers"
                    }
                } finally {
                    foreach ($environmentName in $environmentNames) {
                        [Environment]::SetEnvironmentVariable(
                            $environmentName,
                            $previousEnvironment[$environmentName],
                            "Process"
                        )
                    }
                    $testPassword = $null
                }
            }
        }
    }
}
