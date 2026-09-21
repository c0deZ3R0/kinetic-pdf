# Takes the app screenshots the Store images are made from, of the sample
# drawing set, in a profile of their own. Run from the repo root after
#   cargo build --release
#   cargo run --release --example store_drawings
# Leave the mouse and keyboard alone while it runs: it stops if another
# window comes to the front rather than send anything there.
#
# Pictures go to target\store-images\raw; compose.ps1 frames them.

param([string]$Profile = (Join-Path $env:TEMP "kinetic-pdf-store-profile"))

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
. (Join-Path $PSScriptRoot "drive.ps1")
$set = Join-Path $root "target\store-images"
$raw = Join-Path $set "raw"
New-Item -ItemType Directory -Force $raw | Out-Null

# Client-area positions, at a client area of 2240 x 1360.
$railTools = @(29, 178)
$tableButton = @(36, 1337)
$fitPage = @(367, 1337)

Get-Process kinetic-pdf -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
Remove-Item -Recurse -Force $Profile -ErrorAction SilentlyContinue
Start-Kp (Join-Path $root "target\release\kinetic-pdf.exe") (Join-Path $set "kestrel-lane.pdf") $Profile (Join-Path $set "kestrel-lane-tools.json") -Width 2240 -Height 1360
Start-Sleep 4

function Rest { Move-To 2200 1250; Start-Sleep -Seconds $args[0] }

# 1. The plan, zoomed in on the house.
Keys "{PGDN}"
Click @fitPage
Zoom-At 1053 818 2
Rest 3
Shot (Join-Path $raw "1-plan.png")

# 2. Measuring: the kept tools, and the open plan's area picked out.
Click @railTools
Zoom-At 1297 753 1
Click 1456 784
Rest 2
Shot (Join-Path $raw "2-measure.png")
