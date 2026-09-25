# Draws the image to share that the app is out on the Store: 2400 x 1254,
# which is 1200 x 627 at twice the density. Takes its own screenshot of the
# plan, as capture.ps1 does for the first Store image, so it shows the build
# in target\release. Run from the repo root after
#   cargo build --release
#   cargo run --release --example store_drawings
# Leave the mouse and keyboard alone for the few seconds the app is up.
#
# Writes packaging\store-images\announce.png.

param([string]$Profile = (Join-Path $env:TEMP "kinetic-pdf-store-profile"))

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
. (Join-Path $PSScriptRoot "drive.ps1")
$set = Join-Path $root "target\store-images"
$work = Join-Path $set "announce"
New-Item -ItemType Directory -Force $work | Out-Null
$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"

# The plan, zoomed in on the house: step 1 of capture.ps1.
Get-Process kinetic-pdf -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
Remove-Item -Recurse -Force $Profile -ErrorAction SilentlyContinue
Start-Kp (Join-Path $root "target\release\kinetic-pdf.exe") (Join-Path $set "kestrel-lane.pdf") $Profile (Join-Path $set "kestrel-lane-tools.json") -Width 2240 -Height 1360
Start-Sleep 4
Keys "{PGDN}"
Click 367 1337
Zoom-At 1053 818 2
Move-To 2200 1250; Start-Sleep 3
# Beside the page, since a headless Edge won't load a picture from elsewhere.
Shot (Join-Path $work "shot.png")
Stop-Kp

Copy-Item (Join-Path $root "packaging\Assets\Square150x150Logo.png") (Join-Path $work "logo.png") -Force
$page = Join-Path $work "announce.html"
Copy-Item (Join-Path $PSScriptRoot "announce.html") $page -Force
$out = Join-Path $PSScriptRoot "announce.png"
& $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 --window-size=1200,627 --allow-file-access-from-files "--screenshot=$out" ([Uri]$page).AbsoluteUri 2>$null | Out-Null
Write-Host "wrote $out"
