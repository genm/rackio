# Measure the release daemon against the resource budgets in
# docs/release-checklist.md ("Product acceptance"), on Windows.
#
# This is the Windows-native counterpart to
# scripts/benchmark-agent-resources.sh's `idle_direct_only` profile: one
# daemon, no peer, no pairing, CPU and RSS only.
#
#   pwsh scripts/benchmark-agent-resources.ps1
#   pwsh scripts/benchmark-agent-resources.ps1 C:\path\to\rackio.exe
#
# The `active_peer` profile (two paired daemons plus `nettop`-measured
# traffic) has no Windows counterpart here. `nettop` is macOS-only and
# Windows has no equivalent per-process UDP byte counter available without
# elevated capture; that gap is tracked separately in
# docs/release-checklist.md rather than papered over with an unmeasured
# number.
#
# CPU is gated the same way the corrected bash script gates it: a delta of
# the process's own cumulative CPU time over the sampling window, not an
# instantaneous or decaying %CPU-style figure. The reason is the same one
# that retired `ps -o %cpu` from gating on macOS (see
# scripts/benchmark-agent-resources.sh `cpu_seconds_of`): a decaying/lifetime
# average is dominated by process startup and moves with warm-up length
# rather than with real steady-state load. `Get-Process`'s `TotalProcessorTime`
# is the Windows analogue of `ps -o cputime` -- total processor time consumed
# since process start, not a decaying average -- so the same
# delta-over-window technique applies directly.

param(
    [Parameter(Position = 0)]
    [string]$Binary
)

$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "scripts/benchmark-agent-resources.ps1 must run on Windows; use scripts/benchmark-agent-resources.sh elsewhere"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

# Mirrors the bash script's own fallback: an explicit CARGO_TARGET_DIR wins
# (this is how `mise run` and a shared-build-directory setup reach this
# script), otherwise the workspace's own target/ directory.
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot "target" }
if (-not $Binary) {
    $Binary = Join-Path $targetDir "release\rackio.exe"
}

$defaultResultPath = Join-Path $repoRoot "test-results\resource-benchmark-windows.json"
$resultPath = if ($env:RACKIO_BENCHMARK_RESULT) { $env:RACKIO_BENCHMARK_RESULT } else { $defaultResultPath }

# Same knobs as the bash script, same defaults, so a report from either
# platform reflects the same sampling policy unless a caller deliberately
# overrides it on one side.
$sampleCount = if ($env:RACKIO_BENCHMARK_SAMPLES) { [int]$env:RACKIO_BENCHMARK_SAMPLES } else { 30 }
$warmupSeconds = if ($env:RACKIO_BENCHMARK_WARMUP_SECONDS) { [int]$env:RACKIO_BENCHMARK_WARMUP_SECONDS } else { 5 }
$cpuLimitPercent = if ($env:RACKIO_BENCHMARK_CPU_LIMIT_PERCENT) { [double]$env:RACKIO_BENCHMARK_CPU_LIMIT_PERCENT } else { 1 }
$rssLimitKib = if ($env:RACKIO_BENCHMARK_RSS_LIMIT_KIB) { [double]$env:RACKIO_BENCHMARK_RSS_LIMIT_KIB } else { 40960 }

# A budget measured against the wrong process is worse than no budget: it
# passes for reasons that have nothing to do with the agent. Every sample
# below is guarded against this by comparing the live process name to
# $expectedCommand, mirroring assert_pid_is_the_agent in the bash script.
$expectedCommand = [System.IO.Path]::GetFileNameWithoutExtension($Binary)

# Throws a RuntimeException whose Message is a single compact JSON object,
# so a catch block anywhere up the stack can hand it straight to stderr
# without knowing what kind of failure produced it. Mirrors fail() in
# scripts/benchmark-agent-resources.sh.
function Invoke-Fail {
    param(
        [Parameter(Mandatory = $true)][string]$Reason,
        [hashtable]$Extra = @{}
    )
    $payload = [ordered]@{
        check  = "agent_resources"
        status = "failed"
        os     = "windows"
        reason = $Reason
    }
    foreach ($key in $Extra.Keys) { $payload[$key] = $Extra[$key] }
    throw ($payload | ConvertTo-Json -Compress -Depth 8)
}

if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) {
    $payload = [ordered]@{
        check  = "agent_resources"
        status = "error"
        os     = "windows"
        reason = "binary_not_executable"
        path   = $Binary
    }
    [Console]::Error.WriteLine(($payload | ConvertTo-Json -Compress))
    exit 2
}

$benchmarkRoot = Join-Path ([System.IO.Path]::GetTempPath()) "rackio-resource-benchmark-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $benchmarkRoot -Force | Out-Null
foreach ($sub in @("config", "data", "state", "log")) {
    New-Item -ItemType Directory -Path (Join-Path $benchmarkRoot $sub) -Force | Out-Null
}

$env:RACKIO_CONFIG_DIR = Join-Path $benchmarkRoot "config"
$env:RACKIO_DATA_DIR = Join-Path $benchmarkRoot "data"
$env:RACKIO_STATE_DIR = Join-Path $benchmarkRoot "state"
$env:RACKIO_LOG_DIR = Join-Path $benchmarkRoot "log"
$env:RACKIO_PIPE = "\\.\pipe\rackio-benchmark-$([Guid]::NewGuid().ToString('N'))"

$agentProcess = $null
$exitCode = 0
$resultWritten = $false

try {
    $stdoutLog = Join-Path $benchmarkRoot "agent.stdout.log"
    $stderrLog = Join-Path $benchmarkRoot "agent.stderr.log"
    $agentProcess = Start-Process -FilePath $Binary -ArgumentList "daemon" `
        -PassThru -NoNewWindow `
        -RedirectStandardOutput $stdoutLog `
        -RedirectStandardError $stderrLog

    $ready = $false
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        & $Binary status *>$null
        if ($LASTEXITCODE -eq 0) {
            $ready = $true
            break
        }
        $agentProcess.Refresh()
        if ($agentProcess.HasExited) {
            Invoke-Fail -Reason "daemon_exited_during_startup"
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) {
        Invoke-Fail -Reason "daemon_startup_timeout"
    }

    $agentProcess.Refresh()
    if ($agentProcess.ProcessName -ne $expectedCommand) {
        Invoke-Fail -Reason "sampled_process_is_not_the_agent" -Extra @{
            expected = $expectedCommand
            actual   = $agentProcess.ProcessName
        }
    }

    # Startup allocation and collector warm-up are not steady-state resource
    # use; same rationale and same default as the bash script.
    Start-Sleep -Seconds $warmupSeconds

    $cpuBefore = (Get-Process -Id $agentProcess.Id).TotalProcessorTime.TotalSeconds
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    $rssSamplesKib = New-Object System.Collections.Generic.List[double]
    for ($i = 0; $i -lt $sampleCount; $i++) {
        $proc = Get-Process -Id $agentProcess.Id -ErrorAction SilentlyContinue
        if ($null -eq $proc) {
            Invoke-Fail -Reason "daemon_exited_during_sampling"
        }
        if ($proc.ProcessName -ne $expectedCommand) {
            Invoke-Fail -Reason "sampled_process_is_not_the_agent" -Extra @{
                expected = $expectedCommand
                actual   = $proc.ProcessName
            }
        }
        $rssSamplesKib.Add([double]$proc.WorkingSet64 / 1024)
        Start-Sleep -Seconds 1
    }

    $stopwatch.Stop()
    $cpuAfter = (Get-Process -Id $agentProcess.Id).TotalProcessorTime.TotalSeconds
    $cpuConsumed = $cpuAfter - $cpuBefore
    $wallElapsedSeconds = $stopwatch.Elapsed.TotalSeconds
    $windowCpuPercent = if ($wallElapsedSeconds -gt 0) { 100 * $cpuConsumed / $wallElapsedSeconds } else { 0 }
    $peakRssKib = ($rssSamplesKib | Measure-Object -Maximum).Maximum

    $status = if ($windowCpuPercent -lt $cpuLimitPercent -and $peakRssKib -lt $rssLimitKib) { "passed" } else { "failed" }

    $report = [ordered]@{
        check           = "agent_resources"
        status          = $status
        profile         = "idle_direct_only"
        os              = "windows"
        binary          = $Binary
        samples         = $sampleCount
        warmup_seconds  = $warmupSeconds
        peak_rss_kib    = [int][math]::Round($peakRssKib, 0)
        cpu_measurement = [ordered]@{
            gating_source            = "window_cpu_percent below: delta of Get-Process TotalProcessorTime (the process's own cumulative user+kernel CPU time) over the sampling window, scoped to the window and to nothing else"
            window_cpu_percent       = [math]::Round($windowCpuPercent, 3)
            cpu_seconds_consumed     = [math]::Round($cpuConsumed, 2)
            wall_seconds             = [math]::Round($wallElapsedSeconds, 3)
            average_cpu_percent_note = "no ps -o %cpu equivalent is computed on Windows: that figure was retired from gating scripts/benchmark-agent-resources.sh's macOS/Linux run because an instantaneous or decaying %CPU-style metric is dominated by process startup rather than steady-state load (see that script's cpu_seconds_of comment), and there is no reason a Windows-only decaying-average metric would behave any better"
            method                   = "(Get-Process -Id <pid>).TotalProcessorTime.TotalSeconds sampled before and after the window; wall-clock elapsed measured with System.Diagnostics.Stopwatch, which has sub-millisecond resolution and therefore does not carry the roughly one-over-samples relative error the bash script's one-second-resolution date +%s timing does"
            rss_note                 = "peak_rss_kib is Get-Process WorkingSet64 (bytes) converted to KiB, sampled once per second across the window and reduced to a maximum, mirroring peak_rss_of in scripts/benchmark-agent-resources.sh. Windows working-set and POSIX RSS are the same concept (current resident physical memory) but are computed by different kernels; the two numbers are comparable but not defined identically"
        }
        limits          = [ordered]@{
            cpu_percent_exclusive  = $cpuLimitPercent
            cpu_percent_gated_on   = "cpu_measurement.window_cpu_percent"
            peak_rss_kib_exclusive = $rssLimitKib
        }
        traffic_note    = "this script covers only the idle_direct_only profile. The active_peer profile's traffic measurement (scripts/benchmark-agent-resources.sh, nettop -P) has no Windows implementation here: nettop is macOS-only and Windows exposes no equivalent per-process UDP byte counter without a packet capture; see docs/release-checklist.md for how that gap is tracked"
    }

    $json = $report | ConvertTo-Json -Compress -Depth 8
    $resultDirectory = Split-Path -Parent $resultPath
    New-Item -ItemType Directory -Path $resultDirectory -Force | Out-Null
    Set-Content -LiteralPath $resultPath -Value $json -Encoding utf8
    $resultWritten = $true

    # Pretty-print to stdout the same way `jq` naturally pretty-prints the
    # bash script's `cat "$result_path"` output, so a human reading the
    # console output does not have to reformat a single compact line.
    $report | ConvertTo-Json -Depth 8 | Write-Output

    if ($status -ne "passed") {
        $exitCode = 1
    }
} catch {
    $message = $_.Exception.Message
    $parsedOk = $true
    try { $null = $message | ConvertFrom-Json -ErrorAction Stop } catch { $parsedOk = $false }
    if ($parsedOk) {
        [Console]::Error.WriteLine($message)
    } else {
        $payload = [ordered]@{
            check   = "agent_resources"
            status  = "error"
            os      = "windows"
            reason  = "unexpected_exception"
            message = $message
        }
        [Console]::Error.WriteLine(($payload | ConvertTo-Json -Compress))
    }
    $exitCode = 1
} finally {
    if ($null -ne $agentProcess) {
        $agentProcess.Refresh()
        if (-not $agentProcess.HasExited) {
            Stop-Process -Id $agentProcess.Id -Force -ErrorAction SilentlyContinue
            $agentProcess.WaitForExit(5000) | Out-Null
        }
    }
    if (-not $resultWritten -and $exitCode -ne 0) {
        $resultDirectory = Split-Path -Parent $resultPath
        if ($resultDirectory -and -not (Test-Path -LiteralPath $resultDirectory)) {
            New-Item -ItemType Directory -Path $resultDirectory -Force | Out-Null
        }
        $logCopies = @(
            @{ Source = (Join-Path $benchmarkRoot "agent.stdout.log"); Suffix = ".stdout.log" }
            @{ Source = (Join-Path $benchmarkRoot "agent.stderr.log"); Suffix = ".stderr.log" }
        )
        foreach ($logCopy in $logCopies) {
            if (Test-Path -LiteralPath $logCopy.Source) {
                Copy-Item -LiteralPath $logCopy.Source -Destination "$resultPath$($logCopy.Suffix)" -Force -ErrorAction SilentlyContinue
            }
        }
    }
    Remove-Item -LiteralPath $benchmarkRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit $exitCode
