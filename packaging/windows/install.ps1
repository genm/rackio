[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Archive,
    [Parameter(Mandatory = $true)]
    [string]$Checksum,
    # The SHA-1 thumbprint of the certificate that must have signed
    # rackio.exe. There is deliberately no built-in default: the project's
    # release signing certificate identity does not exist yet, and accepting
    # any trusted-root signature (the prior behavior) let any signed binary
    # register as an auto-start LocalSystem service. Falls back to the
    # RACKIO_EXPECTED_SIGNING_THUMBPRINT environment variable, then to
    # expected-thumbprint.txt next to this script, if not passed explicitly.
    [string]$ExpectedThumbprint
)

$ErrorActionPreference = "Stop"
$serviceName = "RackioAgent"
$viewerGroup = "Rackio Viewers"
$installDir = Join-Path $env:ProgramFiles "Rackio"
$dataRoot = Join-Path $env:ProgramData "Rackio"
# Well-known SIDs: LocalSystem, and the built-in Administrators group.
$trustedOwnerSids = @("S-1-5-18", "S-1-5-32-544")

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Rackio installation requires an elevated PowerShell"
}

if ([string]::IsNullOrWhiteSpace($ExpectedThumbprint)) {
    $ExpectedThumbprint = $env:RACKIO_EXPECTED_SIGNING_THUMBPRINT
}
if ([string]::IsNullOrWhiteSpace($ExpectedThumbprint)) {
    $thumbprintFile = Join-Path $PSScriptRoot "expected-thumbprint.txt"
    if (Test-Path -LiteralPath $thumbprintFile) {
        $ExpectedThumbprint = (Get-Content -Raw -LiteralPath $thumbprintFile).Trim()
    }
}
if ([string]::IsNullOrWhiteSpace($ExpectedThumbprint)) {
    throw ("an expected Authenticode signer thumbprint is required; supply " +
        "-ExpectedThumbprint, set RACKIO_EXPECTED_SIGNING_THUMBPRINT, or " +
        "check in packaging/windows/expected-thumbprint.txt. Refusing to " +
        "install: any signature chaining to a trusted root is not a " +
        "publisher check")
}
$ExpectedThumbprint = $ExpectedThumbprint.Trim().ToUpperInvariant() -replace '[^0-9A-F]', ''
if ($ExpectedThumbprint.Length -ne 40) {
    throw "expected signer thumbprint must be a 40-character SHA-1 hex thumbprint"
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
    $mitLicense = Get-ChildItem -Path $temporary -Filter "LICENSE-MIT" -Recurse | Select-Object -First 1
    $apacheLicense = Get-ChildItem -Path $temporary -Filter "LICENSE-APACHE" -Recurse | Select-Object -First 1
    $thirdPartyNotices = Get-ChildItem -Path $temporary -Filter "THIRDPARTY.html" -Recurse | Select-Object -First 1
    if (
        $null -eq $binary -or
        $null -eq $uninstaller -or
        $null -eq $mitLicense -or
        $null -eq $apacheLicense -or
        $null -eq $thirdPartyNotices
    ) {
        throw "release archive must contain the binary, uninstaller, project licenses, and third-party notices"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $binary.FullName
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "rackio.exe must have a valid Authenticode signature"
    }
    if ($null -eq $signature.SignerCertificate) {
        throw "rackio.exe Authenticode signature has no signer certificate"
    }
    $actualThumbprint = $signature.SignerCertificate.Thumbprint.Trim().ToUpperInvariant()
    if ($actualThumbprint -ne $ExpectedThumbprint) {
        throw ("rackio.exe is signed by an unexpected certificate " +
            "(thumbprint $actualThumbprint, expected $ExpectedThumbprint); " +
            "refusing to install a binary from an unpinned publisher")
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

    # C:\ProgramData grants BUILTIN\Users create-folder rights, so a standard
    # user can pre-create Rackio's data root before this installer ever runs.
    # `icacls /inheritance:r /grant:r` below only replaces grants for the
    # principals it names and never touches the owner, so an attacker-created
    # directory would keep WRITE_DAC via its original owner and could
    # re-grant itself access later. Refuse to reuse such a directory instead
    # of silently taking it over.
    if (Test-Path -LiteralPath $dataRoot -PathType Container) {
        $existingAcl = Get-Acl -LiteralPath $dataRoot
        $existingOwnerSid = $existingAcl.GetOwner([Security.Principal.SecurityIdentifier]).Value
        if ($trustedOwnerSids -notcontains $existingOwnerSid) {
            throw ("refusing to install: $dataRoot already exists and is " +
                "owned by $($existingAcl.Owner), not SYSTEM or " +
                "Administrators. Remove it (after verifying it holds no " +
                "identity you need to keep) and re-run the installer")
        }
    }

    New-Item -ItemType Directory -Force -Path $installDir, $dataRoot | Out-Null
    if (Get-Service -Name $serviceName -ErrorAction SilentlyContinue) {
        Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
        & sc.exe delete $serviceName | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Copy-Item -Force -LiteralPath $binary.FullName -Destination (Join-Path $installDir "rackio.exe")
    Copy-Item -Force -LiteralPath $uninstaller.FullName -Destination (Join-Path $installDir "uninstall.ps1")
    Copy-Item -Force -LiteralPath $mitLicense.FullName -Destination (Join-Path $installDir "LICENSE-MIT")
    Copy-Item -Force -LiteralPath $apacheLicense.FullName -Destination (Join-Path $installDir "LICENSE-APACHE")
    Copy-Item -Force -LiteralPath $thirdPartyNotices.FullName -Destination (Join-Path $installDir "THIRDPARTY.html")

    & icacls.exe $installDir /setowner "SYSTEM" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "failed to set the owner of $installDir to SYSTEM"
    }
    & icacls.exe $installDir /inheritance:r /grant:r `
        "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" "${viewerGroup}:(OI)(CI)RX" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "failed to set the ACL on $installDir"
    }
    & icacls.exe $dataRoot /setowner "SYSTEM" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "failed to set the owner of $dataRoot to SYSTEM"
    }
    & icacls.exe $dataRoot /inheritance:r /grant:r `
        "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "failed to set the ACL on $dataRoot"
    }

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
