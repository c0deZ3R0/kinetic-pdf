#requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$BaselineApp,
    [Parameter(Mandatory)][string]$CurrentApp,
    [Parameter(Mandatory)][string]$BaselineHarness,
    [Parameter(Mandatory)][string]$CurrentHarness,
    [Parameter(Mandatory)][string]$Drawing,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [int]$Repeats = 5
)
$ErrorActionPreference = 'Stop'
if ($Repeats -lt 1) { throw 'Repeats must be positive' }
$BaselineApp = (Resolve-Path -LiteralPath $BaselineApp).Path
$CurrentApp = (Resolve-Path -LiteralPath $CurrentApp).Path
$BaselineHarness = (Resolve-Path -LiteralPath $BaselineHarness).Path
$CurrentHarness = (Resolve-Path -LiteralPath $CurrentHarness).Path
$Drawing = (Resolve-Path -LiteralPath $Drawing).Path
$root = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $root) { throw 'Use a new output directory for each comparison' }
New-Item -ItemType Directory -Path $root | Out-Null
$results = [Collections.Generic.List[object]]::new()

function Invoke-Measurement([string]$Executable, [string[]]$Arguments, [string]$Name, [hashtable]$Environment) {
    $info = [Diagnostics.ProcessStartInfo]::new($Executable)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    # Do not read or overwrite the user's settings while benchmarking.
    foreach ($variable in @('APPDATA', 'LOCALAPPDATA')) {
        $directory = Join-Path $root "$Name-$variable"
        New-Item -ItemType Directory -Path $directory | Out-Null
        $info.Environment[$variable] = $directory
    }
    $info.Environment['KINETIC_PDF_UPDATE'] = '0'
    $info.Environment['KINETIC_PDF_CACHE'] = Join-Path $root "$Name-cache"
    foreach ($key in $Environment.Keys) { $info.Environment[$key] = [string]$Environment[$key] }
    Write-Host "Running $Name"
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (!$process.WaitForExit(180000)) {
            $process.Kill($true)
            throw "$Name timed out"
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errors = $stderr.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $root "$Name.log"), "$output`n$errors")
        if ($process.ExitCode -ne 0) { throw "$Name exited $($process.ExitCode); see its log" }
        return "$output`n$errors"
    } finally { $process.Dispose() }
}

$fixture = Join-Path $root 'text-300-pages.pdf'
for ($pair = 0; $pair -lt 3; $pair++) {
    $order = if ($pair % 2 -eq 0) { @('baseline', 'current') } else { @('current', 'baseline') }
    foreach ($build in $order) {
        $exe = if ($build -eq 'baseline') { $BaselineHarness } else { $CurrentHarness }
        $log = Invoke-Measurement $exe @('--ignored', '--nocapture') "engine-$pair-$build" @{ KINETIC_PERF_FIXTURE = $fixture }
        $measurements = @($log -split "`n" | Where-Object { $_.StartsWith('PERF ') })
        if ($measurements.Count -ne 4) { throw 'Missing engine measurements' }
        foreach ($line in $measurements) {
            $data = $line.Substring(5) | ConvertFrom-Json
            $results.Add(@{ build = $build; pair = $pair; operation = $data.operation; samples_ms = $data.samples_ms })
        }
    }
}

foreach ($document in @(@{ name = 'text'; path = $fixture }, @{ name = 'drawing'; path = $Drawing })) {
    for ($pair = 0; $pair -le $Repeats; $pair++) {
        $order = if ($pair % 2 -eq 0) { @('baseline', 'current') } else { @('current', 'baseline') }
        foreach ($build in $order) {
            $exe = if ($build -eq 'baseline') { $BaselineApp } else { $CurrentApp }
            $log = Invoke-Measurement $exe @($document.path) "navigation-$($document.name)-$pair-$build" @{ KINETIC_PDF_WORK_BENCH = '0' }
            if ($log -match 'gave up' -or $log -notmatch 'work-bench-steps-ms:') { throw 'Navigation did not complete; inspect the log' }
            foreach ($line in ($log -split "`n")) {
                if ($line -match '^work-bench-(steps-ms|frame-ms|frame-p90-ms): (.*)') {
                    $samples = @($Matches[2].Split(',') | ForEach-Object { [double]::Parse($_.Trim(), [Globalization.CultureInfo]::InvariantCulture) })
                    $results.Add(@{ build = $build; pair = $pair; warmup = ($pair -eq 0); operation = "$($document.name)-$($Matches[1])"; samples_ms = $samples })
                }
            }
        }
    }
}
$results | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $root 'samples.json')
Get-FileHash -LiteralPath $BaselineApp, $CurrentApp, $Drawing, $fixture | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'inputs.json')
Write-Host "Results: $root"
