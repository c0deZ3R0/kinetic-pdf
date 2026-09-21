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

# 3. The quantities, grouped by description, the table dragged tall and
#    scrolled past the first notes.
Keys "{ESC}"
Click @railTools
Click @tableButton
Click 298 995
Drag 1500 971 1500 560
Click 269 538
Move-To 1100 1000
for ($i = 0; $i -lt 3; $i++) { [Win]::mouse_event(0x0800, 0, 0, -120, [UIntPtr]::Zero); Start-Sleep -Milliseconds 150 }
Rest 2
Shot (Join-Path $raw "3-quantities.png")
Click 36 538

# 4. Speed: deep into the slab detail on the last sheet.
Keys "{END}"
Click @fitPage
Move-To 1100 700
for ($i = 0; $i -lt 5; $i++) { [Win]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero); Start-Sleep -Milliseconds 150 }
Zoom-At 590 300 8
Zoom-At 700 300 -2
Drag 900 400 1300 950 -Middle
Drag 1200 700 950 700 -Middle
Rest 4
Shot (Join-Path $raw "4-speed.png")

# 5. Arranging: sheets side by side, sheet 4 being dragged ahead of sheet 2.
Zoom-At 1100 700 -12
Click 73 15
Click 173 214
Keys "{HOME}"
Drag 1000 700 480 760 -Middle
Zoom-At 970 700 -1
Drag 1000 800 1174 643 -Middle
Drag 1100 500 1100 670 -Middle
Click 1364 715
Focus-Kp; Move-To 1364 715
[Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
for ($i = 1; $i -le 25; $i++) { Move-To (1364 + (1148 - 1364) * $i / 25) (715 + (470 - 715) * $i / 25) }
Start-Sleep 1
Shot (Join-Path $raw "5-arrange.png")
Keys "{ESC}"
[Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)

# 6. Markup: the notes sheet, with a highlight's note open.
Click 73 15
Click 173 214
Click 1200 700
Keys "{ESC}"
Keys "{HOME}"
Click 269 1337
Zoom-At 380 600 3
Drag 700 800 980 520 -Middle
Drag 900 1000 1250 1000 -Middle
Click 1020 623
Rest 1
Shot (Join-Path $raw "6-markup.png")
Keys "{ESC}"
Stop-Kp
