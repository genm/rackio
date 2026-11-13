[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Binary,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [string]$OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) {
    throw "rackio binary does not exist: $Binary"
}
if ([string]::IsNullOrWhiteSpace($env:RACKIO_SIGNTOOL_CERT_SHA1)) {
    throw "RACKIO_SIGNTOOL_CERT_SHA1 is required for a release archive"
}
$staging = Join-Path ([System.IO.Path]::GetTempPath()) "rackio-package-$([Guid]::NewGuid())"
$archiveName = "rackio-v$Version-$Target.zip"
$archive = Join-Path $OutputDirectory $archiveName
try {
    New-Item -ItemType Directory -Force -Path $staging, $OutputDirectory | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $staging "rackio.exe")
    & signtool.exe sign /sha1 $env:RACKIO_SIGNTOOL_CERT_SHA1 /fd SHA256 `
        /tr "http://timestamp.digicert.com" /td SHA256 (Join-Path $staging "rackio.exe")
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode signing failed"
    }
    & signtool.exe verify /pa /all (Join-Path $staging "rackio.exe")
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed"
    }
    Copy-Item -LiteralPath "packaging/windows/uninstall.ps1" -Destination $staging
    Copy-Item -LiteralPath "LICENSE-MIT", "LICENSE-APACHE", "THIRDPARTY.html" -Destination $staging
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive -Force
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    Set-Content -NoNewline -LiteralPath "$archive.sha256" -Value "$digest  $archiveName`n"
    Write-Output $archive
} finally {
    Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
}
