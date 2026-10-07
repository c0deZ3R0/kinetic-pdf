# Prepare an isolated app, then record its window with stream-recorder and run -Take.
param(
    [switch]$Take,
    [string]$Profile = (Join-Path $env:TEMP "kinetic-pdf-ctrl-k-demo"),
    [string]$Pdf,
    [string]$Tools
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
. (Join-Path $PSScriptRoot 'drive.ps1')
New-Item -ItemType Directory -Force $Profile | Out-Null
$pidFile = Join-Path $Profile 'demo-pid.txt'
if (!$Take) {
    if (!$Pdf -or !$Tools) { throw 'Provide the generated PDF and tools JSON with -Pdf and -Tools.' }
    $catalog = Get-Content -LiteralPath $Tools -Raw | ConvertFrom-Json
    $catalog.saved[0].name = 'Concrete slab 200'
    $catalog.saved[0].group = 'Concrete'
    $catalog.saved[0].settings.defaults.description = 'Concrete slab, 200 mm thick'
    $catalog.saved[0].settings.depth_m = 0.2
    $demoTools = Join-Path $Profile 'demo-tools.json'
    [IO.File]::WriteAllText($demoTools, ($catalog | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding $false))
    $demoPdf = Join-Path $Profile 'demo-drawing.pdf'
    Copy-Item -LiteralPath $Pdf -Destination $demoPdf -Force
    $prefsDir = Join-Path $Profile 'roaming/kinetic-pdf'
    New-Item -ItemType Directory -Force $prefsDir | Out-Null
    $versionMatch = [regex]::Match((Get-Content (Join-Path $root 'Cargo.toml') -Raw), '(?m)^version\s*=\s*"([^"]+)"')
    [IO.File]::WriteAllText((Join-Path $prefsDir 'settings.json'), (@{seen_version=$versionMatch.Groups[1].Value} | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
    Start-Kp (Join-Path $root 'target/release/kinetic-pdf.exe') $demoPdf $Profile $demoTools -Width 1920 -Height 1080
    $script:Kp.Id | Set-Content -LiteralPath $pidFile
    Start-Sleep -Seconds 2
    Keys '{PGDN}'
    Keys '{ESC}'
    Keys '^a'
    Keys '{DELETE}'
    Move-Smooth 1710 870
    Shot (Join-Path $Profile 'prepared.png')
    exit
}
$demoPid = [int](Get-Content -LiteralPath $pidFile)
$script:Kp = Get-Process -Id $demoPid
$script:H = [Win]::Titled([uint32]$demoPid, 'Kinetic PDF')
if ($script:H -eq [IntPtr]::Zero) { throw 'The prepared demo window is no longer open.' }
Focus-Kp
Start-Sleep -Milliseconds 900
# Hold the chord across several app frames.
[Win]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
try {
    [Win]::keybd_event(0x4B, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [Win]::keybd_event(0x4B, 0, 2, [UIntPtr]::Zero)
} finally { [Win]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero) }
Start-Sleep -Milliseconds 900
foreach ($letter in 'slab'.ToCharArray()) { Keys ([string]$letter); Start-Sleep -Milliseconds 90 }
Start-Sleep -Milliseconds 1100
Shot (Join-Path $Profile 'search.png')
Keys '{ENTER}'
Move-Smooth 500 578 700
Click 500 578
Move-Smooth 744 578 500
Click 744 578
Move-Smooth 744 724 500
Click 744 724
Move-Smooth 500 724 500
Click 500 724
Keys '{ENTER}'
Move-Smooth 1450 870 600
Start-Sleep -Milliseconds 1200
Shot (Join-Path $Profile 'selected.png')
Write-Host 'Demo complete'
